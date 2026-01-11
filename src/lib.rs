//! Catbus - WebTransport-based message bus for real-time state synchronization
//!
//! A high-performance message bus that uses WebTransport over HTTP/3 to provide
//! real-time communication between browsers, backend services, and workers.

pub mod auth;
pub mod channels;
pub mod server;
pub mod storage;

pub use auth::{AdminKey, Grant, GrantSet, GrantType, RoleToken, SubToken, Token};
pub use channels::{Channel, ChannelPattern};
pub use server::CatbusServer;
