//! Obsidian vault writer (M4).
//!
//! Persists a converted Poe [`ExportNote`] (the output of [`crate::html2md`]) into
//! an Obsidian vault as a Markdown note plus content-hashed asset files. The
//! design goals, taken from `docs/LINUX_DEVELOPMENT_PLAN.md` (M4) and the Obsidian
//! guidance in `docs/PROTOCOL.md`:
//!
//! - **Self-contained vault:** notes and assets live *inside* the vault root;
//!   attachments go in a dedicated subfolder (default `attachments/`).
//! - **Stable identity:** YAML frontmatter carries a deterministic `uid` so
//!   re-exporting the same conversation **upserts** one file rather than spawning
//!   duplicates.
//! - **Content-hashed assets:** each asset filename is `<sha256-prefix>.<ext>`, so
//!   identical bytes dedupe naturally and links never collide.
//! - **Safe filenames:** titles are sanitized against the Obsidian/Windows
//!   forbidden set, reserved names, control chars, length, and Unicode form.
//! - **Atomic writes:** every file is written to a temp file in the same directory
//!   then `rename`d over the target, so a reader never sees a half-written note.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Length of the hex sha256 prefix used for asset filenames.
const ASSET_HASH_LEN: usize = 16;
/// Length of the hex sha256 prefix used for the note `uid`.
const UID_LEN: usize = 16;
/// Length of the uid suffix used to disambiguate same-title/different-uid notes.
const UID_DISAMBIG_LEN: usize = 8;
/// Maximum length, in bytes, of a sanitized note basename (before the extension).
const MAX_BASENAME_BYTES: usize = 200;
/// Default attachments subfolder name.
const DEFAULT_ATTACHMENTS_DIR: &str = "attachments";

/// One conversation to persist into the vault.
#[derive(Debug, Clone, Default)]
pub struct ExportNote {
    /// Human title (from html2md).
    pub title: String,
    /// Body markdown, asset refs as original URLs.
    pub markdown: String,
    /// Original URL -> downloaded bytes.
    pub assets: HashMap<String, Vec<u8>>,
    /// e.g. the poe.com chat URL.
    pub source_url: Option<String>,
    /// ISO-8601 date/time string, optional.
    pub created: Option<String>,
}

/// Writes notes + assets into an Obsidian vault.
#[derive(Debug, Clone)]
pub struct VaultWriter {
    /// Vault root directory; all notes and assets live under here.
    root: PathBuf,
    /// Attachments subfolder name, relative to `root`.
    attachments_dir: String,
}

