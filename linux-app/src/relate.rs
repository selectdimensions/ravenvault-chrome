//! `relate`: link semantically-related notes in the vault.
//!
//! For every top-level note in the vault, query MemPalace (via the persistent MCP
//! server, see [`crate::mcp`]) for the most semantically-related *other* notes, and
//! append an idempotent `## Related` section of Obsidian `[[wikilinks]]` to the end
//! of the note. Re-running replaces the previously generated block rather than
//! duplicating it.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use crate::mcp::McpClient;

/// Summary of a `relate` run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelateSummary {
    /// Total notes processed.
    pub notes: usize,
    /// Notes that received at least one related link.
    pub linked: usize,
    /// Total number of related links written across all notes.
    pub total_links: usize,
}

/// One note discovered in the vault.
struct Note {
    path: PathBuf,
    /// File name including `.md` (the wikilink/basename key).
    filename: String,
    /// Frontmatter title, or the file stem when absent.
    title: String,
}

/// Link related notes throughout `vault`.
pub async fn run(
    vault: &Path,
    wing: Option<&str>,
    limit_links: usize,
    min_similarity: f64,
    mcp_bin: &str,
) -> Result<RelateSummary> {
    // 1. Scan top-level *.md notes.
    let notes = scan_notes(vault)?;
    let by_basename: HashMap<String, ()> = notes.iter().map(|n| (n.filename.clone(), ())).collect();

    // 2. Start the MCP client + handshake once.
    let mut client = McpClient::spawn(mcp_bin)
        .await
        .context("starting MemPalace MCP server")?;

    let mut summary = RelateSummary {
        notes: notes.len(),
        ..Default::default()
    };

    // 3. Query each note and write its Related block.
    for (idx, note) in notes.iter().enumerate() {
        let contents = match fs::read_to_string(&note.path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let query = select_query(&note.title, &contents);

        let hits = client
            .search(&query, wing, (limit_links + 6) as u32)
            .await
            .with_context(|| format!("searching for {}", note.filename))?;

        let mut targets: Vec<String> = Vec::new();
        for hit in &hits {
            let Some(base) = Path::new(&hit.source_file)
                .file_name()
                .and_then(|n| n.to_str())
            else {
                continue;
            };
            if base == note.filename {
                continue; // self
            }
            if hit.similarity < min_similarity {
                continue; // too weak
            }
            if !by_basename.contains_key(base) {
                continue; // not a note in this vault
            }
            let target = base.strip_suffix(".md").unwrap_or(base).to_string();
            if !targets.contains(&target) {
                targets.push(target);
            }
            if targets.len() >= limit_links {
                break;
            }
        }

        if !targets.is_empty() {
            let updated = set_related_block(&contents, &targets);
            atomic_write(&note.path, updated.as_bytes())
                .with_context(|| format!("writing {}", note.path.display()))?;
            summary.linked += 1;
            summary.total_links += targets.len();
        }

        if (idx + 1) % 50 == 0 {
            info!(processed = idx + 1, total = notes.len(), "relate progress");
        }
    }

    client.shutdown().await;
    Ok(summary)
}

/// Scan the vault root for top-level `*.md` notes (regular files only).
fn scan_notes(vault: &Path) -> Result<Vec<Note>> {
    let mut notes = Vec::new();
    let rd = fs::read_dir(vault)
        .with_context(|| format!("reading vault directory {}", vault.display()))?;
    for dent in rd.flatten() {
        let path = dent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let stem = filename
            .strip_suffix(".md")
            .unwrap_or(&filename)
            .to_string();
        let title = fs::read_to_string(&path)
            .ok()
            .and_then(|c| parse_title(&c))
            .filter(|t| !t.is_empty())
            .unwrap_or(stem);
        notes.push(Note {
            path,
            filename,
            title,
        });
    }
    notes.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(notes)
}

/// Parse the `title:` field from a note's leading frontmatter, mirroring the simple
/// line parsing in `manifest.rs`/`vault.rs` (no YAML crate).
fn parse_title(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("title:") {
            return Some(unquote_yaml_scalar(rest.trim()));
        }
    }
    None
}

/// Reverse of `vault.rs::yaml_scalar`: strip surrounding double quotes / unescape.
fn unquote_yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

/// Choose the search query for a note: prefer the title, but fall back to the body
/// when the title is empty, a placeholder, or too short to be meaningful.
fn select_query(title: &str, contents: &str) -> String {
    let t = title.trim();
    let lower = t.to_lowercase();
    let weak = t.is_empty() || lower == "new chat" || lower == "untitled" || t.chars().count() < 6;
    let query = if weak {
        body_excerpt(contents)
    } else {
        t.to_string()
    };
    query.chars().take(250).collect()
}

