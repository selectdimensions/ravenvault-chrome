//! Integration test: stand up the real WebSocket server on an ephemeral port and
//! replay the exact handshake the extension sends, asserting we answer correctly.

use std::cmp::Ordering;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use ravenvault::context::AppContext;
use ravenvault::server;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn server_answers_extension_handshake() {
    // Bind on port 0 -> OS picks a free port; serve in the background.
    let listener = server::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, Arc::new(AppContext::new(None))));

    // Connect as the extension does.
    let url = format!("ws://{addr}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();

    // Send the exact handshake envelope the extension emits on ws.onopen.
    let handshake = serde_json::json!({
        "version": "1",
        "request_id": "test-rid-42",
        "source": "extension",
        "type": "command",
        "command": "handshake",
        "args": { "version": "0.10.0" }
    });
    ws.send(Message::text(handshake.to_string())).await.unwrap();

    // Read the reply.
    let reply = ws.next().await.expect("no reply").expect("read error");
    let text = reply.into_text().unwrap();
    let env: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(env["type"], "response");
    assert_eq!(env["command"], "handshake");
    assert_eq!(env["request_id"], "test-rid-42", "must echo request_id");

    let app_version = env["result"]["app_version"].as_str().unwrap();
    assert_ne!(
        ravenvault::cmp_versions(app_version, "0.9.1"),
        Ordering::Less,
        "app_version {app_version} must be >= the extension's MIN_APP_VERSION (0.9.1)"
    );
    assert_eq!(env["result"]["min_extension_version"], "0.10.0");
}

#[tokio::test]
async fn server_ignores_keep_alive_ping_without_replying() {
    let listener = server::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, Arc::new(AppContext::new(None))));

    let url = format!("ws://{addr}");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    let ping = serde_json::json!({
        "version": "1",
        "request_id": "keep-alive-123",
        "source": "extension",
        "type": "ping",
        "command": "ping",
        "args": {}
    });
    ws.send(Message::text(ping.to_string())).await.unwrap();

    // Follow with a handshake; the first frame we get back must be the handshake
    // response (proving the ping produced no reply ahead of it).
    let handshake = serde_json::json!({
        "version": "1", "request_id": "after-ping", "source": "extension",
        "type": "command", "command": "handshake", "args": { "version": "0.10.0" }
    });
    ws.send(Message::text(handshake.to_string())).await.unwrap();

    let reply = ws.next().await.expect("no reply").expect("read error");
    let env: serde_json::Value = serde_json::from_str(&reply.into_text().unwrap()).unwrap();
    assert_eq!(env["request_id"], "after-ping");
    assert_eq!(env["command"], "handshake");
}