impl VaultWriter {
    /// Create a writer rooted at `root`, with the default `attachments/` subdir.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            attachments_dir: DEFAULT_ATTACHMENTS_DIR.to_string(),
        }
    }

    /// Override the attachments subfolder name (relative to the vault root).
    pub fn with_attachments_dir(mut self, dir: &str) -> Self {
        self.attachments_dir = dir.to_string();
        self
    }

    /// Write the note (and its assets). Returns the path of the written `.md` file.
    ///
    /// Steps: write/dedup assets → rewrite the body's asset URLs to local relative
    /// paths → build YAML frontmatter with a stable `uid` → pick a deterministic,
    /// sanitized filename (disambiguating only on a real uid collision) → write the
    /// note atomically.
    pub fn write(&self, note: ExportNote) -> Result<PathBuf> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating vault root {}", self.root.display()))?;

        let uid = compute_uid(&note);

        // 1. Persist assets and build the URL -> relative-path rewrite map.
        let rewrites = self.write_assets(&note.assets)?;

        // 2. Rewrite the body markdown.
        let body = rewrite_markdown(&note.markdown, &rewrites);

        // 3. Frontmatter + body.
        let frontmatter = build_frontmatter(&note, &uid);
        let contents = format!("{frontmatter}\n{body}\n");

        // 4. Deterministic, idempotent target path.
        let target = self.resolve_note_path(&note.title, &uid)?;

        // 5. Atomic write.
        atomic_write(&target, contents.as_bytes())
            .with_context(|| format!("writing note {}", target.display()))?;

        Ok(target)
    }

    /// Write each asset under `<root>/<attachments_dir>/<hash>.<ext>`, deduping by
    /// content hash. Returns a map of original URL -> relative vault path
    /// (forward slashes), suitable for rewriting the markdown body.
    fn write_assets(&self, assets: &HashMap<String, Vec<u8>>) -> Result<HashMap<String, String>> {
        let mut rewrites = HashMap::with_capacity(assets.len());
        if assets.is_empty() {
            return Ok(rewrites);
        }

        let dir = self.root.join(&self.attachments_dir);
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating attachments dir {}", dir.display()))?;

        for (url, bytes) in assets {
            let hash = hex_prefix(bytes, ASSET_HASH_LEN);
            let ext = asset_extension(url, bytes);
            let filename = format!("{hash}.{ext}");
            let path = dir.join(&filename);

            // Identical bytes => identical filename; don't rewrite if present.
            if !path.exists() {
                atomic_write(&path, bytes)
                    .with_context(|| format!("writing asset {}", path.display()))?;
            }

            // Relative path with forward slashes (hashed names have no spaces).
            let rel = format!("{}/{}", self.attachments_dir, filename);
            rewrites.insert(url.clone(), rel);
        }

        Ok(rewrites)
    }

    /// Resolve the deterministic note path for `(title, uid)`.
    ///
    /// Uses the sanitized title as the basename. If a note with that name already
    /// exists with a *different* `uid`, append `-<uid8>` to disambiguate. Same uid
    /// (re-export) or no existing file => write to the plain path (overwriting on
    /// same uid), which is what makes the upsert idempotent.
    fn resolve_note_path(&self, title: &str, uid: &str) -> Result<PathBuf> {
        let base = sanitize_filename(title);
        let plain = self.root.join(format!("{base}.md"));

        match read_uid(&plain)? {
            // No existing note, or it belongs to this conversation: reuse the path.
            None => Ok(plain),
            Some(existing) if existing == uid => Ok(plain),
            // A different conversation already owns this title: disambiguate.
            Some(_) => {
                let suffix = &uid[..uid.len().min(UID_DISAMBIG_LEN)];
                Ok(self.root.join(format!("{base}-{suffix}.md")))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// uid + frontmatter
// ---------------------------------------------------------------------------

/// Stable note id: sha256 prefix of the source URL if present, else of the title.
fn compute_uid(note: &ExportNote) -> String {
    let seed = note
        .source_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&note.title);
    hex_prefix(seed.as_bytes(), UID_LEN)
}

/// Build the YAML frontmatter block (including the delimiting `---` lines).
fn build_frontmatter(note: &ExportNote, uid: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&yaml_kv("title", &note.title));
    out.push_str(&yaml_kv("uid", uid));
    if let Some(src) = note.source_url.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&yaml_kv("source", src));
    }
    if let Some(created) = note.created.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&yaml_kv("created", created));
    }
    // Tags as a flow list, WITHOUT `#`.
    out.push_str("tags: [poe, ravenvault]\n");
    out.push_str("---\n");
    out
}

/// Render a single `key: value` YAML line, quoting the value when needed.
fn yaml_kv(key: &str, value: &str) -> String {
    format!("{key}: {}\n", yaml_scalar(value))
}

