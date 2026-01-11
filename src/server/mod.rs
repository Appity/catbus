//! Catbus server implementation
//!
//! Handles WebTransport and WebSocket connections, topic routing, and message fan-out.

mod connections;
mod router;
pub mod transport;
pub mod websocket;

pub use connections::{ClientConnection, ConnectionManager, OutboundMessage};
pub use router::TopicRouter;
pub use transport::{CatbusServer, CatbusServerConfig, ClientMessage, ServerMessage};
pub use websocket::{run_websocket_server, WsState};
