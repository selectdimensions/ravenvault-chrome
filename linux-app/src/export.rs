//! The export orchestration state machine.
//!
//! The app drives the whole export: on `invoke_export` it runs the keep-alive +
//! scroll loop until the conversation is fully loaded, asks the extension to
//! capture the page HTML, reassembles the chunked HTML, and (from M3/M4) converts
//! it to Markdown and writes it to the vault. See `docs/PROTOCOL.md` §9.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::client::Client;
use crate::context::AppContext;
use crate::html2md;
use crate::protocol::Envelope;
use crate::vault::{ExportNote, VaultWriter};

/// Timeout for a single orchestration request (scroll/dom/window commands).
const TIMEOUT_CMD: Duration = Duration::from_secs(10);
/// Max time to wait for the next capture/asset stream message.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60);
/// Pause after each scroll step to let Poe lazy-load older messages.
const SCROLL_SETTLE: Duration = Duration::from_millis(350);
/// Hard cap on scroll iterations (safety against a runaway loop).
const MAX_SCROLL_ITERS: usize = 2000;
/// Consecutive no-progress scroll reads before giving up reaching the top.
const MAX_SCROLL_STALLS: u32 = 4;
/// Selector used to count loaded messages for progress reporting.
const MSG_SELECTOR: &str = r#"[class*="ChatMessage_chatMessage"]"#;

/// The export tab/window the extension is operating on.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub tab_id: i64,
    pub window_id: i64,
}

impl Session {
    /// Extract the session from an `invoke_export` (or similar) envelope.
    /// Mirrors the extension's `getTabIdFromMessage`: `args.tabId || args.session.tabId`.
    pub fn from_envelope(env: &Envelope) -> Session {
        let args = env.args.clone().unwrap_or(Value::Null);
        let tab_id = num_field(&args, "tabId")
            .or_else(|| args.get("session").and_then(|s| num_field(s, "tabId")))
            .unwrap_or(0);
        let window_id = num_field(&args, "windowId")
            .or_else(|| args.get("session").and_then(|s| num_field(s, "windowId")))
            .unwrap_or(0);
        Session { tab_id, window_id }
    }

    fn json(&self) -> Value {
        json!({ "tabId": self.tab_id, "windowId": self.window_id })
    }
}

/// A fully captured conversation page, ready for HTML→Markdown conversion (M3).
#[derive(Debug, Clone)]
pub struct Capture {
    pub chat_title: String,
    pub html: String,
}

/// Entry point: run a complete export for an `invoke_export` envelope. Always
/// clears the busy guard and session-active flag on exit.
pub async fn run_export(client: Client, ctx: Arc<AppContext>, invoke: &Envelope) {
    let session = Session::from_envelope(invoke);
    {
        let mut s = ctx.session.lock().await;
        s.active = true;
        s.tab_id = session.tab_id;
        s.window_id = session.window_id;
        s.status = "Preparing export…".into();
        s.current = 0;
        s.total = 0;
    }

    let (in_tx, in_rx) = mpsc::unbounded_channel();
    client.set_session_inbox(in_tx).await;
    let result = do_export(&client, &ctx, &session, in_rx).await;
    client.clear_session_inbox().await;

    {
        let mut s = ctx.session.lock().await;
        s.active = false;
        s.status = "Idle".into();
    }
    *ctx.busy.lock().await = false;

    match result {
        Ok(outcome) => {
            match &outcome.note_path {
                Some(p) => {
                    info!(title = %outcome.title, path = %p.display(), "export written to vault")
                }
                None => info!(title = %outcome.title, "export captured (no vault configured)"),
            }
            let msg = if outcome.note_path.is_some() {
                "Export complete"
            } else {
                "Captured — set RAVENVAULT_VAULT to save to your vault"
            };
            let _ = client.send(update_ui(&session, "success", msg));
        }
        Err(e) => {
            warn!(error = %e, "export failed");
            let _ = client.send(abort_export(&session, &e.to_string()));
        }
    }
}

