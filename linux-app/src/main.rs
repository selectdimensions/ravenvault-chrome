//! RavenVault Linux companion app — headless binary entry point.
//!
//! For now this just initializes logging and prints a banner; the WebSocket
//! server is wired in at milestone M1.

use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();
    info!(
        version = ravenvault::APP_VERSION,
        bind = ravenvault::WS_BIND_ADDR,
        "RavenVault Linux companion starting"
    );
    println!(
        "RavenVault Linux companion v{} (protocol v{})",
        ravenvault::APP_VERSION,
        ravenvault::PROTOCOL_VERSION
    );
    println!("WebSocket server target: {}", ravenvault::WS_BIND_ADDR);
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
