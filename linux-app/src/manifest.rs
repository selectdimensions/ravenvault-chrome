//! Manifest builder: scan an Obsidian vault for exported Poe notes and emit a
//! slug/URL manifest (JSON + CSV + a plain URL list).
//!
//! This is a read-only companion to [`crate::vault`]: it parses the leading YAML
//! frontmatter of each top-level `*.md` note (using the same simple line-parsing
//! approach as `vault.rs`, deliberately *without* a YAML crate), extracts the
//! `title`, `source`, and `uid`, derives the Poe chat `slug` from the
//! `source: https://poe.com/chat/<slug>` URL, and writes the collected entries to
//! `<vault>/.ravenvault/`. The `chat-urls.txt` output is intended to feed a future
//! Playwright run.
//!
//! No new dependencies: CSV escaping is implemented inline (RFC 4180) and the slug
//! is parsed with plain `str` operations rather than a regex.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// One exported Poe chat discovered in the vault.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct ChatEntry {
    /// Human title from the note frontmatter.
    pub title: String,
    /// Poe chat slug parsed from the `source` URL (`/chat/<slug>`).
    pub slug: String,
    /// The full `source` URL, if present.
    pub url: String,
    /// Stable note id from the frontmatter `uid`.
    pub uid: String,
    /// The note's file name (e.g. `My Chat.md`).
    pub note: String,
}

