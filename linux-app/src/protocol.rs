//! The JSON message envelope spoken between the extension and this app.
//!
//! See `docs/PROTOCOL.md`. Every message is a single JSON object in a WebSocket
//! text frame. Field shapes vary by `command`, and results forwarded from the
//! content script arrive with **all values coerced to strings**, so the flexible
//! fields are kept as [`serde_json::Value`].

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{APP_VERSION, MIN_EXTENSION_VERSION, PROTOCOL_VERSION};

/// `source` value the app stamps on its outgoing messages.
pub const SOURCE_APP: &str = "app";

/// Message envelope. Used for both incoming and outgoing messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol version, always `"1"`.
    #[serde(default = "default_version")]
    pub version: String,

    /// UUID correlating a request with its response. Keep-alive pings use
    /// `"keep-alive-<timestamp>"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// `"extension"` on inbound messages; `"app"` on our outbound messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// `command` | `request` | `response` | `error` | `event` | `ping`.
    #[serde(rename = "type")]
    pub msg_type: String,

    /// The command/action name (e.g. `handshake`, `scrollSet`, `download`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Arguments for a command/request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,

    /// Payload on a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,

    /// Payload on an error reply (`{ "message": "..." }`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

fn default_version() -> String {
    PROTOCOL_VERSION.to_string()
}

impl Envelope {
    /// Build a `response` envelope correlated to `request_id`.
    pub fn response(request_id: Option<String>, command: &str, result: Value) -> Self {
        Envelope {
            version: PROTOCOL_VERSION.to_string(),
            request_id,
            source: Some(SOURCE_APP.to_string()),
            msg_type: "response".to_string(),
            command: Some(command.to_string()),
            args: None,
            result: Some(result),
            error: None,
        }
    }

    /// Build an `error` envelope correlated to `request_id`.
    pub fn error_reply(request_id: Option<String>, command: &str, message: &str) -> Self {
        Envelope {
            version: PROTOCOL_VERSION.to_string(),
            request_id,
            source: Some(SOURCE_APP.to_string()),
            msg_type: "error".to_string(),
            command: Some(command.to_string()),
            args: None,
            result: None,
            error: Some(json!({ "message": message })),
        }
    }

    /// Build a `command` envelope the app initiates (e.g. orchestration). A fresh
    /// UUID `request_id` is generated.
    pub fn command(command: &str, args: Value) -> Self {
        Envelope {
            version: PROTOCOL_VERSION.to_string(),
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            source: Some(SOURCE_APP.to_string()),
            msg_type: "command".to_string(),
            command: Some(command.to_string()),
            args: Some(args),
            result: None,
            error: None,
        }
    }

    /// Build a `request` envelope the app initiates (expects a response). A fresh
    /// UUID `request_id` is generated.
    pub fn request(command: &str, args: Value) -> Self {
        let mut e = Envelope::command(command, args);
        e.msg_type = "request".to_string();
        e
    }

    /// Serialize to a JSON string for a WebSocket text frame.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

/// Build the handshake response the extension requires. Reports our app version
/// and the minimum extension version we support. The extension blocks exports
/// unless `app_version >= 0.9.1`.
pub fn handshake_response(request_id: Option<String>) -> Envelope {
    Envelope::response(
        request_id,
        "handshake",
        json!({
            "app_version": APP_VERSION,
            "min_extension_version": MIN_EXTENSION_VERSION,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extension_handshake() {
        let raw = r#"{"version":"1","request_id":"abc-123","source":"extension",
            "type":"command","command":"handshake","args":{"version":"0.10.0"}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.msg_type, "command");
        assert_eq!(env.command.as_deref(), Some("handshake"));
        assert_eq!(env.request_id.as_deref(), Some("abc-123"));
        assert_eq!(env.args.unwrap()["version"], "0.10.0");
    }

    #[test]
    fn builds_handshake_response_with_required_fields() {
        let resp = handshake_response(Some("abc-123".into()));
        assert_eq!(resp.msg_type, "response");
        assert_eq!(resp.command.as_deref(), Some("handshake"));
        assert_eq!(resp.request_id.as_deref(), Some("abc-123"));
        assert_eq!(resp.source.as_deref(), Some("app"));
        let result = resp.result.unwrap();
        assert_eq!(result["app_version"], APP_VERSION);
        assert_eq!(result["min_extension_version"], MIN_EXTENSION_VERSION);
    }

    #[test]
    fn response_roundtrips_through_json() {
        let resp = handshake_response(Some("rid".into()));
        let s = resp.to_json().unwrap();
        let back: Envelope = serde_json::from_str(&s).unwrap();
        assert_eq!(back.request_id.as_deref(), Some("rid"));
        // The serialized form uses "type", not "msg_type".
        assert!(s.contains("\"type\":\"response\""));
        assert!(!s.contains("msg_type"));
    }

    #[test]
    fn omits_none_fields_when_serializing() {
        let resp = Envelope::response(None, "ping", json!({}));
        let s = resp.to_json().unwrap();
        assert!(!s.contains("request_id"));
        assert!(!s.contains("\"error\""));
        assert!(!s.contains("\"args\""));
    }
}