/// The first ~200 chars of the note body (text after the closing frontmatter
/// `---`), with whitespace collapsed to single spaces.
fn body_excerpt(contents: &str) -> String {
    let body = strip_frontmatter(contents);
    let collapsed: String = {
        let mut out = String::new();
        let mut prev_space = false;
        for c in body.chars() {
            if c.is_whitespace() {
                if !prev_space && !out.is_empty() {
                    out.push(' ');
                }
                prev_space = true;
            } else {
                out.push(c);
                prev_space = false;
            }
        }
        out.trim().to_string()
    };
    collapsed.chars().take(200).collect()
}

/// Return the body of a note, skipping a leading `---`…`---` frontmatter block.
fn strip_frontmatter(contents: &str) -> &str {
    if !contents.starts_with("---") {
        return contents;
    }
    // Find the closing fence line after the first line.
    let mut offset = 0;
    let mut first = true;
    for line in contents.split_inclusive('\n') {
        if first {
            // Skip the opening `---` line.
            offset += line.len();
            first = false;
            continue;
        }
        if line.trim_end_matches(['\r', '\n']) == "---" {
            offset += line.len();
            return &contents[offset..];
        }
        offset += line.len();
    }
    contents
}

/// Marker for the start of our generated section.
const RELATED_HEADING: &str = "## Related";

/// Insert or replace the `## Related` section at the END of `contents`. The block
/// is `## Related`, a blank line, then `- [[Target]]` per target. Idempotent: any
/// existing `## Related` section (which we always place last) is stripped first.
/// Preserves a single trailing newline.
fn set_related_block(contents: &str, targets: &[String]) -> String {
    // Strip a previously-generated Related section: from a line that is exactly
    // `## Related` to EOF (we always append it last).
    let mut base = contents.to_string();
    if let Some(pos) = find_related_heading(&base) {
        base.truncate(pos);
    }
    // Normalize trailing whitespace to exactly one newline before appending.
    let trimmed = base.trim_end_matches(['\n', '\r', ' ', '\t']);
    let mut out = String::with_capacity(trimmed.len() + 32 + targets.len() * 16);
    out.push_str(trimmed);
    out.push_str("\n\n");
    out.push_str(RELATED_HEADING);
    out.push('\n');
    out.push('\n');
    for t in targets {
        out.push_str(&format!("- [[{t}]]\n"));
    }
    out
}

/// Byte offset of a line equal to `## Related`, or `None`.
fn find_related_heading(contents: &str) -> Option<usize> {
    let mut offset = 0;
    for line in contents.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == RELATED_HEADING {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Write `data` to `target` atomically (temp file in same dir + rename), mirroring
/// `vault.rs::atomic_write`.
fn atomic_write(target: &Path, data: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .context("target path has no parent directory")?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("note");
    let unique = format!(".{file_name}.{}.tmp", std::process::id());
    let tmp = dir.join(unique);

    let write_result = (|| -> Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_related_block_when_absent() {
        let note = "---\ntitle: A\n---\n\nbody text\n";
        let out = set_related_block(note, &["Beta".to_string(), "Gamma".to_string()]);
        assert!(out.contains("## Related"));
        assert!(out.contains("- [[Beta]]"));
        assert!(out.contains("- [[Gamma]]"));
        assert!(out.ends_with("- [[Gamma]]\n"));
        // Body preserved.
        assert!(out.contains("body text"));
    }

    #[test]
    fn replacing_does_not_duplicate() {
        let note = "---\ntitle: A\n---\n\nbody\n";
        let once = set_related_block(note, &["Beta".to_string()]);
        let twice = set_related_block(&once, &["Gamma".to_string()]);
        // Exactly one heading, and the old link is gone.
        assert_eq!(twice.matches("## Related").count(), 1);
        assert!(!twice.contains("[[Beta]]"));
        assert!(twice.contains("[[Gamma]]"));
    }

    #[test]
    fn idempotent_same_targets() {
        let note = "---\ntitle: A\n---\n\nbody\n";
        let once = set_related_block(note, &["Beta".to_string()]);
        let twice = set_related_block(&once, &["Beta".to_string()]);
        assert_eq!(once, twice);
    }

    #[test]
    fn query_prefers_title_when_strong() {
        let note = "---\ntitle: Rust async patterns\n---\n\nsome body content here\n";
        assert_eq!(
            select_query("Rust async patterns", note),
            "Rust async patterns"
        );
    }

    #[test]
    fn query_falls_back_to_body_for_weak_title() {
        let note = "---\ntitle: New chat\n---\n\nDiscussing tokio runtimes and futures.\n";
        let q = select_query("New chat", note);
        assert!(q.starts_with("Discussing tokio runtimes"), "got: {q}");

        // Short title also falls back.
        let q2 = select_query("Hi", note);
        assert!(q2.starts_with("Discussing tokio"));
    }

    #[test]
    fn strip_frontmatter_returns_body() {
        let note = "---\ntitle: A\nuid: x\n---\nhello world\n";
        assert_eq!(strip_frontmatter(note), "hello world\n");
        // No frontmatter: whole thing.
        assert_eq!(strip_frontmatter("just text\n"), "just text\n");
    }
}