/// Quote a YAML scalar if it contains a colon-space, a leading special char, a
/// `#`/`:`, or trailing/leading whitespace; otherwise emit it bare. Uses
/// double-quoting with minimal escaping (`\` and `"`).
fn yaml_scalar(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }

    let needs_quote = value.contains(": ")
        || value.contains(" #")
        || value.contains('"')
        || value.contains('\n')
        || value.ends_with(':')
        || value != value.trim()
        || starts_with_yaml_special(value);

    if needs_quote {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

/// Whether the first char of a scalar forces quoting under YAML 1.1 plain-scalar
/// rules (indicators / ambiguous starts).
fn starts_with_yaml_special(value: &str) -> bool {
    match value.chars().next() {
        Some(c) => matches!(
            c,
            '#' | '!'
                | '&'
                | '*'
                | '?'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
                | '"'
                | '\''
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ':'
                | '-'
                | ' '
        ),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Markdown rewrite
// ---------------------------------------------------------------------------

/// Replace every occurrence of each asset URL in `markdown` with its local
/// relative path. URLs not in the map are left untouched. Longest URLs are
/// replaced first so a URL that is a prefix of another can't corrupt the result.
fn rewrite_markdown(markdown: &str, rewrites: &HashMap<String, String>) -> String {
    if rewrites.is_empty() {
        return markdown.to_string();
    }
    let mut pairs: Vec<(&String, &String)> = rewrites.iter().collect();
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));

    let mut out = markdown.to_string();
    for (url, rel) in pairs {
        out = out.replace(url.as_str(), rel.as_str());
    }
    out
}

// ---------------------------------------------------------------------------
// Asset hashing + extension
// ---------------------------------------------------------------------------

/// Lowercase hex of the sha256 of `bytes`, truncated to `len` hex chars.
fn hex_prefix(bytes: &[u8], len: usize) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(len);
    s
}

/// Derive a file extension for an asset. Prefer the URL path's extension (when it
/// is a short, sane, alphanumeric token); else sniff common image magic bytes;
/// else fall back to `bin`.
fn asset_extension(url: &str, bytes: &[u8]) -> String {
    if let Some(ext) = extension_from_url(url) {
        return ext;
    }
    sniff_extension(bytes).unwrap_or("bin").to_string()
}

/// Extract a plausible extension from a URL path component (ignoring query/frag).
fn extension_from_url(url: &str) -> Option<String> {
    // Strip query and fragment, then take the last path segment.
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or(path);
    let (_, ext) = last.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    let ok = !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then_some(ext)
}

/// Sniff a small set of common image formats by magic bytes.
fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Filename sanitization
// ---------------------------------------------------------------------------

/// Forbidden characters under Obsidian/Windows: `[ ] # ^ | \ / : * " < > ?`.
const FORBIDDEN: &[char] = &[
    '[', ']', '#', '^', '|', '\\', '/', ':', '*', '"', '<', '>', '?',
];

/// Windows reserved device basenames (case-insensitive) that must be neutralized.
fn is_reserved_basename(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    // COM1-9 / LPT1-9.
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            if rest.len() == 1 && matches!(rest.as_bytes()[0], b'1'..=b'9') {
                return true;
            }
        }
    }
    false
}

