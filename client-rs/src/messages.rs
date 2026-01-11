//! Message types for the Catbus protocol
//!
//! These mirror the server-side message definitions to ensure protocol compatibility.

use serde::{Deserialize, Serialize};

/// Messages sent from client to server
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Authenticate with a token
    Auth { token: String },

    /// Subscribe to a channel pattern
    Subscribe { pattern: String },

    /// Unsubscribe from a channel pattern
    Unsubscribe { pattern: String },

    /// Publish a message to a channel
    Publish {
        channel: String,
        payload: serde_json::Value,
    },

    /// Publish a Yjs document update (binary, base64-encoded)
    YjsUpdate {
        channel: String,
        update: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },

    /// Publish Yjs awareness state (cursors, presence)
    YjsAwareness { channel: String, update: String },

    /// Request Yjs sync with state vector
    YjsSyncRequest {
        channel: String,
        state_vector: String,
    },

    /// Ping for keepalive
    Ping { seq: u64 },
}

/// Messages received from server
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Authentication successful
    AuthOk {
        #[serde(default)]
        client_id: Option<String>,
    },

    /// Authentication failed
    AuthError { message: String },

    /// Subscription confirmed
    Subscribed { pattern: String },

    /// Subscription denied
    SubscribeError { pattern: String, message: String },

    /// Unsubscription confirmed
    Unsubscribed { pattern: String },

    /// Publish confirmed
    Published { channel: String },

    /// Publish denied
    PublishError { channel: String, message: String },

    /// Incoming message on a subscribed channel
    Message {
        channel: String,
        payload: serde_json::Value,
    },

    /// Incoming Yjs document update
    YjsUpdate {
        channel: String,
        update: String,
        #[serde(default)]
        origin: Option<String>,
    },

    /// Incoming Yjs awareness update
    YjsAwareness { channel: String, update: String },

    /// Response to YjsSyncRequest
    YjsSyncResponse { channel: String, update: String },

    /// Pong response
    Pong { seq: u64 },

    /// Generic error
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_auth_serialization() {
        let msg = ClientMessage::Auth {
            token: "test-token".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"auth","token":"test-token"}"#);
    }

    #[test]
    fn test_client_message_subscribe_serialization() {
        let msg = ClientMessage::Subscribe {
            pattern: "foo.*".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"subscribe","pattern":"foo.*"}"#);
    }

    #[test]
    fn test_client_message_unsubscribe_serialization() {
        let msg = ClientMessage::Unsubscribe {
            pattern: "foo.bar".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"unsubscribe","pattern":"foo.bar"}"#);
    }

    #[test]
    fn test_client_message_publish_serialization() {
        let msg = ClientMessage::Publish {
            channel: "foo.bar".to_string(),
            payload: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"publish","channel":"foo.bar","payload":{"key":"value"}}"#
        );
    }

    #[test]
    fn test_client_message_ping_serialization() {
        let msg = ClientMessage::Ping { seq: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ping","seq":42}"#);
    }

    #[test]
    fn test_client_message_yjs_update_serialization() {
        let msg = ClientMessage::YjsUpdate {
            channel: "doc.123".to_string(),
            update: "base64data".to_string(),
            origin: Some("client-1".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"yjs_update","channel":"doc.123","update":"base64data","origin":"client-1"}"#
        );
    }

    #[test]
    fn test_client_message_yjs_update_no_origin() {
        let msg = ClientMessage::YjsUpdate {
            channel: "doc.123".to_string(),
            update: "base64data".to_string(),
            origin: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"yjs_update","channel":"doc.123","update":"base64data"}"#
        );
    }

    #[test]
    fn test_server_message_auth_ok_deserialization() {
        let json = r#"{"type":"auth_ok","client_id":"client-123"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::AuthOk {
                client_id: Some("client-123".to_string())
            }
        );
    }

    #[test]
    fn test_server_message_auth_ok_no_client_id() {
        let json = r#"{"type":"auth_ok"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg, ServerMessage::AuthOk { client_id: None });
    }

    #[test]
    fn test_server_message_auth_error_deserialization() {
        let json = r#"{"type":"auth_error","message":"Invalid token"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::AuthError {
                message: "Invalid token".to_string()
            }
        );
    }

    #[test]
    fn test_server_message_subscribed_deserialization() {
        let json = r#"{"type":"subscribed","pattern":"foo.*"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::Subscribed {
                pattern: "foo.*".to_string()
            }
        );
    }

    #[test]
    fn test_server_message_subscribe_error_deserialization() {
        let json = r#"{"type":"subscribe_error","pattern":"admin.*","message":"Permission denied"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::SubscribeError {
                pattern: "admin.*".to_string(),
                message: "Permission denied".to_string()
            }
        );
    }

    #[test]
    fn test_server_message_published_deserialization() {
        let json = r#"{"type":"published","channel":"foo.bar"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::Published {
                channel: "foo.bar".to_string()
            }
        );
    }

    #[test]
    fn test_server_message_message_deserialization() {
        let json = r#"{"type":"message","channel":"foo.bar","payload":{"data":123}}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::Message {
                channel: "foo.bar".to_string(),
                payload: serde_json::json!({"data": 123})
            }
        );
    }

    #[test]
    fn test_server_message_pong_deserialization() {
        let json = r#"{"type":"pong","seq":42}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg, ServerMessage::Pong { seq: 42 });
    }

    #[test]
    fn test_server_message_error_deserialization() {
        let json = r#"{"type":"error","message":"Something went wrong"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::Error {
                message: "Something went wrong".to_string()
            }
        );
    }

    #[test]
    fn test_server_message_yjs_update_deserialization() {
        let json =
            r#"{"type":"yjs_update","channel":"doc.123","update":"base64","origin":"client-1"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::YjsUpdate {
                channel: "doc.123".to_string(),
                update: "base64".to_string(),
                origin: Some("client-1".to_string())
            }
        );
    }

    #[test]
    fn test_server_message_yjs_awareness_deserialization() {
        let json = r#"{"type":"yjs_awareness","channel":"doc.123","update":"awareness-data"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ServerMessage::YjsAwareness {
                channel: "doc.123".to_string(),
                update: "awareness-data".to_string()
            }
        );
    }
}
