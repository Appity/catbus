//! Catbus server implementation
//!
//! Handles WebTransport connections, topic routing, and message fan-out.

mod connections;
mod router;
pub mod transport;

pub use connections::{ClientConnection, ConnectionManager, OutboundMessage};
pub use router::TopicRouter;
pub use transport::{CatbusServer, CatbusServerConfig};
