//! WebSocket server implementation
//!
//! Provides WebSocket transport as an alternative to WebTransport for
//! environments where UDP/QUIC is not available (e.g., behind reverse proxies).

use crate::auth::{AdminKey, GrantSet, Token};
use crate::channels::{Channel, ChannelPattern};
use crate::server::connections::{ClientConnection, ConnectionManager, OutboundMessage};
use crate::server::router::TopicRouter;
use crate::server::transport::{ClientMessage, ServerMessage};
use crate::storage::RoleStore;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Shared state for WebSocket handlers
#[derive(Clone)]
pub struct WsState {
    pub connections: Arc<ConnectionManager>,
    pub router: Arc<TopicRouter>,
    pub role_store: Arc<dyn RoleStore>,
    pub token_secret: Vec<u8>,
    pub admin_key: Option<AdminKey>,
}

/// Create the WebSocket router
pub fn create_router(state: WsState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WsState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: WsState) {
    let (mut sender, mut receiver) = socket.split();

    // Wait for auth message and authenticate
    let auth_result = match receiver.next().await {
        Some(Ok(Message::Text(text))) => {
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Auth { token }) => authenticate(&token, &state).await,
                Ok(_) => Err("Must authenticate first".to_string()),
                Err(e) => Err(format!("Invalid message: {}", e)),
            }
        }
        Some(Ok(Message::Binary(data))) => {
            match serde_json::from_slice::<ClientMessage>(&data) {
                Ok(ClientMessage::Auth { token }) => authenticate(&token, &state).await,
                _ => Err("Must authenticate first".to_string()),
            }
        }
        _ => return,
    };

    // Handle auth result
    let (client_id, grants, is_admin) = match auth_result {
        Ok(auth) => {
            let response = ServerMessage::AuthOk { client_id: auth.0.clone() };
            if sender
                .send(Message::Text(serde_json::to_string(&response).unwrap().into()))
                .await
                .is_err()
            {
                return;
            }
            auth
        }
        Err(e) => {
            let response = ServerMessage::AuthError { message: e };
            let _ = sender
                .send(Message::Text(serde_json::to_string(&response).unwrap().into()))
                .await;
            return;
        }
    };

    // Create connection with channel for outbound messages
    let (tx, mut rx) = mpsc::channel::<OutboundMessage>(100);
    let client_conn = Arc::new(ClientConnection::new(client_id, grants, is_admin, tx));

    let conn_id = client_conn.id;
    state.connections.add(client_conn.clone());

    info!(conn_id = %conn_id, "WebSocket client authenticated");

    // Spawn task to forward outbound messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            // Convert to text (our messages are JSON)
            if sender
                .send(Message::Text(String::from_utf8_lossy(&msg.payload).into_owned().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Process incoming messages
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                if let Ok(msg) = serde_json::from_str::<ClientMessage>(&text) {
                    handle_client_message(&msg, &client_conn, &state.connections, &state.router).await;
                }
            }
            Ok(Message::Binary(data)) => {
                if let Ok(msg) = serde_json::from_slice::<ClientMessage>(&data) {
                    handle_client_message(&msg, &client_conn, &state.connections, &state.router).await;
                }
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                // Handled automatically by axum
            }
            Ok(Message::Close(_)) => break,
            Err(e) => {
                warn!(conn_id = %conn_id, error = %e, "WebSocket error");
                break;
            }
        }
    }

    // Cleanup
    debug!(conn_id = %conn_id, "WebSocket client disconnected");
    state.connections.remove(conn_id);
    send_task.abort();
}

