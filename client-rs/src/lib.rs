//! Catbus Rust Client
//!
//! A WebTransport client for the Catbus message bus, providing real-time pub/sub
//! messaging with support for auto-reconnection and request/response patterns.
//!
//! # Example
//!
//! ```no_run
//! use catbus_client::{CatbusClient, CatbusConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = CatbusConfig::new("https://localhost:4433", "your-token");
//!     let client = CatbusClient::new(config);
//!
//!     client.connect().await?;
//!
//!     // Subscribe to messages
//!     let sub = client.subscribe("runner.task.*", |channel, payload| {
//!         println!("Received on {}: {:?}", channel, payload);
//!     }).await?;
//!
//!     // Publish a message
//!     client.publish("runner.task.123.status", &serde_json::json!({"status": "running"})).await?;
//!
//!     // Request/response pattern with correlation ID
//!     let response: serde_json::Value = client.request(
//!         "runner.task.123.assign",
//!         &serde_json::json!({"task_id": "123"}),
//!         "runner.task.123.reply",
//!     ).await?;
//!
//!     Ok(())
//! }
//! ```

mod client;
mod config;
mod error;
mod messages;

pub use client::{CatbusClient, ConnectionState, MessageHandler, Subscription};
pub use config::CatbusConfig;
pub use error::CatbusError;
pub use messages::{ClientMessage, ServerMessage};
