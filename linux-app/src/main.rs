//! RavenVault Linux companion app — headless binary entry point.
//!
//! Initializes logging and runs the WebSocket server the extension connects to.

use std::sync::Arc;

use anyhow::Result;
use ravenvault::context::AppContext;
use ravenvault::server;
use ravenvault::WS_BIND_ADDR;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let ctx = Arc::new(AppContext::load());
    match &ctx.vault_path {
        Some(p) => info!(vault = %p.display(), "vault configured"),
        None => warn!("no vault configured; set RAVENVAULT_VAULT to your Obsidian vault path"),
    }
    info!(
        version = ravenvault::APP_VERSION,
        bind = WS_BIND_ADDR,
        "RavenVault Linux companion starting"
    );
    println!(
        "RavenVault Linux companion v{} — WebSocket server on ws://{}",
        ravenvault::APP_VERSION,
        WS_BIND_ADDR
    );

    let listener = server::bind(WS_BIND_ADDR).await?;

    tokio::select! {
        res = server::serve(listener, ctx) => {
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown signal received");
        }
    }
    Ok(())
}

/// Initialize structured logging. Respects `RUST_LOG`; defaults to `info`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
