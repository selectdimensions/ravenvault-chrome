//! RavenVault Linux companion app — headless binary entry point.
//!
//! Initializes logging and runs the WebSocket server the extension connects to.

use anyhow::Result;
use ravenvault::server::{self, Dispatcher};
use ravenvault::WS_BIND_ADDR;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
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
    let dispatcher = Dispatcher::new();

    tokio::select! {
        res = server::serve(listener, dispatcher) => {
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
