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

    // Explicit user Cancel — always stop the run.
    if command == "request_abort" {
        ctx.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        client.forward_to_session(env).await;
        return;
    }
    // Programmatic abort — usually the export tab navigating. A bulk run
    // navigates the tab on purpose for every chat, so ignore it during bulk;
    // otherwise forward it to the active single-export capture so it can abort.
    if command == "abort_export" {
        if !ctx.bulk_active.load(std::sync::atomic::Ordering::SeqCst) {
            client.forward_to_session(env).await;
        }
        return;
    }

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
            // The extension logs the active tab's URL (message "Active tab URL",
            // url in args.url). Remember poe.com conversation URLs so single
            // exports can record the conversation as the note's `source`.
            if let Some(url) = active_tab_url(&env) {
                *ctx.last_active_url.lock().await = Some(url);
            }
        }
        "open_result" => {
            let last = ctx.last_note_path.lock().await.clone();
            match open_target(last, ctx.vault_path.clone()) {
                Some(path) => open_in_default_app(&path),
                None => debug!("open_result with no target to open"),
            }
        }
        "check_destination" => {
            let _ = client.send(check_destination_reply(ctx, env.request_id.clone()));
        }
        "get_session_status" => {
            let _ = client.send(session_status_reply(ctx, env.request_id.clone()).await);
        }
        "invoke_export" => spawn_export(client, ctx, env, false).await,
        "invoke_bulk_export" => spawn_export(client, ctx, env, true).await,
        other => warn!(command = other, msg_type = %env.msg_type, "unhandled message"),
    }
}

/// Extract a poe.com active-tab URL from a `log` envelope, if it is one.
///
/// The extension emits `log {message:"Active tab URL", url}` for the focused tab.
/// Returns the URL only when the message matches and the url looks like a
/// poe.com URL — pure (no side effects), so it can be unit-tested directly.
fn active_tab_url(env: &Envelope) -> Option<String> {
    let args = env.args.as_ref()?;
    let message = args.get("message").and_then(|v| v.as_str())?;
    if message != "Active tab URL" {
        return None;
    }
    let url = args.get("url").and_then(|v| v.as_str())?;
    is_poe_url(url).then(|| url.to_string())
}

/// Whether `url` is an http(s) URL whose host is poe.com (or a subdomain).
fn is_poe_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let Some(rest) = rest else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Strip any userinfo/port.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host == "poe.com" || host.ends_with(".poe.com")
}

/// Resolve what `open_result` should open: the last written note if we have one,
/// else the vault root, else nothing. Pure (no I/O) so it is unit-testable.
fn open_target(
    last_note_path: Option<std::path::PathBuf>,
    vault_path: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    last_note_path.or(vault_path)
}

/// Open `path` in the OS default application via `xdg-open`, detached. Best-effort:
/// spawn errors are ignored (the user just won't see a window pop up).
fn open_in_default_app(path: &std::path::Path) {
    match std::process::Command::new("xdg-open").arg(path).spawn() {
        Ok(_) => info!(path = %path.display(), "opened result in default app"),
        Err(e) => warn!(path = %path.display(), error = %e, "failed to xdg-open result"),
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

/// Begin an export (single or bulk) if none is running (single-session model).
async fn spawn_export(client: &Client, ctx: &Arc<AppContext>, invoke: Envelope, bulk: bool) {
    {
        let mut busy = ctx.busy.lock().await;
        if *busy {
            warn!("export requested while another is active; ignoring");
            return;
        }
        *busy = true;
    }
    let client = client.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        if bulk {
            export::run_bulk_export(client, ctx, &invoke).await;
        } else {
            export::run_export(client, ctx, &invoke).await;
        }
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

    #[test]
    fn open_target_prefers_last_note_then_vault_then_none() {
        use std::path::PathBuf;
        let note = PathBuf::from("/v/Note.md");
        let vault = PathBuf::from("/v");
        // Last note set => returns the note.
        assert_eq!(
            open_target(Some(note.clone()), Some(vault.clone())),
            Some(note.clone())
        );
        // No note => returns the vault.
        assert_eq!(open_target(None, Some(vault.clone())), Some(vault));
        // Neither => None.
        assert_eq!(open_target(None, None), None);
    }

    #[test]
    fn active_tab_url_extracts_only_matching_poe_logs() {
        // Matching message + poe.com url => extracted.
        let e = Envelope::command(
            "log",
            json!({ "message": "Active tab URL", "url": "https://poe.com/chat/abc" }),
        );
        assert_eq!(
            active_tab_url(&e).as_deref(),
            Some("https://poe.com/chat/abc")
        );

        // Different message => ignored even with a poe url.
        let e2 = Envelope::command(
            "log",
            json!({ "message": "something else", "url": "https://poe.com/chat/abc" }),
        );
        assert!(active_tab_url(&e2).is_none());

        // Right message but non-poe url => ignored.
        let e3 = Envelope::command(
            "log",
            json!({ "message": "Active tab URL", "url": "https://example.com/x" }),
        );
        assert!(active_tab_url(&e3).is_none());

        // Subdomain of poe.com is accepted.
        let e4 = Envelope::command(
            "log",
            json!({ "message": "Active tab URL", "url": "https://www.poe.com/chat/z" }),
        );
        assert_eq!(
            active_tab_url(&e4).as_deref(),
            Some("https://www.poe.com/chat/z")
        );

        // A look-alike host is rejected.
        let e5 = Envelope::command(
            "log",
            json!({ "message": "Active tab URL", "url": "https://poe.com.evil.com/x" }),
        );
        assert!(active_tab_url(&e5).is_none());
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
