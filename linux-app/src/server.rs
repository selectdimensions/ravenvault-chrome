//! The WebSocket server the extension connects to.
//!
//! The app is the **server**; the extension is the client. We bind
//! `127.0.0.1:53122` (see [`crate::WS_BIND_ADDR`]), accept connections, and
//! dispatch each inbound JSON envelope through a [`Dispatcher`].
//!
//! At milestone M1 the dispatcher only answers the handshake, ping, and log; the
//! full export orchestration is layered on in M2.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::protocol::{handshake_response, Envelope};

/// Handles inbound envelopes and produces zero or more reply envelopes.
///
/// This is intentionally a plain struct (not a trait yet) so M2 can grow it with
/// session state and proactive sends without churning callers.
#[derive(Debug, Default, Clone)]
pub struct Dispatcher {}

impl Dispatcher {
    pub fn new() -> Self {
        Dispatcher::default()
    }

    /// Process one inbound envelope, returning replies to send back.
    pub async fn handle(&self, env: &Envelope) -> Vec<Envelope> {
        let command = env.command.as_deref().unwrap_or("");
        match (env.msg_type.as_str(), command) {
            ("command", "handshake") => {
                let ext_version = env
                    .args
                    .as_ref()
                    .and_then(|a| a.get("version"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                info!(extension_version = ext_version, "handshake received");
                vec![handshake_response(env.request_id.clone())]
            }
            ("ping", _) => {
                debug!(request_id = ?env.request_id, "keep-alive ping");
                vec![]
            }
            (_, "log") => {
                let msg = env
                    .args
                    .as_ref()
                    .and_then(|a| a.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let url = env
                    .args
                    .as_ref()
                    .and_then(|a| a.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                debug!(%msg, %url, "extension log");
                vec![]
            }
            (ty, cmd) => {
                warn!(msg_type = ty, command = cmd, "unhandled message (todo M2)");
                vec![]
            }
        }
    }
}

/// Bind the WebSocket server's TCP listener. Pass [`crate::WS_BIND_ADDR`] in
/// production, or `127.0.0.1:0` in tests to get an ephemeral port (read it back
/// via [`TcpListener::local_addr`]).
pub async fn bind(addr: &str) -> Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind WebSocket server to {addr}"))
}

/// Accept connections forever, dispatching each through `dispatcher`.
pub async fn serve(listener: TcpListener, dispatcher: Dispatcher) -> Result<()> {
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
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, dispatcher).await {
                debug!(%peer, error = %e, "connection closed");
            }
        });
    }
}

/// Drive a single client connection: read frames, dispatch, write replies.
async fn handle_connection(stream: TcpStream, dispatcher: Dispatcher) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .context("websocket handshake failed")?;
    let (mut write, mut read) = ws.split();

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
            // Control frames (ping/pong) are handled by the library.
            _ => continue,
        };

        let env: Envelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "malformed envelope, ignoring");
                continue;
            }
        };

        for reply in dispatcher.handle(&env).await {
            let json = reply.to_json().context("serialize reply")?;
            write
                .send(Message::text(json))
                .await
                .context("write reply")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn dispatcher_answers_handshake() {
        let d = Dispatcher::new();
        let env = Envelope {
            version: "1".into(),
            request_id: Some("rid-1".into()),
            source: Some("extension".into()),
            msg_type: "command".into(),
            command: Some("handshake".into()),
            args: Some(json!({"version": "0.10.0"})),
            result: None,
            error: None,
        };
        let replies = d.handle(&env).await;
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].command.as_deref(), Some("handshake"));
        assert_eq!(replies[0].request_id.as_deref(), Some("rid-1"));
    }

    #[tokio::test]
    async fn dispatcher_ignores_ping_and_log() {
        let d = Dispatcher::new();
        let ping = Envelope {
            version: "1".into(),
            request_id: Some("keep-alive-1".into()),
            source: Some("extension".into()),
            msg_type: "ping".into(),
            command: Some("ping".into()),
            args: None,
            result: None,
            error: None,
        };
        assert!(d.handle(&ping).await.is_empty());
    }
}
