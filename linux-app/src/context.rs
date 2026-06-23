//! Shared application state: the configured vault and the single active export
//! session (the extension enforces one export at a time).

use std::path::PathBuf;

use tokio::sync::Mutex;

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

/// Process-wide context shared across connections.
#[derive(Debug)]
pub struct AppContext {
    /// Destination Obsidian vault. `None` until configured (M6 settings UI; for
    /// now via the `RAVENVAULT_VAULT` environment variable).
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

    /// Build context from the environment. `RAVENVAULT_VAULT` sets the vault path;
    /// `RAVENVAULT_MEMPALACE` enables MemPalace ingest.
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
