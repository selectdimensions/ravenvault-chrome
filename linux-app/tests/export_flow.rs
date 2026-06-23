//! End-to-end orchestration test: a mock extension plays the client side of the
//! protocol — handshake, invoke_export, answering the scroll loop, and streaming
//! chunked page HTML — and asserts the app drives capture to a successful finish.

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use ravenvault::context::AppContext;
use ravenvault::server;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

async fn send(sink: &mut Sink, v: Value) {
    sink.send(Message::text(v.to_string())).await.unwrap();
}

fn respond(request_id: &Value, command: &str, result: Value) -> Value {
    json!({
        "version": "1",
        "request_id": request_id,
        "source": "app",
        "type": "response",
        "command": command,
        "result": result,
    })
}

#[tokio::test]
async fn full_export_flow_reaches_success() {
    let listener = server::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, Arc::new(AppContext::new(None))));

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    // Handshake.
    send(
        &mut tx,
        json!({"version":"1","request_id":"hs","source":"extension",
               "type":"command","command":"handshake","args":{"version":"0.10.0"}}),
    )
    .await;
    let hs = rx.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&hs).unwrap()["command"],
        "handshake"
    );

    // Kick off the export.
    send(
        &mut tx,
        json!({"version":"1","request_id":"inv","source":"extension",
               "type":"command","command":"invoke_export","args":{"tabId":123,"windowId":45}}),
    )
    .await;

    // The HTML the mock will "capture", split across two base64 chunks.
    let html_parts = [
        "<!DOCTYPE html><html><body>",
        "<p>hello poe</p></body></html>",
    ];
    let mut metrics_calls = 0;

    // Drive the protocol until the app reports success (or we time out).
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(frame) = rx.next().await {
            let text = frame.unwrap().into_text().unwrap();
            let env: Value = serde_json::from_str(&text).unwrap();
            let command = env["command"].as_str().unwrap_or("");
            let rid = env["request_id"].clone();

            match command {
                "scrollGetMetrics" => {
                    metrics_calls += 1;
                    // Reach the top on the second poll (one scroll iteration).
                    let at_top = if metrics_calls >= 2 { "true" } else { "false" };
                    send(
                        &mut tx,
                        respond(
                            &rid,
                            command,
                            json!({
                                "scrollTop": "-100", "scrollHeight": "2000",
                                "clientHeight": "800", "atTop": at_top,
                                "acceptsNegative": "true"
                            }),
                        ),
                    )
                    .await;
                }
                "domQuery" => {
                    send(&mut tx, respond(&rid, command, json!({"count": "3"}))).await;
                }
                "scrollBy" => {
                    send(
                        &mut tx,
                        respond(&rid, command, json!({"ok": "true", "appliedTop": "-900"})),
                    )
                    .await;
                }
                "capture_start" => {
                    // Stream the page HTML as the extension would.
                    for (i, part) in html_parts.iter().enumerate() {
                        send(
                            &mut tx,
                            json!({
                                "version":"1","request_id":"cap","source":"extension",
                                "type":"request","command":"saveDomHtmlChunk",
                                "args":{"chunkBase64": STANDARD.encode(part),
                                        "chunkIndex": i, "totalChunks": html_parts.len()}
                            }),
                        )
                        .await;
                    }
                    send(
                        &mut tx,
                        json!({
                            "version":"1","request_id":"cap","source":"extension",
                            "type":"command","command":"capture_complete",
                            "args":{"session":{"tabId":123,"windowId":45},
                                    "totalChunks": html_parts.len(), "chatTitle":"My Chat"}
                        }),
                    )
                    .await;
                }
                "update_ui" => {
                    return env["args"]["ui"]["type"].as_str().unwrap_or("").to_string();
                }
                "abort_export" => {
                    panic!(
                        "export aborted: {}",
                        env["args"]["message"].as_str().unwrap_or("?")
                    );
                }
                // startKeepAlive / stopScroll / stopKeepAlive and anything else.
                _ => {
                    if env["type"] == "request" {
                        send(&mut tx, respond(&rid, command, json!({"ok": "true"}))).await;
                    }
                }
            }
        }
        "connection-closed".to_string()
    })
    .await
    .expect("export flow timed out");

    assert_eq!(result, "success", "app should report a successful export");
    assert!(metrics_calls >= 2, "scroll loop should poll metrics");
}
