//! MemPalace (LLM long-term memory) integration.
//!
//! After an export is written to the vault, we optionally feed the conversation
//! into the user's local MemPalace so it becomes agent-queryable memory. MemPalace
//! ingests via its CLI: `mempalace mine <dir> --mode convos`. Mining is
//! idempotent, so re-mining the vault only files new/changed conversations.
//!
//! This is strictly best-effort: a missing binary or a mine failure must NEVER
//! fail the export itself.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tracing::{debug, info, warn};

/// How long to allow a mine to run before giving up. Manual mines of a large
/// vault (especially the first one) can take a while.
const MINE_TIMEOUT: Duration = Duration::from_secs(3600);

/// Configuration for the MemPalace integration.
#[derive(Debug, Clone)]
pub struct MemPalaceConfig {
    /// Whether to ingest exports into MemPalace at all.
    pub enabled: bool,
    /// The `mempalace` executable (name on PATH or absolute path).
    pub binary: String,
    /// Mine mode; `convos` is for chat exports.
    pub mode: String,
    /// Recorded on every drawer.
    pub agent: String,
    /// Override the palace location; `None` uses MemPalace's own default
    /// (`~/.mempalace`).
    pub palace: Option<PathBuf>,
}

impl Default for MemPalaceConfig {
    fn default() -> Self {
        MemPalaceConfig {
            enabled: false,
            binary: "mempalace".to_string(),
            mode: "convos".to_string(),
            agent: "ravenvault".to_string(),
            palace: None,
        }
    }
}

impl MemPalaceConfig {
    /// Build from the environment. `RAVENVAULT_MEMPALACE` (1/true/yes) enables it;
    /// `RAVENVAULT_MEMPALACE_BIN` overrides the executable path.
    pub fn from_env() -> Self {
        let enabled = std::env::var("RAVENVAULT_MEMPALACE")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let mut cfg = MemPalaceConfig {
            enabled,
            ..Default::default()
        };
        if let Ok(bin) = std::env::var("RAVENVAULT_MEMPALACE_BIN") {
            if !bin.is_empty() {
                cfg.binary = bin;
            }
        }
        cfg
    }

    /// The full argument vector passed to the `mempalace` binary to mine `dir`.
    /// Global options (`--palace`) must precede the `mine` subcommand.
    pub fn mine_args(&self, dir: &Path) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(p) = &self.palace {
            args.push("--palace".to_string());
            args.push(p.display().to_string());
        }
        args.push("mine".to_string());
        args.push(dir.display().to_string());
        args.push("--mode".to_string());
        args.push(self.mode.clone());
        args.push("--agent".to_string());
        args.push(self.agent.clone());
        args
    }
}

/// Outcome of an ingest attempt.
#[derive(Debug, Clone)]
pub struct Ingest {
    /// False when the integration was disabled (nothing was run).
    pub ran: bool,
    pub stdout: String,
}

/// Mine `dir` into MemPalace. Returns `Ok(Ingest{ran:false})` immediately when
/// disabled. Errors are returned (the caller logs and ignores them so the export
/// still succeeds).
pub async fn ingest(config: &MemPalaceConfig, dir: &Path) -> Result<Ingest> {
    if !config.enabled {
        debug!("MemPalace integration disabled; skipping ingest");
        return Ok(Ingest {
            ran: false,
            stdout: String::new(),
        });
    }

    let args = config.mine_args(dir);
    info!(binary = %config.binary, dir = %dir.display(), "ingesting into MemPalace");

    let fut = tokio::process::Command::new(&config.binary)
        .args(&args)
        .kill_on_drop(true)
        .output();

    let output: Output = tokio::time::timeout(MINE_TIMEOUT, fut)
        .await
        .map_err(|_| anyhow!("mempalace mine timed out after {MINE_TIMEOUT:?}"))?
        .with_context(|| format!("failed to run `{}` (is it installed?)", config.binary))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "mempalace mine exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    debug!(%stdout, "MemPalace ingest complete");
    Ok(Ingest { ran: true, stdout })
}

/// Convenience wrapper: ingest, logging any failure without propagating it.
pub async fn ingest_best_effort(config: &MemPalaceConfig, dir: &Path) {
    match ingest(config, dir).await {
        Ok(i) if i.ran => info!("export ingested into MemPalace"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "MemPalace ingest failed (export still saved)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mine_args_basic() {
        let cfg = MemPalaceConfig {
            enabled: true,
            ..Default::default()
        };
        let args = cfg.mine_args(Path::new("/vault"));
        assert_eq!(
            args,
            vec![
                "mine",
                "/vault",
                "--mode",
                "convos",
                "--agent",
                "ravenvault"
            ]
        );
    }

    #[test]
    fn mine_args_with_palace_prefixes_global_option() {
        let cfg = MemPalaceConfig {
            enabled: true,
            palace: Some(PathBuf::from("/custom/palace")),
            ..Default::default()
        };
        let args = cfg.mine_args(Path::new("/vault"));
        assert_eq!(args[0], "--palace");
        assert_eq!(args[1], "/custom/palace");
        assert_eq!(args[2], "mine");
    }

    #[tokio::test]
    async fn disabled_config_skips_without_running() {
        let cfg = MemPalaceConfig::default(); // enabled = false
        let res = ingest(&cfg, Path::new("/nonexistent")).await.unwrap();
        assert!(!res.ran);
    }

    #[tokio::test]
    async fn runs_a_fake_binary_successfully() {
        // A stand-in `mempalace` that just echoes its args and succeeds — proves
        // the subprocess path works without touching the real palace.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-mempalace");
        std::fs::write(&fake, "#!/bin/sh\necho \"mined $*\"\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }

        let cfg = MemPalaceConfig {
            enabled: true,
            binary: fake.display().to_string(),
            ..Default::default()
        };
        let res = ingest(&cfg, dir.path()).await.unwrap();
        assert!(res.ran);
        assert!(res.stdout.contains("mined"));
        assert!(res.stdout.contains("--mode convos"));
    }

    #[tokio::test]
    async fn reports_error_for_missing_binary() {
        let cfg = MemPalaceConfig {
            enabled: true,
            binary: "/definitely/not/a/real/mempalace/binary".to_string(),
            ..Default::default()
        };
        assert!(ingest(&cfg, Path::new("/tmp")).await.is_err());
    }

    #[tokio::test]
    async fn fake_binary_failure_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fail-mempalace");
        std::fs::write(&fake, "#!/bin/sh\necho oops 1>&2\nexit 3\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake, perms).unwrap();
        }
        let cfg = MemPalaceConfig {
            enabled: true,
            binary: fake.display().to_string(),
            ..Default::default()
        };
        let err = ingest(&cfg, dir.path()).await.unwrap_err();
        assert!(err.to_string().contains("oops"));
    }
}
