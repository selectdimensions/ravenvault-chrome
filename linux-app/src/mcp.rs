//! Minimal MCP (Model Context Protocol) stdio client for MemPalace.
//!
//! The `relate` feature needs to query MemPalace semantic search once per note in
//! the vault. Spawning the `mempalace` CLI per note is far too slow, so instead we
//! talk to the persistent MemPalace MCP server (`mempalace-mcp`) over its stdio
//! JSON-RPC transport and reuse the one process for every query.
//!
//! Transport is **newline-delimited JSON**: one JSON object per line on stdin and
//! stdout. We perform the standard MCP handshake (`initialize` +
//! `notifications/initialized`), then issue `tools/call` requests for the
//! `mempalace_search` tool. The search tool returns its payload as a JSON string
//! inside `result.content[0].text`, which we parse a second time.

use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// One semantically-related note returned by `mempalace_search`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Absolute or relative path of the matched source file.
    pub source_file: String,
    /// Cosine-style similarity score; higher is more related.
    pub similarity: f64,
}

/// A live connection to a spawned MemPalace MCP server.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl McpClient {
    /// Resolve the MCP server binary to use, given the configured `mempalace`
    /// binary. Honors `RAVENVAULT_MEMPALACE_MCP_BIN`; otherwise derives a sensible
    /// default: if `mempalace_binary` ends in `mempalace`, append `-mcp`, else
    /// fall back to `mempalace-mcp` on PATH.
    pub fn resolve_bin(mempalace_binary: &str) -> String {
        if let Ok(v) = std::env::var("RAVENVAULT_MEMPALACE_MCP_BIN") {
            if !v.is_empty() {
                return v;
            }
        }
        if mempalace_binary.ends_with("mempalace") {
            format!("{mempalace_binary}-mcp")
        } else {
            "mempalace-mcp".to_string()
        }
    }

    /// Spawn the MCP server binary and perform the MCP handshake.
    pub async fn spawn(bin: &str) -> Result<Self> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn MCP server `{bin}` (is it installed?)"))?;

        let stdin = child.stdin.take().context("MCP server has no stdin")?;
        let stdout = child.stdout.take().context("MCP server has no stdout")?;
        let lines = BufReader::new(stdout).lines();

        let mut client = McpClient {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        client.handshake().await?;
        Ok(client)
    }

    /// Perform the MCP `initialize` handshake and send `notifications/initialized`.
    async fn handshake(&mut self) -> Result<()> {
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "poe2obsidian", "version": "0.11.0"}
            }
        });
        self.write_message(&init).await?;
        // Read until the response with id == 1.
        let _ = self.read_response(1).await?;
        self.next_id = 2;

        let initialized = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.write_message(&initialized).await?;
        Ok(())
    }

    /// Search MemPalace for notes related to `query`. Returns the parsed hits.
    pub async fn search(
        &mut self,
        query: &str,
        wing: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Hit>> {
        let query = truncate_chars(query, 250);
        let id = self.next_id;
        self.next_id += 1;

        let mut arguments = json!({
            "query": query,
            "limit": limit,
        });
        if let Some(w) = wing {
            arguments["wing"] = json!(w);
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "mempalace_search",
                "arguments": arguments,
            }
        });
        self.write_message(&req).await?;
        let result = self.read_response(id).await?;

        // result.content[0].text is itself a JSON string.
        let text = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .context("MCP search result missing content[0].text")?;

        let payload: Value =
            serde_json::from_str(text).context("MCP search content text is not valid JSON")?;

        let mut hits = Vec::new();
        if let Some(results) = payload.get("results").and_then(|r| r.as_array()) {
            for r in results {
                let Some(source_file) = r.get("source_file").and_then(|s| s.as_str()) else {
                    continue;
                };
                let similarity = r.get("similarity").and_then(|s| s.as_f64()).unwrap_or(0.0);
                hits.push(Hit {
                    source_file: source_file.to_string(),
                    similarity,
                });
            }
        }
        Ok(hits)
    }

    /// Explicitly shut the server down (dropping also kills it via kill_on_drop).
    pub async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    /// Write a single JSON message followed by a newline.
    async fn write_message(&mut self, value: &Value) -> Result<()> {
        let mut buf = serde_json::to_vec(value).context("serialize MCP message")?;
        buf.push(b'\n');
        self.stdin
            .write_all(&buf)
            .await
            .context("write to MCP server stdin")?;
        self.stdin.flush().await.context("flush MCP server stdin")?;
        Ok(())
    }

    /// Read lines until one parses as a JSON object whose `id` matches `id`.
    /// Returns its `result`, or an error if the response carried an `error`.
    async fn read_response(&mut self, id: u64) -> Result<Value> {
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .context("reading MCP server stdout")?
                .context("MCP server closed stdout before responding")?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue; // Non-JSON log line; ignore.
            };
            let matches = value.get("id").and_then(|v| v.as_u64()) == Some(id);
            if !matches {
                continue;
            }
            if let Some(err) = value.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown MCP error");
                return Err(anyhow!("MCP error: {msg}"));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// Truncate a string to at most `max` chars (not bytes), without splitting a char.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a fake `sh` MCP server that answers the handshake and any search.
    fn fake_server(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake-mempalace-mcp");
        // Reads JSON-RPC lines; replies to `initialize` (id 1) and any
        // `tools/call` with a canned search result. content[0].text is a JSON
        // string holding two results.
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}'
      ;;
    *'"method":"notifications/initialized"'*)
      : # notification, no reply
      ;;
    *'"method":"tools/call"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"results\\\":[{\\\"source_file\\\":\\\"/v/Alpha.md\\\",\\\"similarity\\\":0.91},{\\\"source_file\\\":\\\"/v/Beta.md\\\",\\\"similarity\\\":0.42}]}\"}]}}"
      ;;
  esac
done
"#;
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[tokio::test]
    async fn handshake_and_search_against_fake_server() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_server(dir.path());

        let mut client = McpClient::spawn(&bin.display().to_string()).await.unwrap();
        let hits = client.search("rust async", None, 8).await.unwrap();

        assert_eq!(
            hits,
            vec![
                Hit {
                    source_file: "/v/Alpha.md".to_string(),
                    similarity: 0.91,
                },
                Hit {
                    source_file: "/v/Beta.md".to_string(),
                    similarity: 0.42,
                },
            ]
        );
        client.shutdown().await;
    }

    #[test]
    fn resolve_bin_derives_default() {
        // Guard env isolation: only valid when the override is unset.
        if std::env::var("RAVENVAULT_MEMPALACE_MCP_BIN").is_err() {
            assert_eq!(McpClient::resolve_bin("mempalace"), "mempalace-mcp");
            assert_eq!(
                McpClient::resolve_bin("/opt/bin/mempalace"),
                "/opt/bin/mempalace-mcp"
            );
            assert_eq!(McpClient::resolve_bin("something-else"), "mempalace-mcp");
        }
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel");
        let s = "é".repeat(300);
        assert_eq!(truncate_chars(&s, 250).chars().count(), 250);
    }
}
