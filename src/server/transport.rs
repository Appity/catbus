//! WebTransport server implementation

use crate::auth::{AdminKey, GrantSet, Token};
use crate::channels::{Channel, ChannelPattern};
use crate::server::connections::{ClientConnection, ConnectionManager, OutboundMessage};
use crate::server::router::TopicRouter;
use crate::storage::RoleStore;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use wtransport::endpoint::IncomingSession;
use wtransport::{Endpoint, Identity, ServerConfig};

/// Messages from client to server
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Authenticate with a token
    Auth { token: String },
    /// Subscribe to a channel pattern
    Subscribe { pattern: String },
    /// Unsubscribe from a channel pattern
    Unsubscribe { pattern: String },
    /// Publish a message to a channel
    Publish { channel: String, payload: serde_json::Value },
    /// Ping for keepalive
    Ping { seq: u64 },
}

/// Messages from server to client
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Authentication successful
    AuthOk { client_id: Option<String> },
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
    Message { channel: String, payload: serde_json::Value },
    /// Pong response
    Pong { seq: u64 },
    /// Generic error
    Error { message: String },
}

/// Catbus server configuration
pub struct CatbusServerConfig {
    /// Address to bind to
    pub bind_addr: SocketAddr,
    /// TLS certificate (PEM)
    pub cert_pem: String,
    /// TLS private key (PEM)
    pub key_pem: String,
    /// Secret for signing stateless tokens
    pub token_secret: Vec<u8>,
    /// Admin key for full access
    pub admin_key: Option<AdminKey>,
}

/// The main Catbus server
pub struct CatbusServer {
    config: CatbusServerConfig,
    connections: Arc<ConnectionManager>,
    router: Arc<TopicRouter>,
    role_store: Arc<dyn RoleStore>,
}

impl CatbusServer {
    pub fn new(config: CatbusServerConfig, role_store: Arc<dyn RoleStore>) -> Self {
        let connections = Arc::new(ConnectionManager::new());
        let router = Arc::new(TopicRouter::new(connections.clone()));

        Self {
            config,
            connections,
            router,
            role_store,
        }
    }

    /// Get a reference to the topic router (for publishing from outside)
    pub fn router(&self) -> Arc<TopicRouter> {
        self.router.clone()
    }

    /// Get connection count
    pub fn connection_count(&self) -> usize {
        self.connections.count()
    }

    /// Run the server
    pub async fn run(&self) -> Result<()> {
        // Use self-signed certificate for now
        // TODO: Support custom certificates via PEM/DER
        let identity = Identity::self_signed(["localhost", "127.0.0.1", &self.config.bind_addr.ip().to_string()])
            .context("Failed to create self-signed identity")?;

        let config = ServerConfig::builder()
            .with_bind_address(self.config.bind_addr)
            .with_identity(identity)
            .build();

        let endpoint = Endpoint::server(config)?;

        info!(addr = %self.config.bind_addr, "Catbus server listening");

        loop {
            let incoming = endpoint.accept().await;
            let connections = self.connections.clone();
            let router = self.router.clone();
            let role_store = self.role_store.clone();
            let token_secret = self.config.token_secret.clone();
            let admin_key = self.config.admin_key.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_session(
                    incoming,
                    connections,
                    router,
                    role_store,
                    token_secret,
                    admin_key,
                )
                .await
                {
                    warn!(error = %e, "Session error");
                }
            });
        }
    }

    /// Publish a message (called from Postgres NOTIFY handler or API)
    pub async fn publish(&self, channel: &str, payload: serde_json::Value) -> Result<usize> {
        let channel = Channel::parse(channel).context("Invalid channel name")?;
        let payload_bytes = serde_json::to_vec(&ServerMessage::Message {
            channel: channel.to_string(),
            payload,
        })?;

        Ok(self.router.route(&channel, payload_bytes).await)
    }
}

