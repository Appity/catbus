//! Topic routing and message fan-out

use crate::channels::Channel;
use crate::server::connections::{ConnectionManager, OutboundMessage};
use std::sync::Arc;
use tracing::debug;

/// Routes messages to subscribed clients
#[derive(Clone)]
pub struct TopicRouter {
    connections: Arc<ConnectionManager>,
}

impl TopicRouter {
    pub fn new(connections: Arc<ConnectionManager>) -> Self {
        Self { connections }
    }

    /// Route a message to all subscribed clients
    pub async fn route(&self, channel: &Channel, payload: Vec<u8>) -> usize {
        let subscribers = self.connections.find_subscribers(channel);
        let count = subscribers.len();

        if count == 0 {
            debug!(channel = %channel, "No subscribers for channel");
            return 0;
        }

        debug!(channel = %channel, subscriber_count = count, "Routing message");

        let msg = OutboundMessage {
            channel: channel.to_string(),
            payload,
        };

        for subscriber in subscribers {
            if let Err(e) = subscriber.send(msg.clone()) {
                // Log at debug level - this is expected during rapid disconnect
                debug!(
                    connection_id = %subscriber.id,
                    error = %e,
                    "Failed to send message to subscriber (buffer full or disconnected)"
                );
            }
        }

        count
    }

    /// Route a message to a specific client by client_id
    pub async fn route_to_client(&self, client_id: &str, channel: &Channel, payload: Vec<u8>) -> bool {
        let connections = self.connections.find_by_client_id(client_id);

        if connections.is_empty() {
            debug!(client_id = client_id, "No connections for client");
            return false;
        }

        let msg = OutboundMessage {
            channel: channel.to_string(),
            payload,
        };

        let mut sent = false;
        for conn in connections {
            if conn.should_receive(channel) {
                if conn.send(msg.clone()).is_ok() {
                    sent = true;
                }
            }
        }

        sent
    }

    /// Get the number of active connections
    pub fn connection_count(&self) -> usize {
        self.connections.count()
    }
}
