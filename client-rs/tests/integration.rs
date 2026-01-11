//! Integration tests for catbus-client
//!
//! These tests require a running Catbus server. They are ignored by default
//! and can be run with:
//!
//! ```sh
//! CATBUS_TEST_URL=https://localhost:4433 CATBUS_TEST_TOKEN=your-token cargo test --test integration -- --ignored
//! ```

use catbus_client::{CatbusClient, CatbusConfig, CatbusError, ConnectionState};
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn get_test_config() -> Option<CatbusConfig> {
    let url = env::var("CATBUS_TEST_URL").ok()?;
    let token = env::var("CATBUS_TEST_TOKEN").ok()?;

    Some(
        CatbusConfig::new(url, token)
            .dangerous_skip_cert_verify()
            .ping_interval(Duration::from_secs(5))
            .operation_timeout(Duration::from_secs(5)),
    )
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_connect_disconnect() {
    let config = get_test_config().expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set");
    let client = CatbusClient::new(config);

    // Connect
    client.connect().await.expect("Failed to connect");
    assert_eq!(client.connection_state(), ConnectionState::Connected);

    // Disconnect
    client.disconnect().await.expect("Failed to disconnect");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_ping() {
    let config = get_test_config().expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set");
    let client = CatbusClient::new(config);

    client.connect().await.expect("Failed to connect");

    let rtt = client.ping().await.expect("Ping failed");
    println!("Ping RTT: {}ms", rtt);
    assert!(rtt < 5000, "RTT should be less than 5 seconds");

    client.disconnect().await.expect("Failed to disconnect");
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_subscribe_publish() {
    let config = get_test_config().expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set");
    let client = CatbusClient::new(config);

    client.connect().await.expect("Failed to connect");

    let received = Arc::new(AtomicUsize::new(0));
    let received_clone = received.clone();

    // Subscribe
    let sub = client
        .subscribe("test.channel.*", move |channel, payload| {
            println!("Received on {}: {:?}", channel, payload);
            received_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .expect("Subscribe failed");

    // Give time for subscription to be established
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish
    client
        .publish("test.channel.123", &serde_json::json!({"message": "hello"}))
        .await
        .expect("Publish failed");

    // Wait for message to arrive
    tokio::time::sleep(Duration::from_millis(500)).await;

    // We should have received at least one message (our own publish)
    // Note: Whether we receive our own publishes depends on server configuration
    println!("Received {} messages", received.load(Ordering::SeqCst));

    // Unsubscribe
    sub.unsubscribe().await.expect("Unsubscribe failed");

    client.disconnect().await.expect("Failed to disconnect");
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_multiple_subscriptions() {
    let config = get_test_config().expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set");
    let client = CatbusClient::new(config);

    client.connect().await.expect("Failed to connect");

    let count1 = Arc::new(AtomicUsize::new(0));
    let count2 = Arc::new(AtomicUsize::new(0));

    let count1_clone = count1.clone();
    let count2_clone = count2.clone();

    // Subscribe to two different patterns
    let sub1 = client
        .subscribe("test.a.*", move |_, _| {
            count1_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .expect("Subscribe 1 failed");

    let sub2 = client
        .subscribe("test.b.*", move |_, _| {
            count2_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .expect("Subscribe 2 failed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish to both channels
    client
        .publish("test.a.msg", &serde_json::json!({"from": "a"}))
        .await
        .expect("Publish to a failed");

    client
        .publish("test.b.msg", &serde_json::json!({"from": "b"}))
        .await
        .expect("Publish to b failed");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Cleanup
    sub1.unsubscribe().await.expect("Unsubscribe 1 failed");
    sub2.unsubscribe().await.expect("Unsubscribe 2 failed");

    client.disconnect().await.expect("Failed to disconnect");
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_reconnect() {
    let config = get_test_config()
        .expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set")
        .reconnect_delay(Duration::from_millis(100), Duration::from_secs(1));

    let client = CatbusClient::new(config);

    client.connect().await.expect("Failed to connect");
    assert_eq!(client.connection_state(), ConnectionState::Connected);

    // Subscribe to a channel
    let _sub = client
        .subscribe("test.reconnect.*", |_, _| {})
        .await
        .expect("Subscribe failed");

    println!("Connected and subscribed. Test reconnection behavior manually.");

    client.disconnect().await.expect("Failed to disconnect");
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_invalid_token() {
    let url = env::var("CATBUS_TEST_URL").expect("CATBUS_TEST_URL must be set");
    let config = CatbusConfig::new(url, "invalid-token-12345").dangerous_skip_cert_verify();

    let client = CatbusClient::new(config);

    let result = client.connect().await;
    assert!(
        result.is_err(),
        "Should fail with invalid token"
    );

    if let Err(CatbusError::Authentication(msg)) = result {
        println!("Got expected auth error: {}", msg);
    }
}

#[tokio::test]
#[ignore = "requires running Catbus server"]
async fn test_state_changes() {
    let config = get_test_config().expect("CATBUS_TEST_URL and CATBUS_TEST_TOKEN must be set");
    let client = CatbusClient::new(config);

    let mut rx = client.state_receiver();

    // Initial state
    assert_eq!(*rx.borrow(), ConnectionState::Disconnected);

    // Connect in background
    let client_clone = client.clone();
    let connect_handle = tokio::spawn(async move {
        client_clone.connect().await
    });

    // Watch for state changes
    let mut saw_connecting = false;
    let mut saw_authenticating = false;
    let mut saw_connected = false;

    let timeout = tokio::time::timeout(Duration::from_secs(10), async {
        while rx.changed().await.is_ok() {
            let state = *rx.borrow();
            println!("State changed to: {:?}", state);

            match state {
                ConnectionState::Connecting => saw_connecting = true,
                ConnectionState::Authenticating => saw_authenticating = true,
                ConnectionState::Connected => {
                    saw_connected = true;
                    break;
                }
                _ => {}
            }
        }
    });

    if timeout.await.is_err() {
        println!("Timeout waiting for state changes");
    }

    connect_handle.await.expect("Connect task panicked").expect("Connect failed");

    println!(
        "State transitions - Connecting: {}, Authenticating: {}, Connected: {}",
        saw_connecting, saw_authenticating, saw_connected
    );

    assert!(saw_connected, "Should have reached Connected state");

    client.disconnect().await.expect("Disconnect failed");
}
