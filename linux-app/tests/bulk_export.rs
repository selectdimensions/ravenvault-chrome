//! Bulk "Export All" flow: a mock extension serves a chat list, acks navigation,
//! and streams a capture per chat. Asserts the app loops and writes every note.

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
    json!({"version":"1","request_id":request_id,"source":"app",
           "type":"response","command":command,"result":result})
}

fn page_html(body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>{body} - Poe</title></head>\
         <body><div class=\"ChatMessagesView_a\">\
         <div class=\"ChatMessage_chatMessage_x bot\"><div class=\"Markdown_y\"><p>{body}</p></div></div>\
         </div></body></html>"
    )
}

#[tokio::test]
async fn bulk_export_writes_every_chat() {
    let vault = tempfile::tempdir().unwrap();
    let listener = server::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(
        listener,
        Arc::new(AppContext::new(Some(vault.path().to_path_buf()))),
    ));

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    send(
        &mut tx,
        json!({"version":"1","request_id":"hs","source":"extension",
        "type":"command","command":"handshake","args":{"version":"0.10.0"}}),
    )
    .await;
    rx.next().await.unwrap().unwrap();

    // Kick off bulk export.
    send(
        &mut tx,
        json!({"version":"1","request_id":"inv","source":"extension",
        "type":"command","command":"invoke_bulk_export","args":{"tabId":1,"windowId":1}}),
    )
    .await;

    // The two chats the mock will serve.
    let chats = json!([
        {"url":"https://poe.com/chat/aaa","title":"Alpha"},
        {"url":"https://poe.com/chat/bbb","title":"Beta"}
    ])
    .to_string();

    let mut current_title = String::new();

    let outcome = tokio::time::timeout(Duration::from_secs(25), async {
        while let Some(frame) = rx.next().await {
            let env: Value = serde_json::from_str(&frame.unwrap().into_text().unwrap()).unwrap();
            let command = env["command"].as_str().unwrap_or("");
            let rid = env["request_id"].clone();
            match command {
                "list_chats" => {
                    send(&mut tx, respond(&rid, command, json!({ "chats": chats }))).await;
                }
                "navigate" => {
                    // Remember which chat we're on so capture returns matching HTML.
                    let url = env["args"]["url"].as_str().unwrap_or("");
                    current_title = if url.ends_with("aaa") { "Alpha" } else { "Beta" }.to_string();
                    send(&mut tx, respond(&rid, command, json!({ "ok": "true" }))).await;
                }
                "scrollGetMetrics" => {
                    send(&mut tx, respond(&rid, command, json!({
                        "scrollTop":"0","scrollHeight":"800","clientHeight":"800",
                        "atTop":"true","acceptsNegative":"false"}))).await;
                }
                "capture_start" => {
                    let html = page_html(&current_title);
                    send(&mut tx, json!({"version":"1","request_id":"cap","source":"extension",
                        "type":"request","command":"saveDomHtmlChunk",
                        "args":{"chunkBase64":STANDARD.encode(&html),"chunkIndex":0,"totalChunks":1}})).await;
                    send(&mut tx, json!({"version":"1","request_id":"cap","source":"extension",
                        "type":"command","command":"capture_complete",
                        "args":{"session":{"tabId":1,"windowId":1},"totalChunks":1,
                                "chatTitle":current_title}})).await;
                }
                "update_ui" => {
                    let ui = &env["args"]["ui"];
                    if ui["type"] == "success" {
                        return ui["message"].as_str().unwrap_or("").to_string();
                    }
                }
                "abort_export" => panic!("aborted: {}", env["args"]["message"]),
                _ => {
                    if env["type"] == "request" {
                        send(&mut tx, respond(&rid, command, json!({"ok":"true"}))).await;
                    }
                }
            }
        }
        "closed".to_string()
    })
    .await
    .expect("bulk flow timed out");

    assert!(outcome.contains("2 exported"), "summary was: {outcome}");

    // Both notes exist.
    assert!(vault.path().join("Alpha.md").is_file(), "Alpha.md missing");
    assert!(vault.path().join("Beta.md").is_file(), "Beta.md missing");
}
