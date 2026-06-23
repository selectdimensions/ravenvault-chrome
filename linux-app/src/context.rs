//! Shared application state: the configured vault and the single active export
//! session (the extension enforces one export at a time).
//!
//! Configuration is layered: a JSON file at `$XDG_CONFIG_HOME/ravenvault/config.json`
//! (default `~/.config/ravenvault/config.json`) provides defaults, and environment
//! variables override it. See [`AppContext::load`].

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

use crate::mempalace::MemPalaceConfig;

/// Live status of an export, surfaced to the extension via `get_session_status`.
/// Mirrors the string-typed fields the extension expects.
#[derive(Debug, Default, Clone)]
pub struct SessionStatus {
    pub active: bool,
    pub tab_id: i64,
    pub window_id: i64,
    pub status: String,
    pub current: u64,
    pub total: u64,
}

/// On-disk configuration (all optional; missing fields fall back to defaults).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    /// Path to the destination Obsidian vault.
    pub vault_path: Option<String>,
    /// Whether to ingest exports into MemPalace.
    pub mempalace_enabled: Option<bool>,
    /// Override for the `mempalace` executable.
    pub mempalace_bin: Option<String>,
}

/// Process-wide context shared across connections.
#[derive(Debug)]
pub struct AppContext {
    /// Destination Obsidian vault. `None` until configured (config file or the
    /// `RAVENVAULT_VAULT` environment variable).
    pub vault_path: Option<PathBuf>,
    /// Status of the current/last export.
    pub session: Mutex<SessionStatus>,
    /// Single-session guard: true while an export is running.
    pub busy: Mutex<bool>,
    /// MemPalace (LLM memory) ingest configuration.
    pub mempalace: MemPalaceConfig,
}

impl AppContext {
    pub fn new(vault_path: Option<PathBuf>) -> Self {
        AppContext {
            vault_path,
            session: Mutex::new(SessionStatus::default()),
            busy: Mutex::new(false),
            mempalace: MemPalaceConfig::default(),
        }
    }

    /// Build context from a resolved [`FileConfig`].
    pub fn from_file_config(fc: FileConfig) -> Self {
        let vault_path = fc.vault_path.filter(|s| !s.is_empty()).map(PathBuf::from);
        let mempalace = MemPalaceConfig {
            enabled: fc.mempalace_enabled.unwrap_or(false),
            binary: fc
                .mempalace_bin
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "mempalace".to_string()),
            ..MemPalaceConfig::default()
        };
        AppContext {
            vault_path,
            session: Mutex::new(SessionStatus::default()),
            busy: Mutex::new(false),
            mempalace,
        }
    }

    /// Load configuration: read the config file (if any), then apply environment
    /// overrides (`RAVENVAULT_VAULT`, `RAVENVAULT_MEMPALACE`,
    /// `RAVENVAULT_MEMPALACE_BIN`).
    pub fn load() -> Self {
        let mut fc = config_file_path()
            .and_then(read_file_config)
            .unwrap_or_default();

        if let Ok(v) = std::env::var("RAVENVAULT_VAULT") {
            if !v.is_empty() {
                fc.vault_path = Some(v);
            }
        }
        if let Ok(v) = std::env::var("RAVENVAULT_MEMPALACE") {
            fc.mempalace_enabled = Some(matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ));
        }
        if let Ok(v) = std::env::var("RAVENVAULT_MEMPALACE_BIN") {
            if !v.is_empty() {
                fc.mempalace_bin = Some(v);
            }
        }
        AppContext::from_file_config(fc)
    }

    /// Build context from the environment only (no config file). Retained for
    /// tests and minimal launches.
    pub fn from_env() -> Self {
        let vault = std::env::var("RAVENVAULT_VAULT")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        AppContext {
            vault_path: vault,
            session: Mutex::new(SessionStatus::default()),
            busy: Mutex::new(false),
            mempalace: MemPalaceConfig::from_env(),
        }
    }
}

/// The current on-disk config, or defaults if absent/unreadable.
pub fn current_file_config() -> FileConfig {
    config_file_path()
        .and_then(read_file_config)
        .unwrap_or_default()
}

/// Write the config file (creating `~/.config/ravenvault/` if needed), pretty.
pub fn save_file_config(fc: &FileConfig) -> Result<()> {
    let path = config_file_path().context("could not resolve config path (no HOME?)")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(fc).context("serialize config")?;
    std::fs::write(&path, json).with_context(|| format!("write config {}", path.display()))?;
    Ok(())
}

/// Resolve the config file path from `XDG_CONFIG_HOME` or `~/.config`.
pub fn config_file_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("ravenvault").join("config.json"))
}

/// Read and parse a config file, warning (but not failing) on malformed JSON.
fn read_file_config(path: PathBuf) -> Option<FileConfig> {
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(fc) => Some(fc),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "ignoring malformed config file");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_config_json() {
        let raw = r#"{"vault_path":"/home/me/Vault","mempalace_enabled":true}"#;
        let fc: FileConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(fc.vault_path.as_deref(), Some("/home/me/Vault"));
        assert_eq!(fc.mempalace_enabled, Some(true));
        assert!(fc.mempalace_bin.is_none());
    }

    #[test]
    fn empty_json_is_all_defaults() {
        let fc: FileConfig = serde_json::from_str("{}").unwrap();
        let ctx = AppContext::from_file_config(fc);
        assert!(ctx.vault_path.is_none());
        assert!(!ctx.mempalace.enabled);
        assert_eq!(ctx.mempalace.binary, "mempalace");
    }

    #[test]
    fn file_config_maps_into_context() {
        let fc = FileConfig {
            vault_path: Some("/v".to_string()),
            mempalace_enabled: Some(true),
            mempalace_bin: Some("/opt/mempalace".to_string()),
        };
        let ctx = AppContext::from_file_config(fc);
        assert_eq!(ctx.vault_path.as_deref(), Some(std::path::Path::new("/v")));
        assert!(ctx.mempalace.enabled);
        assert_eq!(ctx.mempalace.binary, "/opt/mempalace");
    }

    #[test]
    fn empty_strings_are_treated_as_unset() {
        let fc = FileConfig {
            vault_path: Some(String::new()),
            mempalace_enabled: None,
            mempalace_bin: Some(String::new()),
        };
        let ctx = AppContext::from_file_config(fc);
        assert!(ctx.vault_path.is_none());
        assert_eq!(ctx.mempalace.binary, "mempalace");
    }
}
