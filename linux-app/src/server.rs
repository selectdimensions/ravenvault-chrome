//! The WebSocket server the extension connects to.
//!
//! The app is the **server**; the extension is the client. Each connection gets a
//! writer task (draining outbound envelopes to the socket) and a read loop that
//! routes inbound frames:
//!
//! * `response`/`error` → delivered to the pending request that initiated it,
//! * capture/asset stream messages → forwarded to the active orchestrator,
//! * control messages (handshake, check_destination, invoke_export, …) → handled
//!   here, with `invoke_export` spawning the [`crate::export`] state machine.

use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::client::Client;
use crate::context::AppContext;
use crate::export;
use crate::protocol::{handshake_response, Envelope};

/// Extension-initiated messages that belong to an in-flight export and must be
/// forwarded to the orchestrator rather than handled inline.
const STREAM_COMMANDS: &[&str] = &[
    "saveDomHtmlChunk",
    "capture_complete",
    "save_file",
    "save_file_complete",
    "save_file_error",
    "request_abort",
    "reset_timeout",
    "update_tab_status",
];

/// Bind the WebSocket server's TCP listener. Use [`crate::WS_BIND_ADDR`] in
/// production or `127.0.0.1:0` in tests to get an ephemeral port.
pub async fn bind(addr: &str) -> Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind WebSocket server to {addr}"))
}

/// Accept connections forever, each sharing the application `ctx`.
pub async fn serve(listener: TcpListener, ctx: Arc<AppContext>) -> Result<()> {
    let addr = listener.local_addr()?;
    info!(%addr, "WebSocket server listening");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "accept failed");
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, ctx).await {
                debug!(%peer, error = %e, "connection closed");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, ctx: Arc<AppContext>) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .context("websocket handshake failed")?;
    let (mut write, mut read) = ws.split();

    // Outbound: a writer task drains envelopes to the socket so any task holding
    // a Client can send concurrently.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let writer = tokio::spawn(async move {
        while let Some(env) = out_rx.recv().await {
            match env.to_json() {
                Ok(json) => {
                    if write.send(Message::text(json)).await.is_err() {
                        break;
                    }
                }
                Err(e) => warn!(error = %e, "failed to serialize outbound envelope"),
            }
        }
    });

    let client = Client::new(out_tx);

    while let Some(frame) = read.next().await {
        let frame = frame.context("read error")?;
        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                Ok(s) => s,
                Err(_) => {
                    warn!("ignoring non-utf8 binary frame");
                    continue;
                }
            },
            Message::Close(_) => break,
            _ => continue,
        };

        let env: Envelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "malformed envelope, ignoring");
                continue;
            }
        };

        route(env, &client, &ctx).await;
    }

    writer.abort();
    Ok(())
}

/// Route one inbound envelope.
async fn route(env: Envelope, client: &Client, ctx: &Arc<AppContext>) {
    // Responses/errors satisfy a request we sent.
    if matches!(env.msg_type.as_str(), "response" | "error") {
        if !client.deliver_response(env).await {
            debug!("received response with no matching pending request");
        }
        return;
    }

    let command = env.command.as_deref().unwrap_or("").to_string();

    // Stream messages belong to the active orchestrator.
    if STREAM_COMMANDS.contains(&command.as_str()) {
        if !client.forward_to_session(env).await {
            debug!(%command, "stream message with no active session");
        }
        return;
    }

    match command.as_str() {
        "handshake" => {
            let ext = env
                .args
                .as_ref()
                .and_then(|a| a.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            info!(extension_version = ext, "handshake received");
            let _ = client.send(handshake_response(env.request_id.clone()));
        }
        "ping" => debug!("keep-alive ping"),
        "log" => {
            let msg = env
                .args
                .as_ref()
                .and_then(|a| a.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            debug!(%msg, "extension log");
        }
        "check_destination" => {
            let _ = client.send(check_destination_reply(ctx, env.request_id.clone()));
        }
        "get_session_status" => {
            let _ = client.send(session_status_reply(ctx, env.request_id.clone()).await);
        }
        "invoke_export" => spawn_export(client, ctx, env).await,
        other => warn!(command = other, msg_type = %env.msg_type, "unhandled message"),
    }
}

/// Reply to `check_destination`: empty `message` means the vault is ready.
fn check_destination_reply(ctx: &Arc<AppContext>, request_id: Option<String>) -> Envelope {
    match &ctx.vault_path {
        Some(p) if p.is_dir() => Envelope::response(request_id, "check_destination", json!({})),
        Some(p) => Envelope::response(
            request_id,
            "check_destination",
            json!({ "message": format!("Vault folder does not exist: {}", p.display()) }),
        ),
        None => Envelope::response(
            request_id,
            "check_destination",
            json!({ "message": "No vault configured. Set RAVENVAULT_VAULT to your Obsidian vault path." }),
        ),
    }
}

/// Reply to `get_session_status` with string-typed fields the extension expects.
async fn session_status_reply(ctx: &Arc<AppContext>, request_id: Option<String>) -> Envelope {
    let s = ctx.session.lock().await;
    Envelope::response(
        request_id,
        "session_status",
        json!({
            "active": s.active.to_string(),
            "tabId": s.tab_id.to_string(),
            "windowId": s.window_id.to_string(),
            "status": s.status,
            "current": s.current.to_string(),
            "total": s.total.to_string(),
        }),
    )
}

/// Begin an export if none is running (single-session model).
async fn spawn_export(client: &Client, ctx: &Arc<AppContext>, invoke: Envelope) {
    {
        let mut busy = ctx.busy.lock().await;
        if *busy {
            warn!("invoke_export while another export is active; ignoring");
            return;
        }
        *busy = true;
    }
    let client = client.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        export::run_export(client, ctx, &invoke).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_destination_reports_missing_vault() {
        let ctx = Arc::new(AppContext::new(None));
        let reply = check_destination_reply(&ctx, Some("rid".into()));
        let msg = reply.result.unwrap()["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(msg.contains("No vault configured"));
    }

    #[tokio::test]
    async fn check_destination_ok_for_existing_dir() {
        let dir = std::env::temp_dir();
        let ctx = Arc::new(AppContext::new(Some(dir)));
        let reply = check_destination_reply(&ctx, Some("rid".into()));
        // Ready => no "message" field.
        assert!(reply.result.unwrap().get("message").is_none());
    }

    #[tokio::test]
    async fn session_status_fields_are_strings() {
        let ctx = Arc::new(AppContext::new(None));
        {
            let mut s = ctx.session.lock().await;
            s.active = true;
            s.tab_id = 42;
            s.current = 7;
        }
        let reply = session_status_reply(&ctx, Some("rid".into())).await;
        let r = reply.result.unwrap();
        assert_eq!(r["active"], "true");
        assert_eq!(r["tabId"], "42");
        assert_eq!(r["current"], "7");
    }
}
