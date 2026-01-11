//! Catbus client implementation

use crate::config::CatbusConfig;
use crate::error::{CatbusError, Result};
use crate::messages::{ClientMessage, ServerMessage};

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use wtransport::endpoint::endpoint_side::Client;
use wtransport::{ClientConfig, Endpoint};

/// Connection state of the client
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected to the server
    Disconnected,
    /// Attempting to connect
    Connecting,
    /// Authenticating with the server
    Authenticating,
    /// Connected and ready
    Connected,
    /// Attempting to reconnect after disconnect
    Reconnecting,
}

/// Handler for incoming messages
pub type MessageHandler = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// An active subscription that can be unsubscribed
pub struct Subscription {
    pattern: String,
    client: Arc<ClientInner>,
    handler_id: Uuid,
}

impl Subscription {
    /// Get the subscription pattern
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Unsubscribe from this subscription
    pub async fn unsubscribe(self) -> Result<()> {
        self.client.remove_handler(&self.pattern, self.handler_id).await
    }
}

/// Internal client state
struct ClientInner {
    config: CatbusConfig,
    state: watch::Sender<ConnectionState>,
    state_rx: watch::Receiver<ConnectionState>,
    client_id: Mutex<Option<String>>,

    // Channel for sending messages to the connection task
    tx: Mutex<Option<mpsc::Sender<OutboundMessage>>>,

    // Subscriptions: pattern -> list of (handler_id, handler)
    subscriptions: Mutex<HashMap<String, Vec<(Uuid, MessageHandler)>>>,

    // Pending operations awaiting responses
    pending_subscribes: Mutex<HashMap<String, oneshot::Sender<Result<()>>>>,
    pending_unsubscribes: Mutex<HashMap<String, oneshot::Sender<Result<()>>>>,
    pending_publishes: Mutex<HashMap<String, oneshot::Sender<Result<()>>>>,
    pending_pings: Mutex<HashMap<u64, oneshot::Sender<()>>>,

    // Request/response correlation
    pending_requests: Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>,

    // Counters
    ping_seq: AtomicU64,

    // Shutdown signal
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

/// Message to send to the connection task
enum OutboundMessage {
    Send(ClientMessage),
    Shutdown,
}

/// Catbus WebTransport client
///
/// Provides pub/sub messaging with automatic reconnection and request/response patterns.
/// This struct is cheaply cloneable as it uses an internal Arc.
#[derive(Clone)]
pub struct CatbusClient {
    inner: Arc<ClientInner>,
}

impl CatbusClient {
    /// Create a new Catbus client with the given configuration
    pub fn new(config: CatbusConfig) -> Self {
        let (state_tx, state_rx) = watch::channel(ConnectionState::Disconnected);

        let inner = Arc::new(ClientInner {
            config,
            state: state_tx,
            state_rx,
            client_id: Mutex::new(None),
            tx: Mutex::new(None),
            subscriptions: Mutex::new(HashMap::new()),
            pending_subscribes: Mutex::new(HashMap::new()),
            pending_unsubscribes: Mutex::new(HashMap::new()),
            pending_publishes: Mutex::new(HashMap::new()),
            pending_pings: Mutex::new(HashMap::new()),
            pending_requests: Mutex::new(HashMap::new()),
            ping_seq: AtomicU64::new(0),
            shutdown: Mutex::new(None),
        });

        Self { inner }
    }

    /// Get the current connection state
    pub fn connection_state(&self) -> ConnectionState {
        *self.inner.state_rx.borrow()
    }

    /// Get a receiver for connection state changes
    pub fn state_receiver(&self) -> watch::Receiver<ConnectionState> {
        self.inner.state_rx.clone()
    }

    /// Get the client ID (if authenticated with a sub token)
    pub fn client_id(&self) -> Option<String> {
        self.inner.client_id.lock().clone()
    }

    /// Connect to the Catbus server
    pub async fn connect(&self) -> Result<()> {
        let current_state = self.connection_state();
        if current_state != ConnectionState::Disconnected
            && current_state != ConnectionState::Reconnecting
        {
            return Err(CatbusError::Connection(format!(
                "Cannot connect in state: {:?}",
                current_state
            )));
        }

        self.inner.set_state(ConnectionState::Connecting);

        // Spawn connection task
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = connection_task(inner.clone()).await {
                error!("Connection task error: {}", e);
                inner.handle_disconnect().await;
            }
        });