/// What an export produced.
#[derive(Debug, Clone)]
struct ExportResult {
    title: String,
    /// Path of the written note, or `None` if no vault is configured.
    note_path: Option<PathBuf>,
}

/// The export flow proper: scroll, capture, convert, download assets, write.
async fn do_export(
    client: &Client,
    ctx: &Arc<AppContext>,
    session: &Session,
    mut inbox: mpsc::UnboundedReceiver<Envelope>,
) -> Result<ExportResult> {
    // Start the service-worker keep-alive so the tab stays live during scrolling.
    let _ = client
        .request(
            scroll_req("startKeepAlive", session, json!({})),
            TIMEOUT_CMD,
        )
        .await;

    scroll_to_top(client, ctx, session).await?;

    // Stop active scrolling before capture.
    let _ = client
        .request(scroll_req("stopScroll", session, json!({})), TIMEOUT_CMD)
        .await;

    set_status(ctx, "Capturing…").await;
    client.send(Envelope::command(
        "capture_start",
        json!({ "session": session.json() }),
    ))?;

    let capture = collect_capture(&mut inbox).await?;

    // Convert HTML -> Markdown (M3).
    let convo = html2md::html_to_markdown(&capture.html, &capture.chat_title);

    // Download referenced assets via the extension relay (M2 mechanism).
    let mut assets: HashMap<String, Vec<u8>> = HashMap::new();
    let total_assets = convo.asset_urls.len();
    for (i, url) in convo.asset_urls.iter().enumerate() {
        set_status(
            ctx,
            &format!("Downloading assets… {}/{total_assets}", i + 1),
        )
        .await;
        match download_asset(client, &mut inbox, url).await {
            Ok(bytes) => {
                assets.insert(url.clone(), bytes);
            }
            // A missing asset shouldn't fail the whole export.
            Err(e) => warn!(%url, error = %e, "asset download failed; skipping"),
        }
    }

    let _ = client
        .request(scroll_req("stopKeepAlive", session, json!({})), TIMEOUT_CMD)
        .await;

    // Write to the vault (M4), if one is configured.
    let note_path = match &ctx.vault_path {
        Some(root) => {
            set_status(ctx, "Writing to vault…").await;
            let note = ExportNote {
                title: convo.title.clone(),
                markdown: convo.markdown,
                assets,
                source_url: None,
                created: None,
            };
            let writer = VaultWriter::new(root.clone());
            Some(writer.write(note)?)
        }
        None => None,
    };

    Ok(ExportResult {
        title: convo.title,
        note_path,
    })
}

/// Drive the scroll loop until the conversation is scrolled to the top (oldest
/// message), counting loaded messages for progress.
async fn scroll_to_top(client: &Client, ctx: &Arc<AppContext>, session: &Session) -> Result<()> {
    let mut last_top = f64::NAN;
    let mut stalls = 0u32;

    for i in 0..MAX_SCROLL_ITERS {
        let metrics = client
            .request(
                scroll_req("scrollGetMetrics", session, json!({})),
                TIMEOUT_CMD,
            )
            .await
            .context("scrollGetMetrics failed")?;

        if result_bool(&metrics, "atTop") {
            debug!(iters = i, "reached top of conversation");
            break;
        }

        let scroll_top = result_f64(&metrics, "scrollTop").unwrap_or(0.0);
        let client_h = result_f64(&metrics, "clientHeight").unwrap_or(800.0);

        // Update progress from the message count (best-effort).
        if let Ok(q) = client
            .request(
                scroll_req(
                    "domQuery",
                    session,
                    json!({ "selector": MSG_SELECTOR, "inContainer": true }),
                ),
                TIMEOUT_CMD,
            )
            .await
        {
            if let Some(count) = result_f64(&q, "count") {
                let mut s = ctx.session.lock().await;
                s.current = count as u64;
                s.status = format!("Scrolling… {} messages", count as u64);
            }
        }

        // Scroll up by ~80% of a viewport. A negative delta moves toward the top
        // for both normal and column-reverse (acceptsNegative) containers.
        let delta = -(client_h * 0.8).max(200.0);
        client
            .request(
                scroll_req("scrollBy", session, json!({ "delta": delta })),
                TIMEOUT_CMD,
            )
            .await
            .context("scrollBy failed")?;

        // Stall detection: if scrollTop stops changing, assume we're done/stuck.
        if (scroll_top - last_top).abs() < 1.0 {
            stalls += 1;
            if stalls >= MAX_SCROLL_STALLS {
                debug!(iters = i, "scroll stalled; assuming top reached");
                break;
            }
        } else {
            stalls = 0;
        }
        last_top = scroll_top;

        tokio::time::sleep(SCROLL_SETTLE).await;
    }
    Ok(())
}

