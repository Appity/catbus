//! Connection state management

use crate::auth::GrantSet;
use crate::channels::ChannelPattern;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Message to send to a client
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub channel: String,
    pub payload: Vec<u8>,
}

/// A connected client
pub struct ClientConnection {
    /// Unique connection ID
    pub id: Uuid,
    /// Client ID from token (for intrinsic channels)
    pub client_id: Option<String>,
    /// Grants for this connection
    pub grants: GrantSet,
    /// Channels this client is subscribed to
    pub subscriptions: RwLock<HashSet<ChannelPattern>>,
    /// Channel for sending messages to this client
    pub tx: mpsc::Sender<OutboundMessage>,
    /// Is this an admin connection?
    pub is_admin: bool,
}

impl ClientConnection {
    pub fn new(
        client_id: Option<String>,
        grants: GrantSet,
        is_admin: bool,
        tx: mpsc::Sender<OutboundMessage>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            client_id,
            grants,
            subscriptions: RwLock::new(HashSet::new()),
            tx,
            is_admin,
        }
    }

    /// Add a subscription (if allowed)
    pub fn subscribe(&self, pattern: ChannelPattern) -> bool {
        // Admin can subscribe to anything
        if self.is_admin {
            self.subscriptions.write().insert(pattern);
            return true;
        }

        // Check if any read grant covers this pattern
        // For now, simple check: the pattern must match one of our grant patterns
        let allowed = self.grants.patterns_for(crate::auth::GrantType::Read).iter().any(|g| {
            // Grant pattern must be equal or broader than subscription pattern
            // e.g., grant "project.*" allows subscribing to "project.abc.*" or "project.abc.xyz"
            let grant_str = g.to_string();
            let sub_str = pattern.to_string();

            if g.is_wildcard() {
                // Grant is wildcard, check if subscription is under it
                sub_str.starts_with(g.prefix()) &&
                    (g.prefix().is_empty() || sub_str.len() == g.prefix().len() || sub_str.as_bytes().get(g.prefix().len()) == Some(&b'.'))
            } else {
                // Exact match only
                grant_str == sub_str
            }
        });

        if allowed {
            self.subscriptions.write().insert(pattern);
        }

        allowed
    }

    /// Remove a subscription
    pub fn unsubscribe(&self, pattern: &ChannelPattern) {
        self.subscriptions.write().remove(pattern);
    }

    /// Check if this connection should receive a message on the given channel
    pub fn should_receive(&self, channel: &crate::channels::Channel) -> bool {
        if self.is_admin {
            // Admin receives if subscribed
            return self.subscriptions.read().iter().any(|p| p.matches(channel));
        }

        // Check subscriptions
        self.subscriptions.read().iter().any(|p| p.matches(channel))
    }

    /// Send a message to this client
    pub async fn send(&self, msg: OutboundMessage) -> Result<(), mpsc::error::SendError<OutboundMessage>> {
        self.tx.send(msg).await
    }
}

/// Manages all active connections
#[derive(Clone)]
pub struct ConnectionManager {
    /// All active connections by ID
    connections: Arc<DashMap<Uuid, Arc<ClientConnection>>>,
    /// Index: channel prefix -> connection IDs that might be interested
    /// This allows O(1) lookup for most common case (exact channel match)
    prefix_index: Arc<DashMap<String, HashSet<Uuid>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            prefix_index: Arc::new(DashMap::new()),
        }
    }

    /// Register a new connection
    pub fn add(&self, conn: Arc<ClientConnection>) {
        self.connections.insert(conn.id, conn);
    }

    /// Remove a connection
    pub fn remove(&self, id: Uuid) {
        if let Some((_, conn)) = self.connections.remove(&id) {
            // Clean up prefix index
            for pattern in conn.subscriptions.read().iter() {
                if let Some(mut set) = self.prefix_index.get_mut(pattern.prefix()) {
                    set.remove(&id);
                }
            }
        }
    }

    /// Get a connection by ID
    pub fn get(&self, id: Uuid) -> Option<Arc<ClientConnection>> {
        self.connections.get(&id).map(|r| r.clone())
    }

    /// Update prefix index when a client subscribes
    pub fn index_subscription(&self, conn_id: Uuid, pattern: &ChannelPattern) {
        self.prefix_index
            .entry(pattern.prefix().to_string())
            .or_insert_with(HashSet::new)
            .insert(conn_id);
    }

    /// Find all connections that should receive a message on the given channel
    pub fn find_subscribers(&self, channel: &crate::channels::Channel) -> Vec<Arc<ClientConnection>> {
        let mut result = Vec::new();

        // Check all connections (we can optimize this later with better indexing)
        for entry in self.connections.iter() {
            if entry.value().should_receive(channel) {
                result.push(entry.value().clone());
            }
        }

        result
    }

    /// Alias for find_subscribers (for test compatibility)
    pub fn get_subscribers_for_channel(&self, channel: &crate::channels::Channel) -> Vec<Arc<ClientConnection>> {
        self.find_subscribers(channel)
    }

    /// Get total connection count
    pub fn count(&self) -> usize {
        self.connections.len()
    }

    /// Get connection by client ID
    pub fn find_by_client_id(&self, client_id: &str) -> Vec<Arc<ClientConnection>> {
        self.connections
            .iter()
            .filter(|entry| entry.value().client_id.as_deref() == Some(client_id))
            .map(|entry| entry.value().clone())
            .collect()
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Grant, GrantType};

    #[test]
    fn test_connection_subscribe_allowed() {
        let (tx, _rx) = mpsc::channel(10);
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("project.abc.*").unwrap(),
        ));

        let conn = ClientConnection::new(Some("client-1".to_string()), grants, false, tx);

        // Should allow subscription under granted pattern
        assert!(conn.subscribe(ChannelPattern::parse("project.abc.*").unwrap()));
        assert!(conn.subscribe(ChannelPattern::parse("project.abc.updates").unwrap()));

        // Should deny subscription outside granted pattern
        assert!(!conn.subscribe(ChannelPattern::parse("project.xyz.*").unwrap()));
    }

    #[test]
    fn test_admin_subscribe_anything() {
        let (tx, _rx) = mpsc::channel(10);
        let conn = ClientConnection::new(None, GrantSet::new(), true, tx);

        // Admin can subscribe to anything
        assert!(conn.subscribe(ChannelPattern::parse("anything.*").unwrap()));
        assert!(conn.subscribe(ChannelPattern::parse("*").unwrap()));
    }
}
