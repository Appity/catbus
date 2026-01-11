//! Error types for Catbus client

use thiserror::Error;

/// Errors that can occur when using the Catbus client
#[derive(Error, Debug)]
pub enum CatbusError {
    /// Connection to the server failed
    #[error("Connection error: {0}")]
    Connection(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Not currently connected to the server
    #[error("Not connected")]
    NotConnected,

    /// Operation timed out
    #[error("Operation timed out")]
    Timeout,

    /// Server returned an error
    #[error("Server error: {0}")]
    Server(String),

    /// Failed to serialize/deserialize message
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// WebTransport error
    #[error("Transport error: {0}")]
    Transport(String),

    /// Permission denied for the requested operation
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Invalid channel or pattern
    #[error("Invalid channel: {0}")]
    InvalidChannel(String),

    /// The client has been shut down
    #[error("Client shut down")]
    Shutdown,
}

/// Result type for Catbus operations
pub type Result<T> = std::result::Result<T, CatbusError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_connection() {
        let err = CatbusError::Connection("failed to connect".to_string());
        assert_eq!(err.to_string(), "Connection error: failed to connect");
    }

    #[test]
    fn test_error_display_authentication() {
        let err = CatbusError::Authentication("invalid token".to_string());
        assert_eq!(err.to_string(), "Authentication failed: invalid token");
    }

    #[test]
    fn test_error_display_not_connected() {
        let err = CatbusError::NotConnected;
        assert_eq!(err.to_string(), "Not connected");
    }

    #[test]
    fn test_error_display_timeout() {
        let err = CatbusError::Timeout;
        assert_eq!(err.to_string(), "Operation timed out");
    }

    #[test]
    fn test_error_display_server() {
        let err = CatbusError::Server("internal error".to_string());
        assert_eq!(err.to_string(), "Server error: internal error");
    }

    #[test]
    fn test_error_display_transport() {
        let err = CatbusError::Transport("stream closed".to_string());
        assert_eq!(err.to_string(), "Transport error: stream closed");
    }

    #[test]
    fn test_error_display_permission_denied() {
        let err = CatbusError::PermissionDenied("cannot write to channel".to_string());
        assert_eq!(err.to_string(), "Permission denied: cannot write to channel");
    }

    #[test]
    fn test_error_display_invalid_channel() {
        let err = CatbusError::InvalidChannel("bad!channel".to_string());
        assert_eq!(err.to_string(), "Invalid channel: bad!channel");
    }

    #[test]
    fn test_error_display_shutdown() {
        let err = CatbusError::Shutdown;
        assert_eq!(err.to_string(), "Client shut down");
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_err = serde_json::from_str::<String>("not valid json").unwrap_err();
        let err: CatbusError = json_err.into();
        assert!(matches!(err, CatbusError::Serialization(_)));
        assert!(err.to_string().starts_with("Serialization error:"));
    }

    #[test]
    fn test_result_type_ok() {
        let result: Result<i32> = Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_result_type_err() {
        let result: Result<i32> = Err(CatbusError::NotConnected);
        assert!(result.is_err());
    }

    #[test]
    fn test_error_debug() {
        let err = CatbusError::Connection("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("Connection"));
        assert!(debug.contains("test"));
    }
}