async fn authenticate(
    token: &str,
    state: &WsState,
) -> Result<(Option<String>, GrantSet, bool), String> {
    match Token::parse(token, &state.token_secret, state.admin_key.as_ref()) {
        Ok(Token::Admin) => Ok((None, GrantSet::new(), true)),
        Ok(Token::Sub(sub_token)) => {
            let client_id = sub_token.client_id().to_string();
            let grants = sub_token.grants().clone();
            Ok((Some(client_id), grants, false))
        }
        Ok(Token::Role(role_token)) => {
            match state.role_store.get_grants(role_token.id()).await {
                Ok(grants) => Ok((None, grants, false)),
                Err(e) => Err(format!("Role lookup failed: {}", e)),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn handle_client_message(
    msg: &ClientMessage,
    conn: &Arc<ClientConnection>,
    connections: &ConnectionManager,
    router: &TopicRouter,
) {
    match msg {
        ClientMessage::Auth { .. } => {
            // Already authenticated, ignore
        }
        ClientMessage::Subscribe { pattern } => {
            match ChannelPattern::parse(pattern) {
                Ok(p) => {
                    if conn.subscribe(p.clone()) {
                        connections.index_subscription(conn.id, &p);
                        let response = ServerMessage::Subscribed { pattern: pattern.clone() };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    } else {
                        let response = ServerMessage::SubscribeError {
                            pattern: pattern.clone(),
                            message: "Permission denied".to_string(),
                        };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    }
                }
                Err(e) => {
                    let response = ServerMessage::SubscribeError {
                        pattern: pattern.clone(),
                        message: e.to_string(),
                    };
                    let _ = conn.send(OutboundMessage {
                        channel: String::new(),
                        payload: serde_json::to_vec(&response).unwrap(),
                    });
                }
            }
        }
        ClientMessage::Unsubscribe { pattern } => {
            if let Ok(p) = ChannelPattern::parse(pattern) {
                conn.unsubscribe(&p);
                let response = ServerMessage::Unsubscribed { pattern: pattern.clone() };
                let _ = conn.send(OutboundMessage {
                    channel: String::new(),
                    payload: serde_json::to_vec(&response).unwrap(),
                });
            }
        }
        ClientMessage::Publish { channel, payload } => {
            match Channel::parse(channel) {
                Ok(ch) => {
                    let allowed = conn.is_admin || conn.grants.can_write(&ch);

                    if allowed {
                        let msg_bytes = serde_json::to_vec(&ServerMessage::Message {
                            channel: channel.clone(),
                            payload: payload.clone(),
                        })
                        .unwrap();

                        router.route(&ch, msg_bytes).await;

                        let response = ServerMessage::Published { channel: channel.clone() };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    } else {
                        let response = ServerMessage::PublishError {
                            channel: channel.clone(),
                            message: "Permission denied".to_string(),
                        };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    }
                }
                Err(e) => {
                    let response = ServerMessage::PublishError {
                        channel: channel.clone(),
                        message: e.to_string(),
                    };
                    let _ = conn.send(OutboundMessage {
                        channel: String::new(),
                        payload: serde_json::to_vec(&response).unwrap(),
                    });
                }
            }
        }
        ClientMessage::YjsUpdate { channel, update, origin } => {
            match Channel::parse(channel) {
                Ok(ch) => {
                    let allowed = conn.is_admin || conn.grants.can_write(&ch);

                    if allowed {
                        let msg_bytes = serde_json::to_vec(&ServerMessage::YjsUpdate {
                            channel: channel.clone(),
                            update: update.clone(),
                            origin: origin.clone(),
                        })
                        .unwrap();

                        router.route(&ch, msg_bytes).await;

                        let response = ServerMessage::Published { channel: channel.clone() };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    } else {
                        let response = ServerMessage::PublishError {
                            channel: channel.clone(),
                            message: "Permission denied".to_string(),
                        };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        });
                    }
                }
                Err(e) => {
                    let response = ServerMessage::PublishError {
                        channel: channel.clone(),
                        message: e.to_string(),
                    };
                    let _ = conn.send(OutboundMessage {
                        channel: String::new(),
                        payload: serde_json::to_vec(&response).unwrap(),
                    });
                }
            }
        }
        ClientMessage::YjsAwareness { channel, update } => {
            if let Ok(ch) = Channel::parse(channel) {
                let msg_bytes = serde_json::to_vec(&ServerMessage::YjsAwareness {
                    channel: channel.clone(),
                    update: update.clone(),
                })
                .unwrap();

                router.route(&ch, msg_bytes).await;
            }
        }
        ClientMessage::YjsSyncRequest { channel, state_vector } => {
            if let Ok(_ch) = Channel::parse(channel) {
                let msg_bytes = serde_json::to_vec(&ServerMessage::YjsSyncResponse {
                    channel: channel.clone(),
                    update: state_vector.clone(),
                })
                .unwrap();

                let _ = conn.send(OutboundMessage {
                    channel: String::new(),
                    payload: msg_bytes,
                });
            }
        }
        ClientMessage::Ping { seq } => {
            let response = ServerMessage::Pong { seq: *seq };
            let _ = conn.send(OutboundMessage {
                channel: String::new(),
                payload: serde_json::to_vec(&response).unwrap(),
            });
        }
    }
}

/// Run the WebSocket server
pub async fn run_websocket_server(bind_addr: SocketAddr, state: WsState) -> anyhow::Result<()> {
    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(addr = %bind_addr, "WebSocket server listening");

    axum::serve(listener, app).await?;

    Ok(())
}
