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
use ravenvault::mcp::McpClient;
use ravenvault::WS_BIND_ADDR;
use ravenvault::{manifest, mempalace, relate, server};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let mut args = std::env::args().skip(1);
    if let Some(cmd) = args.next() {
        let rest: Vec<String> = args.collect();
        match cmd.as_str() {
            "ingest" => return run_ingest(rest).await,
            "relate" => return run_relate(rest).await,
            "manifest" => return run_manifest(rest.into_iter().next()).await,
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
async fn run_ingest(args: Vec<String>) -> Result<()> {
    let ctx = AppContext::load();
    let (dir_arg, wing) = parse_dir_and_wing(&args)?;
    let dir = dir_arg
        .map(PathBuf::from)
        .or(ctx.vault_path)
        .context("no folder given and no vault configured (set vault_path in config.json)")?;

    let mut cfg = ctx.mempalace;
    cfg.enabled = true; // explicit invocation always runs
    if wing.is_some() {
        cfg.wing = wing;
    }

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

/// For every note in the vault, find related notes via MemPalace and append a
/// `## Related` section of `[[wikilinks]]`, then exit.
async fn run_relate(args: Vec<String>) -> Result<()> {
    let ctx = AppContext::load();

    let mut dir_arg: Option<String> = None;
    let mut wing: Option<String> = None;
    let mut limit: usize = 6;
    let mut min_similarity: f64 = 0.3;

    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--wing" => wing = Some(it.next().context("--wing requires a value")?),
            "--limit" => {
                limit = it
                    .next()
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be a non-negative integer")?
            }
            "--min-similarity" => {
                min_similarity = it
                    .next()
                    .context("--min-similarity requires a value")?
                    .parse()
                    .context("--min-similarity must be a number")?
            }
            other if other.starts_with('-') => {
                return Err(anyhow::anyhow!("unknown flag: {other}"));
            }
            _ if dir_arg.is_none() => dir_arg = Some(a),
            other => return Err(anyhow::anyhow!("unexpected argument: {other}")),
        }
    }

    let dir = dir_arg
        .map(PathBuf::from)
        .or(ctx.vault_path)
        .context("no folder given and no vault configured (set vault_path in config.json)")?;

    let mcp_bin = McpClient::resolve_bin(&ctx.mempalace.binary);

    println!("Relating notes in {} via MemPalace…", dir.display());
    let summary = relate::run(&dir, wing.as_deref(), limit, min_similarity, &mcp_bin).await?;
    println!(
        "Linked {}/{} notes with {} related links",
        summary.linked, summary.notes, summary.total_links
    );
    Ok(())
}

/// Parse a shared `[DIR] [--wing W]` argument list (used by `ingest`).
fn parse_dir_and_wing(args: &[String]) -> Result<(Option<String>, Option<String>)> {
    let mut dir_arg: Option<String> = None;
    let mut wing: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--wing" => {
                wing = Some(
                    args.get(i + 1)
                        .context("--wing requires a value")?
                        .to_string(),
                );
                i += 2;
            }
            other if other.starts_with('-') => {
                return Err(anyhow::anyhow!("unknown flag: {other}"));
            }
            _ if dir_arg.is_none() => {
                dir_arg = Some(args[i].clone());
                i += 1;
            }
            other => return Err(anyhow::anyhow!("unexpected argument: {other}")),
        }
    }
    Ok((dir_arg, wing))
}

fn print_help() {
    println!(
        "Poe2Obsidian Linux companion v{}\n\n\
         USAGE:\n  \
         ravenvault              Run the WebSocket server for the extension\n  \
         ravenvault ingest [DIR] [--wing W] Ingest DIR (or the configured vault) into MemPalace\n  \
         ravenvault relate [DIR] [--wing W] [--limit N] [--min-similarity F]\n                          Link related notes via MemPalace (## Related wikilinks)\n  \
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