/// Reassemble the chunked page HTML streamed via `saveDomHtmlChunk`, terminated
/// by `capture_complete`. All chunks share one logical document; order by index.
async fn collect_capture(inbox: &mut mpsc::UnboundedReceiver<Envelope>) -> Result<Capture> {
    let mut chunks: Vec<Option<String>> = Vec::new();

    let chat_title = loop {
        let env = timeout(STREAM_TIMEOUT, inbox.recv())
            .await
            .map_err(|_| anyhow!("timed out waiting for capture stream"))?
            .ok_or_else(|| anyhow!("connection closed during capture"))?;

        match env.command.as_deref() {
            Some("saveDomHtmlChunk") => {
                let args = env.args.clone().unwrap_or(Value::Null);
                let idx = num_field(&args, "chunkIndex").unwrap_or(0) as usize;
                let total = num_field(&args, "totalChunks").unwrap_or(0) as usize;
                let b64 = str_field(&args, "chunkBase64").unwrap_or_default();
                if chunks.len() < total {
                    chunks.resize(total, None);
                }
                if idx >= chunks.len() {
                    chunks.resize(idx + 1, None);
                }
                chunks[idx] = Some(b64);
            }
            Some("capture_complete") => {
                let args = env.args.clone().unwrap_or(Value::Null);
                break str_field(&args, "chatTitle").unwrap_or_default();
            }
            Some("request_abort") => return Err(anyhow!("export cancelled by user")),
            // reset_timeout / update_tab_status are advisory during capture.
            other => debug!(?other, "ignoring stream message during capture"),
        }
    };

    let html = reassemble_base64(&chunks)?;
    Ok(Capture { chat_title, html })
}

/// Download one asset via the extension relay: send `download {url}`, collect the
/// `save_file` chunks for that url, finish on `save_file_complete`.
pub async fn download_asset(
    client: &Client,
    inbox: &mut mpsc::UnboundedReceiver<Envelope>,
    url: &str,
) -> Result<Vec<u8>> {
    client.send(Envelope::request("download", json!({ "url": url })))?;

    let mut chunks: Vec<Option<String>> = Vec::new();
    loop {
        let env = timeout(STREAM_TIMEOUT, inbox.recv())
            .await
            .map_err(|_| anyhow!("timed out downloading {url}"))?
            .ok_or_else(|| anyhow!("connection closed downloading {url}"))?;

        let args = env.args.clone().unwrap_or(Value::Null);
        if str_field(&args, "url").as_deref() != Some(url) {
            debug!("stream message for a different url; ignoring");
            continue;
        }
        match env.command.as_deref() {
            Some("save_file") => {
                let idx = num_field(&args, "chunkIndex").unwrap_or(0) as usize;
                let total = num_field(&args, "totalChunks").unwrap_or(0) as usize;
                let b64 = str_field(&args, "chunkBase64").unwrap_or_default();
                if chunks.len() < total {
                    chunks.resize(total, None);
                }
                if idx >= chunks.len() {
                    chunks.resize(idx + 1, None);
                }
                chunks[idx] = Some(b64);
            }
            Some("save_file_complete") => break,
            Some("save_file_error") => {
                let msg = str_field(&args, "message").unwrap_or_default();
                return Err(anyhow!("asset fetch failed for {url}: {msg}"));
            }
            other => debug!(?other, "unexpected message during download"),
        }
    }
    let bytes = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let s = c
                .as_ref()
                .ok_or_else(|| anyhow!("missing asset chunk {i}"))?;
            base64::engine::general_purpose::STANDARD
                .decode(s)
                .with_context(|| format!("bad base64 in asset chunk {i}"))
        })
        .collect::<Result<Vec<Vec<u8>>>>()?
        .concat();
    Ok(bytes)
}