/// Scan `vault` for top-level `*.md` notes and build a manifest of exported Poe
/// chats.
///
/// Each note's leading `---`…`---` YAML block is parsed for `title`, `source`, and
/// `uid`; the `slug` is derived from the `source` URL's `/chat/<slug>` segment.
/// Files without frontmatter, and notes with neither a source nor a slug, are
/// skipped. Entries are sorted by title (case-insensitive).
pub fn build_manifest(vault: &Path) -> Result<Vec<ChatEntry>> {
    let mut entries = Vec::new();

    let rd = fs::read_dir(vault)
        .with_context(|| format!("reading vault directory {}", vault.display()))?;
    for dent in rd.flatten() {
        let path = dent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Top-level only (read_dir doesn't recurse), regular files only.
        if !path.is_file() {
            continue;
        }

        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let Some(front) = parse_frontmatter(&contents) else {
            continue; // No frontmatter: skip.
        };

        let title = front.title.unwrap_or_default();
        let url = front.source.unwrap_or_default();
        let uid = front.uid.unwrap_or_default();
        let slug = slug_from_source(&url).unwrap_or_default();

        // Only collect entries that actually look like an exported chat.
        if url.is_empty() && slug.is_empty() {
            continue;
        }

        let note = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        entries.push(ChatEntry {
            title,
            slug,
            url,
            uid,
            note,
        });
    }

    entries.sort_by_key(|e| e.title.to_lowercase());
    Ok(entries)
}

/// Write the manifest in three formats under `<vault>/.ravenvault/`:
/// `chats-manifest.json`, `chats-manifest.csv`, and `chat-urls.txt`. Returns the
/// `.ravenvault` directory path.
pub fn write_manifest(vault: &Path, entries: &[ChatEntry]) -> Result<PathBuf> {
    let dir = vault.join(".ravenvault");
    fs::create_dir_all(&dir).with_context(|| format!("creating manifest dir {}", dir.display()))?;

    // 1. JSON (pretty).
    let json = serde_json::to_string_pretty(entries).context("serialize manifest JSON")?;
    let json_path = dir.join("chats-manifest.json");
    fs::write(&json_path, json).with_context(|| format!("writing {}", json_path.display()))?;

    // 2. CSV (RFC 4180 quoting via the inline escaper).
    let csv_path = dir.join("chats-manifest.csv");
    {
        let mut f = fs::File::create(&csv_path)
            .with_context(|| format!("writing {}", csv_path.display()))?;
        writeln!(f, "title,slug,url,uid,note")?;
        for e in entries {
            writeln!(
                f,
                "{},{},{},{},{}",
                csv_field(&e.title),
                csv_field(&e.slug),
                csv_field(&e.url),
                csv_field(&e.uid),
                csv_field(&e.note),
            )?;
        }
    }

    // 3. Plain URL list (skip empties), one per line, for Playwright.
    let urls_path = dir.join("chat-urls.txt");
    {
        let mut f = fs::File::create(&urls_path)
            .with_context(|| format!("writing {}", urls_path.display()))?;
        for e in entries {
            if !e.url.is_empty() {
                writeln!(f, "{}", e.url)?;
            }
        }
    }

    Ok(dir)
}

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

/// The subset of frontmatter fields the manifest cares about.
#[derive(Debug, Default)]
struct Frontmatter {
    title: Option<String>,
    source: Option<String>,
    uid: Option<String>,
}

/// Parse the leading `---`…`---` YAML block of a note. Returns `None` when the
/// file does not start with a frontmatter fence. Uses the same simple line
/// parsing as `vault.rs::read_uid` (no YAML crate).
fn parse_frontmatter(contents: &str) -> Option<Frontmatter> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        return None;
    }

    let mut front = Frontmatter::default();
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("title:") {
            front.title = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("source:") {
            front.source = Some(unquote_yaml_scalar(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("uid:") {
            front.uid = Some(unquote_yaml_scalar(rest.trim()));
        }
    }
    Some(front)
}

/// Reverse of `vault.rs::yaml_scalar`: strip surrounding double quotes and
/// unescape `\"`/`\\`.
fn unquote_yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

/// Derive the Poe chat slug from a `source` URL by finding the `/chat/` segment
/// and reading the following run of `[A-Za-z0-9_-]` characters. Returns `None`
/// when there is no `/chat/<slug>` segment. Plain string parsing — no regex.
fn slug_from_source(source: &str) -> Option<String> {
    let marker = "/chat/";
    let start = source.find(marker)? + marker.len();
    let slug: String = source[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if slug.is_empty() {
        None
    } else {
        Some(slug)
    }
}

// ---------------------------------------------------------------------------
// CSV escaping (RFC 4180)
// ---------------------------------------------------------------------------

/// Quote a CSV field per RFC 4180 when it contains a comma, double quote, or
/// newline (CR or LF); double quotes are escaped by doubling them.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A note with full frontmatter and a `/chat/<slug>` source URL.
    fn note_with_source(title: &str, uid: &str, slug: &str) -> String {
        format!(
            "---\ntitle: {title}\nuid: {uid}\nsource: https://poe.com/chat/{slug}\n\
             tags: [poe, ravenvault]\n---\n\nbody\n"
        )
    }

    #[test]
    fn slug_parsing() {
        assert_eq!(
            slug_from_source("https://poe.com/chat/abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            slug_from_source("https://poe.com/chat/Ab_3-x?ref=1").as_deref(),
            Some("Ab_3-x")
        );
        assert_eq!(slug_from_source("https://poe.com/somewhere"), None);
        assert_eq!(slug_from_source(""), None);
    }

    #[test]
    fn csv_quotes_special_fields() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("has,comma"), "\"has,comma\"");
        assert_eq!(csv_field("has\"quote"), "\"has\"\"quote\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn build_skips_files_without_frontmatter_and_parses_the_rest() {
        let tmp = tempdir().unwrap();
        let v = tmp.path();

        // Two valid notes (one title has a comma) + one without frontmatter.
        fs::write(
            v.join("Zebra.md"),
            note_with_source("Zebra chat", "uidz", "zzz999"),
        )
        .unwrap();
        fs::write(
            v.join("Apple.md"),
            note_with_source("Apple, banana", "uida", "aaa111"),
        )
        .unwrap();
        fs::write(v.join("plain.md"), "no frontmatter here\njust text\n").unwrap();

        let entries = build_manifest(v).unwrap();

        // Only the two notes with frontmatter+source are collected.
        assert_eq!(entries.len(), 2);

        // Sorted by title, case-insensitive: "Apple, banana" before "Zebra chat".
        assert_eq!(entries[0].title, "Apple, banana");
        assert_eq!(entries[1].title, "Zebra chat");

        // Slugs parsed correctly.
        assert_eq!(entries[0].slug, "aaa111");
        assert_eq!(entries[1].slug, "zzz999");

        // uid + url + note populated.
        assert_eq!(entries[0].uid, "uida");
        assert_eq!(entries[0].url, "https://poe.com/chat/aaa111");
        assert_eq!(entries[0].note, "Apple.md");
    }

    #[test]
    fn write_produces_all_three_files() {
        let tmp = tempdir().unwrap();
        let v = tmp.path();

        fs::write(
            v.join("Apple.md"),
            note_with_source("Apple, banana", "uida", "aaa111"),
        )
        .unwrap();
        fs::write(
            v.join("Zebra.md"),
            note_with_source("Zebra chat", "uidz", "zzz999"),
        )
        .unwrap();

        let entries = build_manifest(v).unwrap();
        let dir = write_manifest(v, &entries).unwrap();

        assert_eq!(dir, v.join(".ravenvault"));
        let json_path = dir.join("chats-manifest.json");
        let csv_path = dir.join("chats-manifest.csv");
        let urls_path = dir.join("chat-urls.txt");
        assert!(json_path.exists());
        assert!(csv_path.exists());
        assert!(urls_path.exists());

        // JSON round-trips back to the same entries.
        let raw = fs::read_to_string(&json_path).unwrap();
        let parsed: Vec<ChatEntry> = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed, entries);

        // CSV: header present, comma-containing title is quoted.
        let csv = fs::read_to_string(&csv_path).unwrap();
        let mut csv_lines = csv.lines();
        assert_eq!(csv_lines.next(), Some("title,slug,url,uid,note"));
        assert!(
            csv.contains("\"Apple, banana\","),
            "comma title should be quoted:\n{csv}"
        );

        // chat-urls.txt: one line per entry with a url.
        let urls = fs::read_to_string(&urls_path).unwrap();
        let url_count = urls.lines().filter(|l| !l.is_empty()).count();
        let expected = entries.iter().filter(|e| !e.url.is_empty()).count();
        assert_eq!(url_count, expected);
        assert_eq!(url_count, 2);
    }

    #[test]
    fn entry_without_frontmatter_uid_derivable_round_trip() {
        // A note that has a source but no uid/title still yields a slug entry.
        let tmp = tempdir().unwrap();
        let v = tmp.path();
        fs::write(
            v.join("bare.md"),
            "---\nsource: https://poe.com/chat/bare42\n---\nbody\n",
        )
        .unwrap();

        let entries = build_manifest(v).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "bare42");
        assert_eq!(entries[0].title, "");
        assert_eq!(entries[0].uid, "");
    }

    #[derive(serde::Deserialize)]
    struct ChatEntryOwned {
        slug: String,
    }

    #[test]
    fn json_field_names_stable() {
        // Guard against accidental rename of the serialized field names.
        let entries = vec![ChatEntry {
            title: "T".into(),
            slug: "s".into(),
            url: "u".into(),
            uid: "i".into(),
            note: "n.md".into(),
        }];
        let json = serde_json::to_string(&entries).unwrap();
        let back: Vec<ChatEntryOwned> = serde_json::from_str(&json).unwrap();
        assert_eq!(back[0].slug, "s");
    }
}
