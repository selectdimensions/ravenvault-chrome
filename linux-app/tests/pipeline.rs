//! Full end-to-end pipeline test: mock extension → capture HTML containing a
//! poecdn image → app converts (M3), downloads the asset (M2 relay), and writes
//! a real note + attachment into a temp Obsidian vault (M4). Asserts files land.

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

const PAGE_HTML: &str = r#"<!DOCTYPE html><html><head><title>Trip Plan - Poe</title></head>
<body><div class="ChatMessagesView_a">
<div class="ChatMessage_chatMessage_x human"><div class="Markdown_y"><p>Plan my trip?</p></div></div>
<div class="ChatMessage_chatMessage_x bot"><div class="Markdown_y"><p>Here is a map</p>
<p><img src="https://poecdn.net/pic.png" alt="map"></p></div></div>
</div></body></html>"#;

// A minimal valid-ish PNG header so extension derivation/sniffing is exercised.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRfake-image-data";

#[tokio::test]
async fn pipeline_writes_note_and_attachment_to_vault() {
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
        json!({"version":"1","request_id":"hs","source":"extension","type":"command",
               "command":"handshake","args":{"version":"0.10.0"}}),
    )
    .await;
    rx.next().await.unwrap().unwrap(); // handshake response

    send(
        &mut tx,
        json!({"version":"1","request_id":"inv","source":"extension","type":"command",
               "command":"invoke_export","args":{"tabId":1,"windowId":1}}),
    )
    .await;

    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(frame) = rx.next().await {
            let env: Value = serde_json::from_str(&frame.unwrap().into_text().unwrap()).unwrap();
            let command = env["command"].as_str().unwrap_or("");
            let rid = env["request_id"].clone();
            match command {
                // Already at the top: capture immediately.
                "scrollGetMetrics" => {
                    send(
                        &mut tx,
                        respond(
                            &rid,
                            command,
                            json!({
                        "scrollTop":"0","scrollHeight":"800","clientHeight":"800",
                        "atTop":"true","acceptsNegative":"false"}),
                        ),
                    )
                    .await;
                }
                "capture_start" => {
                    send(
                        &mut tx,
                        json!({"version":"1","request_id":"cap","source":"extension",
                        "type":"request","command":"saveDomHtmlChunk",
                        "args":{"chunkBase64":STANDARD.encode(PAGE_HTML),
                                "chunkIndex":0,"totalChunks":1}}),
                    )
                    .await;
                    send(
                        &mut tx,
                        json!({"version":"1","request_id":"cap","source":"extension",
                        "type":"command","command":"capture_complete",
                        "args":{"session":{"tabId":1,"windowId":1},
                                "totalChunks":1,"chatTitle":"Trip Plan"}}),
                    )
                    .await;
                }
                "download" => {
                    let url = env["args"]["url"].clone();
                    send(
                        &mut tx,
                        json!({"version":"1","request_id":"dl","source":"extension",
                        "type":"request","command":"save_file",
                        "args":{"url":url,"chunkBase64":STANDARD.encode(PNG_BYTES),
                                "chunkIndex":0,"totalChunks":1}}),
                    )
                    .await;
                    send(
                        &mut tx,
                        json!({"version":"1","request_id":"dl","source":"extension",
                        "type":"request","command":"save_file_complete",
                        "args":{"url":url}}),
                    )
                    .await;
                }
                "update_ui" => {
                    return env["args"]["ui"]["type"].as_str().unwrap_or("").to_string();
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
    .expect("pipeline timed out");

    assert_eq!(outcome, "success");

    // The note landed in the vault.
    let note = vault.path().join("Trip Plan.md");
    assert!(note.is_file(), "expected note at {}", note.display());
    let body = std::fs::read_to_string(&note).unwrap();
    assert!(body.contains("title:"), "frontmatter present");
    assert!(
        body.contains("attachments/"),
        "image rewritten to local path"
    );
    assert!(
        !body.contains("poecdn.net"),
        "original asset URL should be rewritten away"
    );

    // The attachment landed in attachments/ (exactly one file).
    let att = vault.path().join("attachments");
    let count = std::fs::read_dir(&att).unwrap().count();
    assert_eq!(count, 1, "exactly one attachment written");
}
