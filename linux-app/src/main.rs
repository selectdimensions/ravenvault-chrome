//! Poe2Obsidian Linux companion app — headless binary entry point.
//!
//! Usage:
//!   ravenvault            Run the WebSocket server the extension connects to.
//!   ravenvault ingest [DIR]
//!                         Ingest a folder (default: the configured vault) into
//!                         MemPalace, then exit. Ingest is manual — never run
//!                         automatically during an export.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use ravenvault::context::AppContext;
use ravenvault::WS_BIND_ADDR;
use ravenvault::{manifest, mempalace, server};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let mut args = std::env::args().skip(1);
    if let Some(cmd) = args.next() {
        match cmd.as_str() {
            "ingest" => return run_ingest(args.next()).await,
            "manifest" => return run_manifest(args.next()).await,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("unknown command: {other}\n");
                print_help();
                std::process::exit(2);
            }
        }
    }

    run_server().await
}

/// Run the WebSocket server until Ctrl-C.
async fn run_server() -> Result<()> {
    let ctx = Arc::new(AppContext::load());
    match &ctx.vault_path {
        Some(p) => info!(vault = %p.display(), "vault configured"),
        None => warn!("no vault configured; set RAVENVAULT_VAULT or edit config.json"),
    }
    info!(
        version = ravenvault::APP_VERSION,
        bind = WS_BIND_ADDR,
        "Poe2Obsidian Linux companion starting"
    );
    println!(
        "Poe2Obsidian Linux companion v{} — WebSocket server on ws://{}",
        ravenvault::APP_VERSION,
        WS_BIND_ADDR
    );

    let listener = server::bind(WS_BIND_ADDR).await?;
    tokio::select! {
        res = server::serve(listener, ctx) => res?,
        _ = tokio::signal::ctrl_c() => info!("shutdown signal received"),
    }
    Ok(())
}

/// Mine a folder (default: the configured vault) into MemPalace, then exit.
async fn run_ingest(dir_arg: Option<String>) -> Result<()> {
    let ctx = AppContext::load();
    let dir = dir_arg
        .map(PathBuf::from)
        .or(ctx.vault_path)
        .context("no folder given and no vault configured (set vault_path in config.json)")?;

    let mut cfg = ctx.mempalace;
    cfg.enabled = true; // explicit invocation always runs

    println!("Ingesting {} into MemPalace…", dir.display());
    let res = mempalace::ingest(&cfg, &dir).await?;
    print!("{}", res.stdout);
    println!("Done.");
    Ok(())
}

/// Scan a vault (default: the configured vault) for exported Poe notes and write a
/// slug/URL manifest into `<vault>/.ravenvault/`, then exit.
async fn run_manifest(dir_arg: Option<String>) -> Result<()> {
    let ctx = AppContext::load();
    let dir = dir_arg
        .map(PathBuf::from)
        .or(ctx.vault_path)
        .context("no folder given and no vault configured (set vault_path in config.json)")?;

    let entries = manifest::build_manifest(&dir)?;
    let with_slug = entries.iter().filter(|e| !e.slug.is_empty()).count();
    let out = manifest::write_manifest(&dir, &entries)?;
    println!(
        "Wrote manifest for {} chats to {} ({} with a slug)",
        entries.len(),
        out.display(),
        with_slug
    );
    Ok(())
}

fn print_help() {
    println!(
        "Poe2Obsidian Linux companion v{}\n\n\
         USAGE:\n  \
         ravenvault              Run the WebSocket server for the extension\n  \
         ravenvault ingest [DIR] Ingest DIR (or the configured vault) into MemPalace\n  \
         ravenvault manifest [DIR] Scan DIR (or the vault) for Poe notes; write a slug/URL manifest\n  \
         ravenvault --help       Show this help",
        ravenvault::APP_VERSION
    );
}

/// Initialize structured logging. Respects `RUST_LOG`; defaults to `info`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