        // Wait for connected state or error
        let mut state_rx = self.inner.state_rx.clone();
        loop {
            if state_rx.changed().await.is_err() {
                return Err(CatbusError::Shutdown);
            }

            match *state_rx.borrow() {
                ConnectionState::Connected => return Ok(()),
                ConnectionState::Disconnected => {
                    return Err(CatbusError::Connection("Connection failed".into()));
                }
                _ => continue,
            }
        }
    }

    /// Disconnect from the server
    pub async fn disconnect(&self) -> Result<()> {
        if let Some(tx) = self.inner.tx.lock().take() {
            let _ = tx.send(OutboundMessage::Shutdown).await;
        }

        if let Some(shutdown) = self.inner.shutdown.lock().take() {
            let _ = shutdown.send(());
        }

        self.inner.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    /// Subscribe to a channel pattern
    ///
    /// The handler will be called for each message matching the pattern.
    /// Returns a `Subscription` that can be used to unsubscribe.
    pub async fn subscribe<F>(&self, pattern: &str, handler: F) -> Result<Subscription>
    where
        F: Fn(&str, serde_json::Value) + Send + Sync + 'static,
    {
        let handler_id = Uuid::new_v4();
        let handler: MessageHandler = Arc::new(handler);

        // Add handler to subscriptions
        let need_server_subscribe = {
            let mut subs = self.inner.subscriptions.lock();
            let handlers = subs.entry(pattern.to_string()).or_default();
            let first_handler = handlers.is_empty();
            handlers.push((handler_id, handler));
            first_handler
        };

        // Subscribe on server if this is the first handler for this pattern
        if need_server_subscribe && self.connection_state() == ConnectionState::Connected {
            self.inner.do_subscribe(pattern).await?;
        }

        Ok(Subscription {
            pattern: pattern.to_string(),
            client: self.inner.clone(),
            handler_id,
        })
    }

    /// Publish a message to a channel
    pub async fn publish<T: Serialize>(&self, channel: &str, payload: &T) -> Result<()> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(CatbusError::NotConnected);
        }

        let payload = serde_json::to_value(payload)?;
        self.inner.do_publish(channel, payload).await
    }

    /// Send a request and wait for a response (using correlation ID pattern)
    ///
    /// This publishes a message with a correlation ID to `request_channel` and
    /// waits for a response on `reply_channel` with the same correlation ID.
    pub async fn request<Req: Serialize, Res: DeserializeOwned>(
        &self,
        request_channel: &str,
        payload: &Req,
        reply_channel: &str,
    ) -> Result<Res> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(CatbusError::NotConnected);
        }

        let correlation_id = Uuid::new_v4().to_string();

        // Set up response listener before sending request
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending_requests.lock();
            pending.insert(correlation_id.clone(), response_tx);
        }

        // Ensure we're subscribed to the reply channel
        let sub = self
            .subscribe(reply_channel, {
                let inner = self.inner.clone();
                let correlation_id = correlation_id.clone();
                move |_channel, payload| {
                    // Check if this response matches our correlation ID
                    if let Some(cid) = payload.get("correlation_id").and_then(|v| v.as_str()) {
                        if cid == correlation_id {
                            let mut pending = inner.pending_requests.lock();
                            if let Some(tx) = pending.remove(cid) {
                                let _ = tx.send(payload);
                            }
                        }
                    }
                }
            })
            .await?;

        // Build request payload with correlation ID
        let mut request_payload = serde_json::to_value(payload)?;
        if let Some(obj) = request_payload.as_object_mut() {
            obj.insert(
                "correlation_id".to_string(),
                serde_json::Value::String(correlation_id.clone()),
            );
            obj.insert(
                "reply_channel".to_string(),
                serde_json::Value::String(reply_channel.to_string()),
            );
        }

        // Send request
        self.inner.do_publish(request_channel, request_payload).await?;

        // Wait for response with timeout
        let result = timeout(self.inner.config.operation_timeout, response_rx).await;

        // Cleanup subscription
        let _ = sub.unsubscribe().await;

        match result {
            Ok(Ok(response)) => {
                // Remove correlation_id from response before deserializing
                let mut response = response;
                if let Some(obj) = response.as_object_mut() {
                    obj.remove("correlation_id");
                    obj.remove("reply_channel");
                }
                serde_json::from_value(response).map_err(CatbusError::Serialization)
            }
            Ok(Err(_)) => Err(CatbusError::Shutdown),
            Err(_) => {
                // Cleanup on timeout
                self.inner.pending_requests.lock().remove(&correlation_id);
                Err(CatbusError::Timeout)
            }
        }
    }

    /// Send a ping and return the round-trip time in milliseconds
    pub async fn ping(&self) -> Result<u64> {
        if self.connection_state() != ConnectionState::Connected {
            return Err(CatbusError::NotConnected);
        }

        let seq = self.inner.ping_seq.fetch_add(1, Ordering::SeqCst);
        let start = std::time::Instant::now();

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending_pings.lock();
            pending.insert(seq, tx);
        }

        self.inner.send(ClientMessage::Ping { seq }).await?;

        match timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(())) => Ok(start.elapsed().as_millis() as u64),
            Ok(Err(_)) => Err(CatbusError::Shutdown),
            Err(_) => {
                self.inner.pending_pings.lock().remove(&seq);
                Err(CatbusError::Timeout)
            }
        }
    }
}