/// Sanitize a human title into a safe note basename (no extension).
///
/// Replaces the forbidden set + ASCII control chars with spaces, strips a leading
/// dot, NFC-normalizes, collapses whitespace to single spaces, trims, neutralizes
/// Windows reserved names by suffixing `_`, caps the basename to ~200 bytes (never
/// splitting a UTF-8 char), trims trailing dots/spaces, and falls back to
/// `"Untitled"` if the result is empty.
pub fn sanitize_filename(title: &str) -> String {
    // Replace forbidden + control chars with a space (so words don't fuse).
    let replaced: String = title
        .chars()
        .map(|c| {
            if FORBIDDEN.contains(&c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();

    // NFC-normalize.
    let normalized: String = replaced.nfc().collect();

    // Collapse all whitespace runs to single spaces and trim.
    let mut collapsed = String::with_capacity(normalized.len());
    let mut prev_space = false;
    for c in normalized.chars() {
        if c.is_whitespace() {
            if !prev_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    let mut name = collapsed.trim().to_string();

    // Strip a leading dot (hidden-file guard).
    while name.starts_with('.') {
        name = name[1..].trim_start().to_string();
    }

    // Cap to MAX_BASENAME_BYTES without splitting a UTF-8 char.
    if name.len() > MAX_BASENAME_BYTES {
        let mut end = MAX_BASENAME_BYTES;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }

    // Trim trailing dots/spaces (Windows strips these from filenames).
    name = name.trim_end_matches(['.', ' ']).to_string();
    // Re-trim leading whitespace that truncation/strip may have exposed.
    name = name.trim().to_string();

    if name.is_empty() {
        return "Untitled".to_string();
    }

    // Neutralize Windows reserved device names.
    if is_reserved_basename(&name) {
        name.push('_');
    }

    name
}

// ---------------------------------------------------------------------------
// Frontmatter read-back (for idempotent upsert)
// ---------------------------------------------------------------------------

/// Read the `uid` from the YAML frontmatter of an existing note, if any.
/// Returns `Ok(None)` if the file does not exist or has no parseable `uid`.
fn read_uid(path: &Path) -> Result<Option<String>> {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };

    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        return Ok(None);
    }
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("uid:") {
            let val = unquote_yaml_scalar(rest.trim());
            if !val.is_empty() {
                return Ok(Some(val));
            }
        }
    }
    Ok(None)
}

/// Reverse [`yaml_scalar`]: strip surrounding double quotes and unescape `\"`/`\\`.
fn unquote_yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Atomic write
// ---------------------------------------------------------------------------

/// Write `data` to `target` atomically: a temp file in the *same* directory is
/// fully written + flushed, then `rename`d over the target. On any failure the
/// temp file is removed so no leftover artifacts remain.
fn atomic_write(target: &Path, data: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .context("target path has no parent directory")?;
    fs::create_dir_all(dir)?;

    // Unique temp name in the same dir (so rename is atomic on the same fs).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn count_md_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .count()
    }

    fn list_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn write_creates_md_with_frontmatter_and_body() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());
        let note = ExportNote {
            title: "Rust async patterns".to_string(),
            markdown: "### 🧑 You\n\nHow do I do async?".to_string(),
            assets: HashMap::new(),
            source_url: Some("https://poe.com/chat/abc123".to_string()),
            created: Some("2026-06-22T10:00:00Z".to_string()),
        };
        let path = w.write(note).unwrap();

        assert_eq!(path, tmp.path().join("Rust async patterns.md"));
        let contents = fs::read_to_string(&path).unwrap();

        assert!(contents.starts_with("---\n"));
        assert!(contents.contains("title: Rust async patterns\n"));
        assert!(contents.contains("uid: "));
        assert!(contents.contains("source: https://poe.com/chat/abc123\n"));
        assert!(
            contents.contains("created: 2026-06-22T10:00:00Z\n")
                || contents.contains("created: \"2026-06-22T10:00:00Z\"\n")
        );
        assert!(contents.contains("tags: [poe, ravenvault]\n"));
        // Body follows the closing fence after a blank line.
        assert!(contents.contains("---\n\n### 🧑 You"));
        assert!(contents.contains("How do I do async?"));
    }

    #[test]
    fn assets_written_with_hashed_names_and_body_rewritten() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());

        let url = "https://qph.cf2.poecdn.net/main-thumb-diagram.png";
        // Minimal valid PNG magic header.
        let bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let mut assets = HashMap::new();
        assets.insert(url.to_string(), bytes.clone());

        let note = ExportNote {
            title: "With image".to_string(),
            markdown: format!("Here is a pic:\n\n![diagram]({url})"),
            assets,
            source_url: None,
            created: None,
        };
        let path = w.write(note).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        // Original URL must be gone from the body.
        assert!(!contents.contains(url), "original URL should be rewritten");
        // Rewritten to the attachments-relative path.
        let hash = hex_prefix(&bytes, ASSET_HASH_LEN);
        let expected_rel = format!("attachments/{hash}.png");
        assert!(
            contents.contains(&format!("![diagram]({expected_rel})")),
            "body not rewritten:\n{contents}"
        );

        // Asset file exists on disk.
        let asset_path = tmp.path().join("attachments").join(format!("{hash}.png"));
        assert!(asset_path.exists());
        assert_eq!(fs::read(&asset_path).unwrap(), bytes);
    }

    #[test]
    fn identical_bytes_dedupe_to_single_file() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());

        let bytes = vec![0xFF, 0xD8, 0xFF, 9, 8, 7]; // jpg magic
        let mut assets = HashMap::new();
        assets.insert("https://x.poecdn.net/a.jpg".to_string(), bytes.clone());
        assets.insert("https://x.poecdn.net/b.jpg".to_string(), bytes.clone());

        let note = ExportNote {
            title: "Dedup".to_string(),
            markdown: "![a](https://x.poecdn.net/a.jpg) ![b](https://x.poecdn.net/b.jpg)"
                .to_string(),
            assets,
            source_url: None,
            created: None,
        };
        let path = w.write(note).unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        let attach = tmp.path().join("attachments");
        let files = list_files(&attach);
        assert_eq!(files.len(), 1, "expected one deduped asset, got {files:?}");

        // Both refs point at the same single file.
        let hash = hex_prefix(&bytes, ASSET_HASH_LEN);
        let rel = format!("attachments/{hash}.jpg");
        assert_eq!(contents.matches(&rel).count(), 2);
    }

    #[test]
    fn sanitize_forbidden_chars() {
        let out = sanitize_filename(r#"a[b]c#d^e|f:g*h"i<j>k?l"#);
        for c in FORBIDDEN {
            assert!(!out.contains(*c), "forbidden char {c} survived in {out:?}");
        }
    }

    #[test]
    fn sanitize_path_separators_dont_survive() {
        let out = sanitize_filename("foo/bar\\baz");
        assert!(!out.contains('/'));
        assert!(!out.contains('\\'));
        assert_eq!(out, "foo bar baz");
    }

    #[test]
    fn sanitize_leading_dot() {
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("...dots"), "dots");
    }

    #[test]
    fn sanitize_reserved_name() {
        assert_eq!(sanitize_filename("CON"), "CON_");
        assert_eq!(sanitize_filename("con"), "con_");
        assert_eq!(sanitize_filename("COM1"), "COM1_");
        assert_eq!(sanitize_filename("LPT9"), "LPT9_");
        // Not reserved: extra chars.
        assert_eq!(sanitize_filename("CONTENT"), "CONTENT");
    }

    #[test]
    fn sanitize_overlong_title_capped() {
        let long = "x".repeat(500);
        let out = sanitize_filename(&long);
        assert!(out.len() <= MAX_BASENAME_BYTES);
        assert!(!out.is_empty());
    }

    #[test]
    fn sanitize_overlong_unicode_no_split() {
        // Multi-byte chars; ensure truncation never splits one.
        let long = "é".repeat(300); // 2 bytes each
        let out = sanitize_filename(&long);
        assert!(out.len() <= MAX_BASENAME_BYTES);
        // Must still be valid UTF-8 (guaranteed by String) and non-empty.
        assert!(!out.is_empty());
    }

    #[test]
    fn sanitize_empty_falls_back() {
        assert_eq!(sanitize_filename(""), "Untitled");
        assert_eq!(sanitize_filename("   "), "Untitled");
        assert_eq!(sanitize_filename("///"), "Untitled");
    }

    #[test]
    fn sanitize_unicode_title_preserved() {
        let out = sanitize_filename("Café — Über Straße 日本語");
        assert!(out.contains("Café"));
        assert!(out.contains("日本語"));
        assert!(out.contains("Über"));
    }

    #[test]
    fn idempotent_same_note_twice_one_file() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());
        let make = || ExportNote {
            title: "Repeatable".to_string(),
            markdown: "body".to_string(),
            assets: HashMap::new(),
            source_url: Some("https://poe.com/chat/same".to_string()),
            created: None,
        };

        let p1 = w.write(make()).unwrap();
        let p2 = w.write(make()).unwrap();

        assert_eq!(p1, p2);
        assert_eq!(count_md_files(tmp.path()), 1);
    }

    #[test]
    fn same_title_different_conversations_two_files() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());

        let a = ExportNote {
            title: "Shared Title".to_string(),
            markdown: "first".to_string(),
            assets: HashMap::new(),
            source_url: Some("https://poe.com/chat/one".to_string()),
            created: None,
        };
        let b = ExportNote {
            title: "Shared Title".to_string(),
            markdown: "second".to_string(),
            assets: HashMap::new(),
            source_url: Some("https://poe.com/chat/two".to_string()),
            created: None,
        };

        let pa = w.write(a).unwrap();
        let pb = w.write(b).unwrap();

        assert_ne!(pa, pb);
        assert_eq!(count_md_files(tmp.path()), 2);
        assert_eq!(pa, tmp.path().join("Shared Title.md"));
    }

    #[test]
    fn no_leftover_temp_files() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());

        let mut assets = HashMap::new();
        assets.insert(
            "https://x.poecdn.net/z.png".to_string(),
            vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        );
        let note = ExportNote {
            title: "Temp check".to_string(),
            markdown: "![z](https://x.poecdn.net/z.png)".to_string(),
            assets,
            source_url: None,
            created: None,
        };
        w.write(note).unwrap();

        // No .tmp leftovers anywhere in the vault.
        for entry in fs::read_dir(tmp.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.ends_with(".tmp"), "leftover temp at root: {name}");
        }
        for entry in fs::read_dir(tmp.path().join("attachments")).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.ends_with(".tmp"),
                "leftover temp in attachments: {name}"
            );
        }
    }

    #[test]
    fn frontmatter_quotes_colon_space_values() {
        let tmp = tempdir().unwrap();
        let w = VaultWriter::new(tmp.path());
        let note = ExportNote {
            title: "Title: with colon space".to_string(),
            markdown: "x".to_string(),
            source_url: None,
            created: None,
            assets: HashMap::new(),
        };
        let path = w.write(note).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("title: \"Title: with colon space\"\n"));
    }

    #[test]
    fn uid_stable_from_source_url() {
        let n1 = ExportNote {
            title: "A".to_string(),
            source_url: Some("https://poe.com/chat/x".to_string()),
            ..Default::default()
        };
        let n2 = ExportNote {
            title: "Totally different title".to_string(),
            source_url: Some("https://poe.com/chat/x".to_string()),
            ..Default::default()
        };
        // Same source => same uid regardless of title.
        assert_eq!(compute_uid(&n1), compute_uid(&n2));

        // No source => uid from title.
        let n3 = ExportNote {
            title: "A".to_string(),
            ..Default::default()
        };
        assert_eq!(compute_uid(&n3), hex_prefix(b"A", UID_LEN));
    }

    #[test]
    fn extension_derivation() {
        // From URL path.
        assert_eq!(asset_extension("https://x/y/a.PNG?w=1", &[]), "png");
        assert_eq!(asset_extension("https://x/y/a.jpeg", &[]), "jpeg");
        // No usable URL ext -> sniff.
        assert_eq!(
            asset_extension(
                "https://x/y/blob",
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            ),
            "png"
        );
        assert_eq!(
            asset_extension("https://x/y/blob", &[0xFF, 0xD8, 0xFF]),
            "jpg"
        );
        // Unknown -> bin.
        assert_eq!(asset_extension("https://x/y/blob", &[0, 1, 2, 3]), "bin");
        // Implausible "extension" (too long / non-alnum) -> sniff/fallback.
        assert_eq!(
            asset_extension("https://x/y/file.somelongthing", &[0, 1]),
            "bin"
        );
    }
}