async fn handle_session(
    incoming: IncomingSession,
    connections: Arc<ConnectionManager>,
    router: Arc<TopicRouter>,
    role_store: Arc<dyn RoleStore>,
    token_secret: Vec<u8>,
    admin_key: Option<AdminKey>,
) -> Result<()> {
    let session_request = incoming.await?;
    let connection = session_request.accept().await?;

    debug!("New WebTransport connection");

    // Wait for bidirectional stream (client will open one for message exchange)
    let (mut send, mut recv) = connection.accept_bi().await?;

    // Channel for outbound messages
    let (tx, mut rx) = mpsc::channel::<OutboundMessage>(100);

    // Client must authenticate first
    let client_conn = {
        let mut buf = vec![0u8; 65536];
        let n = recv.read(&mut buf).await?.ok_or_else(|| anyhow::anyhow!("Connection closed before auth"))?;

        let msg: ClientMessage = serde_json::from_slice(&buf[..n])
            .context("Invalid message format")?;

        match msg {
            ClientMessage::Auth { token } => {
                match Token::parse(&token, &token_secret, admin_key.as_ref()) {
                    Ok(Token::Admin) => {
                        let response = ServerMessage::AuthOk { client_id: None };
                        send.write_all(&serde_json::to_vec(&response)?).await?;
                        Arc::new(ClientConnection::new(None, GrantSet::new(), true, tx))
                    }
                    Ok(Token::Sub(sub_token)) => {
                        let client_id = sub_token.client_id().to_string();
                        let grants = sub_token.grants().clone();
                        let response = ServerMessage::AuthOk { client_id: Some(client_id.clone()) };
                        send.write_all(&serde_json::to_vec(&response)?).await?;
                        Arc::new(ClientConnection::new(Some(client_id), grants, false, tx))
                    }
                    Ok(Token::Role(role_token)) => {
                        // Look up grants in store
                        match role_store.get_grants(role_token.id()).await {
                            Ok(grants) => {
                                let response = ServerMessage::AuthOk { client_id: None };
                                send.write_all(&serde_json::to_vec(&response)?).await?;
                                Arc::new(ClientConnection::new(None, grants, false, tx))
                            }
                            Err(e) => {
                                let response = ServerMessage::AuthError {
                                    message: format!("Role lookup failed: {}", e),
                                };
                                send.write_all(&serde_json::to_vec(&response)?).await?;
                                return Ok(());
                            }
                        }
                    }
                    Err(e) => {
                        let response = ServerMessage::AuthError {
                            message: e.to_string(),
                        };
                        send.write_all(&serde_json::to_vec(&response)?).await?;
                        return Ok(());
                    }
                }
            }
            _ => {
                let response = ServerMessage::Error {
                    message: "Must authenticate first".to_string(),
                };
                send.write_all(&serde_json::to_vec(&response)?).await?;
                return Ok(());
            }
        }
    };

    let conn_id = client_conn.id;
    connections.add(client_conn.clone());

    info!(conn_id = %conn_id, "Client authenticated");

    // Spawn task to forward outbound messages
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if send.write_all(&msg.payload).await.is_err() {
                break;
            }
        }
    });

    // Process incoming messages
    let mut buf = vec![0u8; 65536];
    loop {
        match recv.read(&mut buf).await {
            Ok(Some(n)) => {
                let msg: ClientMessage = match serde_json::from_slice(&buf[..n]) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(error = %e, "Invalid message from client");
                        continue;
                    }
                };

                handle_client_message(
                    &msg,
                    &client_conn,
                    &connections,
                    &router,
                )
                .await;
            }
            Ok(None) => {
                debug!(conn_id = %conn_id, "Client disconnected");
                break;
            }
            Err(e) => {
                warn!(conn_id = %conn_id, error = %e, "Read error");
                break;
            }
        }
    }

    // Cleanup
    connections.remove(conn_id);
    send_task.abort();

    Ok(())
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
                        }).await;
                    } else {
                        let response = ServerMessage::SubscribeError {
                            pattern: pattern.clone(),
                            message: "Permission denied".to_string(),
                        };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        }).await;
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
                    }).await;
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
                }).await;
            }
        }
        ClientMessage::Publish { channel, payload } => {
            match Channel::parse(channel) {
                Ok(ch) => {
                    // Check write permission
                    let allowed = conn.is_admin || conn.grants.can_write(&ch);

                    if allowed {
                        let msg_bytes = serde_json::to_vec(&ServerMessage::Message {
                            channel: channel.clone(),
                            payload: payload.clone(),
                        }).unwrap();

                        router.route(&ch, msg_bytes).await;

                        let response = ServerMessage::Published { channel: channel.clone() };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        }).await;
                    } else {
                        let response = ServerMessage::PublishError {
                            channel: channel.clone(),
                            message: "Permission denied".to_string(),
                        };
                        let _ = conn.send(OutboundMessage {
                            channel: String::new(),
                            payload: serde_json::to_vec(&response).unwrap(),
                        }).await;
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
                    }).await;
                }
            }
        }
        ClientMessage::Ping { seq } => {
            let response = ServerMessage::Pong { seq: *seq };
            let _ = conn.send(OutboundMessage {
                channel: String::new(),
                payload: serde_json::to_vec(&response).unwrap(),
            }).await;
        }
    }
}
