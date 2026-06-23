//! Per-connection client handle.
//!
//! The app drives the export, so it must *send* requests and *await* the
//! extension's responses. [`Client`] provides:
//!
//! * [`Client::send`] — fire-and-forget (commands).
//! * [`Client::request`] — send a request and await the correlated response.
//! * a **session inbox** — the read loop forwards extension-initiated stream
//!   messages (HTML chunks, asset chunks, abort) to the active orchestrator.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::protocol::Envelope;

#[derive(Debug)]
struct ConnState {
    outbound: mpsc::UnboundedSender<Envelope>,
    pending: Mutex<HashMap<String, oneshot::Sender<Envelope>>>,
    session_inbox: Mutex<Option<mpsc::UnboundedSender<Envelope>>>,
}

/// Cheap-to-clone handle to one WebSocket connection.
#[derive(Debug, Clone)]
pub struct Client {
    state: Arc<ConnState>,
}

impl Client {
    /// Create a client over an outbound channel drained by the writer task.
    pub fn new(outbound: mpsc::UnboundedSender<Envelope>) -> Self {
        Client {
            state: Arc::new(ConnState {
                outbound,
                pending: Mutex::new(HashMap::new()),
                session_inbox: Mutex::new(None),
            }),
        }
    }

    /// Send an envelope without awaiting a reply.
    pub fn send(&self, env: Envelope) -> Result<()> {
        self.state
            .outbound
            .send(env)
            .map_err(|_| anyhow!("connection closed"))
    }

    /// Send a request and await the response correlated by `request_id`.
    pub async fn request(&self, env: Envelope, timeout: Duration) -> Result<Envelope> {
        let rid = env
            .request_id
            .clone()
            .ok_or_else(|| anyhow!("request envelope has no request_id"))?;
        let (tx, rx) = oneshot::channel();
        self.state.pending.lock().await.insert(rid.clone(), tx);
        self.send(env)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(anyhow!("response channel dropped for {rid}")),
            Err(_) => {
                self.state.pending.lock().await.remove(&rid);
                Err(anyhow!("timed out waiting for response to {rid}"))
            }
        }
    }

    /// Deliver an inbound response/error to a pending request. Returns true if it
    /// matched (and was consumed).
    pub async fn deliver_response(&self, env: Envelope) -> bool {
        if let Some(rid) = env.request_id.clone() {
            if let Some(tx) = self.state.pending.lock().await.remove(&rid) {
                let _ = tx.send(env);
                return true;
            }
        }
        false
    }

    /// Install the active orchestrator's inbox for stream messages.
    pub async fn set_session_inbox(&self, tx: mpsc::UnboundedSender<Envelope>) {
        *self.state.session_inbox.lock().await = Some(tx);
    }

    /// Remove the active session inbox.
    pub async fn clear_session_inbox(&self) {
        *self.state.session_inbox.lock().await = None;
    }

    /// Forward a stream message to the active orchestrator. Returns true if an
    /// inbox was present and accepted it.
    pub async fn forward_to_session(&self, env: Envelope) -> bool {
        if let Some(tx) = self.state.session_inbox.lock().await.as_ref() {
            return tx.send(env).is_ok();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn request_resolves_when_response_delivered() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::new(tx);

        // Simulate the extension: read our request off the wire and answer it.
        let client2 = client.clone();
        let responder = tokio::spawn(async move {
            let outgoing = rx.recv().await.unwrap();
            let rid = outgoing.request_id.clone();
            let resp = Envelope::response(rid, "scrollGetMetrics", json!({"atTop": "true"}));
            client2.deliver_response(resp).await;
        });

        let req = Envelope::request("scrollGetMetrics", json!({}));
        let resp = client
            .request(req, Duration::from_secs(2))
            .await
            .expect("should resolve");
        assert_eq!(resp.result.unwrap()["atTop"], "true");
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn request_times_out_without_response() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Client::new(tx);
        let req = Envelope::request("scrollGetMetrics", json!({}));
        let err = client
            .request(req, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn session_inbox_forwards_when_set() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Client::new(tx);
        let (in_tx, mut in_rx) = mpsc::unbounded_channel();
        client.set_session_inbox(in_tx).await;

        let chunk = Envelope::request("saveDomHtmlChunk", json!({"chunkIndex": 0}));
        assert!(client.forward_to_session(chunk).await);
        assert_eq!(
            in_rx.recv().await.unwrap().command.as_deref(),
            Some("saveDomHtmlChunk")
        );

        client.clear_session_inbox().await;
        let chunk2 = Envelope::request("saveDomHtmlChunk", json!({"chunkIndex": 1}));
        assert!(!client.forward_to_session(chunk2).await);
    }
}