impl ClientInner {
    fn set_state(&self, state: ConnectionState) {
        let _ = self.state.send(state);
    }

    async fn send(&self, msg: ClientMessage) -> Result<()> {
        let tx = self.tx.lock().clone();
        if let Some(tx) = tx {
            tx.send(OutboundMessage::Send(msg))
                .await
                .map_err(|_| CatbusError::NotConnected)
        } else {
            Err(CatbusError::NotConnected)
        }
    }

    async fn do_subscribe(&self, pattern: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_subscribes.lock();
            pending.insert(pattern.to_string(), tx);
        }

        self.send(ClientMessage::Subscribe {
            pattern: pattern.to_string(),
        })
        .await?;

        match timeout(self.config.operation_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CatbusError::Shutdown),
            Err(_) => {
                self.pending_subscribes.lock().remove(pattern);
                Err(CatbusError::Timeout)
            }
        }
    }

    async fn do_unsubscribe(&self, pattern: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_unsubscribes.lock();
            pending.insert(pattern.to_string(), tx);
        }

        self.send(ClientMessage::Unsubscribe {
            pattern: pattern.to_string(),
        })
        .await?;

        match timeout(self.config.operation_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CatbusError::Shutdown),
            Err(_) => {
                self.pending_unsubscribes.lock().remove(pattern);
                Err(CatbusError::Timeout)
            }
        }
    }

    async fn do_publish(&self, channel: &str, payload: serde_json::Value) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_publishes.lock();
            pending.insert(channel.to_string(), tx);
        }

        self.send(ClientMessage::Publish {
            channel: channel.to_string(),
            payload,
        })
        .await?;

        match timeout(self.config.operation_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CatbusError::Shutdown),
            Err(_) => {
                self.pending_publishes.lock().remove(channel);
                Err(CatbusError::Timeout)
            }
        }
    }

    async fn remove_handler(&self, pattern: &str, handler_id: Uuid) -> Result<()> {
        let should_unsubscribe = {
            let mut subs = self.subscriptions.lock();
            if let Some(handlers) = subs.get_mut(pattern) {
                handlers.retain(|(id, _)| *id != handler_id);
                if handlers.is_empty() {
                    subs.remove(pattern);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_unsubscribe && *self.state_rx.borrow() == ConnectionState::Connected {
            self.do_unsubscribe(pattern).await?;
        }

        Ok(())
    }

    fn handle_message(&self, msg: ServerMessage) {
        match msg {
            ServerMessage::Subscribed { pattern } => {
                if let Some(tx) = self.pending_subscribes.lock().remove(&pattern) {
                    let _ = tx.send(Ok(()));
                }
            }
            ServerMessage::SubscribeError { pattern, message } => {
                if let Some(tx) = self.pending_subscribes.lock().remove(&pattern) {
                    let _ = tx.send(Err(CatbusError::PermissionDenied(message)));
                }
            }
            ServerMessage::Unsubscribed { pattern } => {
                if let Some(tx) = self.pending_unsubscribes.lock().remove(&pattern) {
                    let _ = tx.send(Ok(()));
                }
            }
            ServerMessage::Published { channel } => {
                if let Some(tx) = self.pending_publishes.lock().remove(&channel) {
                    let _ = tx.send(Ok(()));
                }
            }
            ServerMessage::PublishError { channel, message } => {
                if let Some(tx) = self.pending_publishes.lock().remove(&channel) {
                    let _ = tx.send(Err(CatbusError::PermissionDenied(message)));
                }
            }
            ServerMessage::Message { channel, payload } => {
                self.dispatch_message(&channel, payload);
            }
            ServerMessage::Pong { seq } => {
                if let Some(tx) = self.pending_pings.lock().remove(&seq) {
                    let _ = tx.send(());
                }
            }
            ServerMessage::Error { message } => {
                warn!("Server error: {}", message);
            }
            // Yjs messages - forward to handlers
            ServerMessage::YjsUpdate {
                channel,
                update,
                origin,
            } => {
                let payload = serde_json::json!({
                    "type": "yjs_update",
                    "update": update,
                    "origin": origin,
                });
                self.dispatch_message(&channel, payload);
            }
            ServerMessage::YjsAwareness { channel, update } => {
                let payload = serde_json::json!({
                    "type": "yjs_awareness",
                    "update": update,
                });
                self.dispatch_message(&channel, payload);
            }
            ServerMessage::YjsSyncResponse { channel, update } => {
                let payload = serde_json::json!({
                    "type": "yjs_sync_response",
                    "update": update,
                });
                self.dispatch_message(&channel, payload);
            }
            _ => {}
        }
    }

    fn dispatch_message(&self, channel: &str, payload: serde_json::Value) {
        let subs = self.subscriptions.lock();

        for (pattern, handlers) in subs.iter() {
            if pattern_matches(pattern, channel) {
                for (_, handler) in handlers {
                    let handler = handler.clone();
                    let channel = channel.to_string();
                    let payload = payload.clone();
                    // Call handler - could spawn if handlers are slow
                    handler(&channel, payload);
                }
            }
        }

        // Also check pending requests for correlation ID
        if let Some(cid) = payload.get("correlation_id").and_then(|v| v.as_str()) {
            if let Some(tx) = self.pending_requests.lock().remove(cid) {
                let _ = tx.send(payload);
            }
        }
    }

    async fn resubscribe(&self) -> Result<()> {
        let patterns: Vec<String> = {
            let subs = self.subscriptions.lock();
            subs.keys().cloned().collect()
        };

        for pattern in patterns {
            if let Err(e) = self.do_subscribe(&pattern).await {
                warn!("Failed to resubscribe to {}: {}", pattern, e);
            }
        }

        Ok(())
    }

    async fn handle_disconnect(&self) {
        // Clear sender
        *self.tx.lock() = None;

        // Reject pending operations
        for (_, tx) in self.pending_subscribes.lock().drain() {
            let _ = tx.send(Err(CatbusError::NotConnected));
        }
        for (_, tx) in self.pending_unsubscribes.lock().drain() {
            let _ = tx.send(Err(CatbusError::NotConnected));
        }
        for (_, tx) in self.pending_publishes.lock().drain() {
            let _ = tx.send(Err(CatbusError::NotConnected));
        }
        for (_, tx) in self.pending_pings.lock().drain() {
            let _ = tx.send(());
        }
        for (_, tx) in self.pending_requests.lock().drain() {
            let _ = tx.send(serde_json::Value::Null);
        }

        if self.config.auto_reconnect {
            self.set_state(ConnectionState::Reconnecting);
            self.schedule_reconnect();
        } else {
            self.set_state(ConnectionState::Disconnected);
        }
    }

    fn schedule_reconnect(&self) {
        let inner = self.clone_arc();
        tokio::spawn(async move {
            let mut attempt = 0u32;

            loop {
                let delay = std::cmp::min(
                    inner.config.reconnect_delay * 2u32.saturating_pow(attempt),
                    inner.config.max_reconnect_delay,
                );

                info!("Reconnecting in {:?}...", delay);
                tokio::time::sleep(delay).await;

                if *inner.state_rx.borrow() == ConnectionState::Disconnected {
                    // Manual disconnect, stop reconnecting
                    break;
                }

                inner.set_state(ConnectionState::Connecting);

                match connection_task(inner.clone()).await {
                    Ok(()) => break, // Connection task will handle state
                    Err(e) => {
                        warn!("Reconnect attempt {} failed: {}", attempt + 1, e);
                        attempt += 1;
                        inner.set_state(ConnectionState::Reconnecting);
                    }
                }
            }
        });
    }

    fn clone_arc(&self) -> Arc<Self> {
        // This is a bit hacky but works - we need to get Arc<Self> from &Self
        // In practice, this is always called on an Arc<ClientInner>
        unsafe {
            let ptr = self as *const Self;
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    }
}

/// Check if a pattern matches a channel
fn pattern_matches(pattern: &str, channel: &str) -> bool {
    if pattern == channel {
        return true;
    }

    if pattern.ends_with(".*") {
        let prefix = &pattern[..pattern.len() - 1]; // Keep the dot
        return channel.starts_with(prefix);
    }

    if pattern == "*" {
        return true;
    }

    false
}

/// Main connection task
async fn connection_task(inner: Arc<ClientInner>) -> Result<()> {
    // Build client config based on certificate verification setting
    let config = if inner.config.dangerous_skip_cert_verify {
        ClientConfig::builder()
            .with_bind_default()
            .with_no_cert_validation()
            .build()
    } else {
        ClientConfig::builder()
            .with_bind_default()
            .with_native_certs()
            .build()
    };

    let endpoint: Endpoint<Client> = Endpoint::client(config)
        .map_err(|e| CatbusError::Connection(e.to_string()))?;

    // Connect to server
    debug!("Connecting to {}", inner.config.url);
    let connection = endpoint
        .connect(&inner.config.url)
        .await
        .map_err(|e| CatbusError::Connection(e.to_string()))?;

    // Open bidirectional stream
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| CatbusError::Transport(e.to_string()))?
        .await
        .map_err(|e| CatbusError::Transport(e.to_string()))?;

    // Authenticate
    inner.set_state(ConnectionState::Authenticating);

    let auth_msg = ClientMessage::Auth {
        token: inner.config.token.clone(),
    };
    let auth_bytes = serde_json::to_vec(&auth_msg)?;
    send.write_all(&auth_bytes)
        .await
        .map_err(|e| CatbusError::Transport(e.to_string()))?;

    // Wait for auth response
    let mut buf = vec![0u8; 65536];
    let n = recv
        .read(&mut buf)
        .await
        .map_err(|e| CatbusError::Transport(e.to_string()))?
        .ok_or_else(|| CatbusError::Connection("Connection closed during auth".into()))?;

    let response: ServerMessage = serde_json::from_slice(&buf[..n])?;

    match response {
        ServerMessage::AuthOk { client_id } => {
            *inner.client_id.lock() = client_id.clone();
            info!(client_id = ?client_id, "Authenticated");
        }
        ServerMessage::AuthError { message } => {
            return Err(CatbusError::Authentication(message));
        }
        _ => {
            return Err(CatbusError::Connection("Unexpected auth response".into()));
        }
    }

    // Set up channels for outbound messages
    let (tx, mut rx) = mpsc::channel::<OutboundMessage>(100);
    *inner.tx.lock() = Some(tx);

    // Set connected state
    inner.set_state(ConnectionState::Connected);

    // Resubscribe to existing subscriptions
    if let Err(e) = inner.resubscribe().await {
        warn!("Failed to resubscribe: {}", e);
    }

    // Shutdown channel
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    *inner.shutdown.lock() = Some(shutdown_tx);

    // Spawn ping task
    let ping_inner = inner.clone();
    let ping_interval = inner.config.ping_interval;
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(ping_interval);
        loop {
            interval.tick().await;
            if *ping_inner.state_rx.borrow() != ConnectionState::Connected {
                break;
            }
            let seq = ping_inner.ping_seq.fetch_add(1, Ordering::SeqCst);
            if let Err(e) = ping_inner.send(ClientMessage::Ping { seq }).await {
                warn!("Ping failed: {}", e);
                break;
            }
        }
    });

    // Main loop
    let result: Result<()> = loop {
        tokio::select! {
            // Handle outbound messages
            msg = rx.recv() => {
                match msg {
                    Some(OutboundMessage::Send(client_msg)) => {
                        let bytes = serde_json::to_vec(&client_msg)?;
                        if let Err(e) = send.write_all(&bytes).await {
                            break Err(CatbusError::Transport(e.to_string()));
                        }
                    }
                    Some(OutboundMessage::Shutdown) | None => {
                        break Ok(());
                    }
                }
            }

            // Handle inbound messages
            result = recv.read(&mut buf) => {
                match result {
                    Ok(Some(n)) => {
                        match serde_json::from_slice::<ServerMessage>(&buf[..n]) {
                            Ok(msg) => inner.handle_message(msg),
                            Err(e) => warn!("Failed to parse message: {}", e),
                        }
                    }
                    Ok(None) => {
                        debug!("Connection closed by server");
                        break Err(CatbusError::Connection("Connection closed".into()));
                    }
                    Err(e) => {
                        break Err(CatbusError::Transport(e.to_string()));
                    }
                }
            }

            // Handle shutdown
            _ = &mut shutdown_rx => {
                break Ok(());
            }
        }
    };

    // Cleanup
    ping_task.abort();
    *inner.tx.lock() = None;

    if result.is_err() {
        inner.handle_disconnect().await;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pattern matching tests
    #[test]
    fn test_pattern_matches_exact() {
        assert!(pattern_matches("foo.bar", "foo.bar"));
        assert!(!pattern_matches("foo.bar", "foo.baz"));
        assert!(!pattern_matches("foo.bar", "foo.bar.baz"));
        assert!(!pattern_matches("foo.bar.baz", "foo.bar"));
    }

    #[test]
    fn test_pattern_matches_wildcard() {
        // Trailing wildcard
        assert!(pattern_matches("foo.*", "foo.bar"));
        assert!(pattern_matches("foo.*", "foo.bar.baz"));
        assert!(pattern_matches("foo.*", "foo.x"));

        // Should not match different prefix
        assert!(!pattern_matches("foo.*", "baz.bar"));
        assert!(!pattern_matches("foo.*", "foobar"));

        // Exact match with wildcard pattern should still work
        assert!(!pattern_matches("foo.*", "foo"));
    }

    #[test]
    fn test_pattern_matches_global_wildcard() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("*", "foo.bar.baz"));
        assert!(pattern_matches("*", ""));
        assert!(pattern_matches("*", "a"));
    }

    #[test]
    fn test_pattern_matches_runner_channels() {
        // Test the runner channel patterns from OE-81
        assert!(pattern_matches("runner.task.*", "runner.task.123.assign"));
        assert!(pattern_matches("runner.task.*", "runner.task.456.reply"));
        assert!(pattern_matches("runner.plan.*", "runner.plan.789.status"));
        assert!(pattern_matches("runner.agent.*", "runner.agent.abc.status"));

        assert!(!pattern_matches("runner.task.*", "runner.plan.123.status"));
    }

    // Client state tests
    #[test]
    fn test_client_initial_state() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
        assert!(client.client_id().is_none());
    }

    #[test]
    fn test_connection_state_equality() {
        assert_eq!(ConnectionState::Disconnected, ConnectionState::Disconnected);
        assert_eq!(ConnectionState::Connected, ConnectionState::Connected);
        assert_ne!(ConnectionState::Disconnected, ConnectionState::Connected);
        assert_ne!(ConnectionState::Connecting, ConnectionState::Authenticating);
    }

    #[test]
    fn test_connection_state_copy() {
        let state = ConnectionState::Connected;
        let state_copy = state;
        assert_eq!(state, state_copy);
    }

    // Config integration tests
    #[test]
    fn test_client_with_custom_config() {
        use std::time::Duration;

        let config = CatbusConfig::new("https://example.com:4433", "my-token")
            .no_reconnect()
            .ping_interval(Duration::from_secs(10))
            .dangerous_skip_cert_verify();

        let client = CatbusClient::new(config);
        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    }

    // State receiver tests
    #[test]
    fn test_state_receiver() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        let rx = client.state_receiver();
        assert_eq!(*rx.borrow(), ConnectionState::Disconnected);
    }

    // Subscription struct tests
    #[test]
    fn test_subscription_pattern() {
        // We can't easily test Subscription without a full client connection,
        // but we can at least verify the pattern accessor would work
        // This is a compile-time check more than runtime
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;

    #[tokio::test]
    async fn test_publish_not_connected() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        let result = client.publish("foo.bar", &serde_json::json!({"test": true})).await;
        assert!(matches!(result, Err(CatbusError::NotConnected)));
    }

    #[tokio::test]
    async fn test_ping_not_connected() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        let result = client.ping().await;
        assert!(matches!(result, Err(CatbusError::NotConnected)));
    }

    #[tokio::test]
    async fn test_request_not_connected() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        let result: Result<serde_json::Value> = client
            .request("foo.request", &serde_json::json!({}), "foo.reply")
            .await;
        assert!(matches!(result, Err(CatbusError::NotConnected)));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = CatbusConfig::new("https://localhost:4433", "token");
        let client = CatbusClient::new(config);

        // Should not error when disconnecting while already disconnected
        let result = client.disconnect().await;
        assert!(result.is_ok());
    }
}
