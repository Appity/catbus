//! Configuration for the Catbus client

use std::time::Duration;

/// Configuration for connecting to a Catbus server
#[derive(Debug, Clone)]
pub struct CatbusConfig {
    /// Server URL (e.g., "https://localhost:4433")
    pub url: String,

    /// Authentication token (sub-*, role-*, or admin key)
    pub token: String,

    /// Whether to automatically reconnect on disconnect
    pub auto_reconnect: bool,

    /// Initial delay before reconnecting
    pub reconnect_delay: Duration,

    /// Maximum delay between reconnection attempts
    pub max_reconnect_delay: Duration,

    /// Interval between keepalive pings
    pub ping_interval: Duration,

    /// Timeout for operations (subscribe, publish, etc.)
    pub operation_timeout: Duration,

    /// Whether to skip TLS certificate verification (for development)
    pub dangerous_skip_cert_verify: bool,
}

impl CatbusConfig {
    /// Create a new configuration with the given URL and token
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            auto_reconnect: true,
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(30),
            ping_interval: Duration::from_secs(30),
            operation_timeout: Duration::from_secs(10),
            dangerous_skip_cert_verify: false,
        }
    }

    /// Disable automatic reconnection
    pub fn no_reconnect(mut self) -> Self {
        self.auto_reconnect = false;
        self
    }

    /// Set the reconnection delay range
    pub fn reconnect_delay(mut self, initial: Duration, max: Duration) -> Self {
        self.reconnect_delay = initial;
        self.max_reconnect_delay = max;
        self
    }

    /// Set the ping interval
    pub fn ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Set the operation timeout
    pub fn operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    /// Skip TLS certificate verification (DANGEROUS - only for development)
    pub fn dangerous_skip_cert_verify(mut self) -> Self {
        self.dangerous_skip_cert_verify = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_new_defaults() {
        let config = CatbusConfig::new("https://localhost:4433", "test-token");

        assert_eq!(config.url, "https://localhost:4433");
        assert_eq!(config.token, "test-token");
        assert!(config.auto_reconnect);
        assert_eq!(config.reconnect_delay, Duration::from_secs(1));
        assert_eq!(config.max_reconnect_delay, Duration::from_secs(30));
        assert_eq!(config.ping_interval, Duration::from_secs(30));
        assert_eq!(config.operation_timeout, Duration::from_secs(10));
        assert!(!config.dangerous_skip_cert_verify);
    }

    #[test]
    fn test_config_no_reconnect() {
        let config = CatbusConfig::new("https://localhost:4433", "token").no_reconnect();

        assert!(!config.auto_reconnect);
    }

    #[test]
    fn test_config_reconnect_delay() {
        let config = CatbusConfig::new("https://localhost:4433", "token")
            .reconnect_delay(Duration::from_millis(500), Duration::from_secs(60));

        assert_eq!(config.reconnect_delay, Duration::from_millis(500));
        assert_eq!(config.max_reconnect_delay, Duration::from_secs(60));
    }

    #[test]
    fn test_config_ping_interval() {
        let config =
            CatbusConfig::new("https://localhost:4433", "token").ping_interval(Duration::from_secs(15));

        assert_eq!(config.ping_interval, Duration::from_secs(15));
    }

    #[test]
    fn test_config_operation_timeout() {
        let config = CatbusConfig::new("https://localhost:4433", "token")
            .operation_timeout(Duration::from_secs(30));

        assert_eq!(config.operation_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_config_dangerous_skip_cert_verify() {
        let config =
            CatbusConfig::new("https://localhost:4433", "token").dangerous_skip_cert_verify();

        assert!(config.dangerous_skip_cert_verify);
    }

    #[test]
    fn test_config_builder_chain() {
        let config = CatbusConfig::new("https://example.com:4433", "my-token")
            .no_reconnect()
            .ping_interval(Duration::from_secs(10))
            .operation_timeout(Duration::from_secs(5))
            .dangerous_skip_cert_verify();

        assert_eq!(config.url, "https://example.com:4433");
        assert_eq!(config.token, "my-token");
        assert!(!config.auto_reconnect);
        assert_eq!(config.ping_interval, Duration::from_secs(10));
        assert_eq!(config.operation_timeout, Duration::from_secs(5));
        assert!(config.dangerous_skip_cert_verify);
    }

    #[test]
    fn test_config_clone() {
        let config1 = CatbusConfig::new("https://localhost:4433", "token").no_reconnect();
        let config2 = config1.clone();

        assert_eq!(config1.url, config2.url);
        assert_eq!(config1.token, config2.token);
        assert_eq!(config1.auto_reconnect, config2.auto_reconnect);
    }
}