// ---- helpers ---------------------------------------------------------------

fn reassemble_base64(chunks: &[Option<String>]) -> Result<String> {
    let mut bytes = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let s = c.as_ref().ok_or_else(|| anyhow!("missing chunk {i}"))?;
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(s)
                .with_context(|| format!("bad base64 in chunk {i}"))?,
        );
    }
    String::from_utf8(bytes).context("captured HTML was not valid UTF-8")
}

/// Build a `request` envelope carrying the session, as the extension's
/// orchestration commands expect.
fn scroll_req(command: &str, session: &Session, extra: Value) -> Envelope {
    let mut args = json!({ "session": session.json() });
    if let (Some(obj), Some(extra_obj)) = (args.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    Envelope::request(command, args)
}

fn update_ui(session: &Session, ui_type: &str, message: &str) -> Envelope {
    Envelope::command(
        "update_ui",
        json!({
            "tabId": session.tab_id,
            "session": session.json(),
            "ui": { "type": ui_type, "message": message }
        }),
    )
}

fn abort_export(session: &Session, message: &str) -> Envelope {
    Envelope::command(
        "abort_export",
        json!({
            "tabId": session.tab_id,
            "windowId": session.window_id,
            "message": message
        }),
    )
}

async fn set_status(ctx: &Arc<AppContext>, status: &str) {
    ctx.session.lock().await.status = status.to_string();
}

/// Read a numeric field that may arrive as a JSON number or a stringified number
/// (extension results are flattened to strings).
fn num_field(v: &Value, key: &str) -> Option<i64> {
    let f = v.get(key)?;
    f.as_i64()
        .or_else(|| f.as_f64().map(|x| x as i64))
        .or_else(|| f.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn result_str<'a>(env: &'a Envelope, key: &str) -> Option<&'a str> {
    env.result.as_ref()?.get(key)?.as_str()
}

fn result_f64(env: &Envelope, key: &str) -> Option<f64> {
    result_str(env, key)?.parse().ok()
}

fn result_bool(env: &Envelope, key: &str) -> bool {
    result_str(env, key) == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_from_envelope_prefers_top_level_then_session() {
        let e = Envelope::command("invoke_export", json!({"tabId": 7, "windowId": 3}));
        let s = Session::from_envelope(&e);
        assert_eq!((s.tab_id, s.window_id), (7, 3));

        let e2 = Envelope::command(
            "capture_start",
            json!({"session": {"tabId": 9, "windowId": 4}}),
        );
        let s2 = Session::from_envelope(&e2);
        assert_eq!((s2.tab_id, s2.window_id), (9, 4));
    }

    #[test]
    fn reassembles_ordered_base64_chunks() {
        use base64::engine::general_purpose::STANDARD;
        let parts = ["<html>", "<body>hi</body>", "</html>"];
        let chunks: Vec<Option<String>> = parts.iter().map(|p| Some(STANDARD.encode(p))).collect();
        let html = reassemble_base64(&chunks).unwrap();
        assert_eq!(html, "<html><body>hi</body></html>");
    }

    #[test]
    fn reassemble_fails_on_missing_chunk() {
        let chunks = vec![Some("aGk=".to_string()), None];
        assert!(reassemble_base64(&chunks).is_err());
    }

    #[test]
    fn num_field_handles_numbers_and_strings() {
        assert_eq!(num_field(&json!({"n": 5}), "n"), Some(5));
        assert_eq!(num_field(&json!({"n": "12"}), "n"), Some(12));
        assert_eq!(num_field(&json!({"n": 3.9}), "n"), Some(3));
        assert_eq!(num_field(&json!({}), "n"), None);
    }
}
