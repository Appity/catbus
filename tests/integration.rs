//! Integration tests for Catbus server
//!
//! These tests verify thread safety and concurrent operation handling.

use catbus::auth::{Grant, GrantSet, GrantType, SubToken};
use catbus::channels::ChannelPattern;
use catbus::storage::RingBuffer;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

const TEST_SECRET: &[u8] = b"test-secret-for-integration-tests";

fn create_token(client_id: &str, patterns: &[&str]) -> String {
    let mut grants = GrantSet::new();
    for pattern in patterns {
        let p = ChannelPattern::parse(pattern).unwrap();
        grants.add(Grant::new(GrantType::Read, p.clone()));
        grants.add(Grant::new(GrantType::Write, p));
    }
    SubToken::create(client_id.to_string(), grants, TEST_SECRET).to_string()
}

#[tokio::test]
async fn test_connection_manager_concurrent_access() {
    use catbus::server::{ClientConnection, ConnectionManager};

    let manager = ConnectionManager::new();

    // Spawn 100 tasks that add/remove connections concurrently
    let mut handles = vec![];

    for i in 0..100 {
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let grants = GrantSet::new();
            let (tx, _rx) = mpsc::channel(10);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("client-{}", i)),
                grants,
                false,
                tx,
            ));

            let id = conn.id;
            manager.add(conn);

            // Simulate some work
            tokio::time::sleep(Duration::from_micros(100)).await;

            manager.remove(id);
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(manager.count(), 0);
}

#[tokio::test]
async fn test_topic_router_concurrent_publish() {
    use catbus::channels::Channel;
    use catbus::server::{ClientConnection, ConnectionManager, TopicRouter};

    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create 10 subscribers
    let mut receivers = vec![];
    for i in 0..10 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("test.*").unwrap(),
        ));

        let (tx, rx) = mpsc::channel(100);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("sub-{}", i)),
            grants,
            false,
            tx,
        ));

        // Subscribe to pattern
        conn.subscribe(ChannelPattern::parse("test.*").unwrap());
        connections.index_subscription(conn.id, &ChannelPattern::parse("test.*").unwrap());
        connections.add(conn);
        receivers.push(rx);
    }

    // Publish 100 messages concurrently
    let mut publish_handles = vec![];
    for i in 0..100 {
        let router = router.clone();
        publish_handles.push(tokio::spawn(async move {
            let channel = Channel::parse(&format!("test.msg{}", i)).unwrap();
            let payload = format!(r#"{{"id":{}}}"#, i).into_bytes();
            router.route(&channel, payload).await
        }));
    }

    // Wait for all publishes
    let mut total_delivered = 0;
    for handle in publish_handles {
        total_delivered += handle.await.unwrap();
    }

    // Each message should be delivered to all 10 subscribers
    assert_eq!(total_delivered, 100 * 10);

    // Each subscriber should have received 100 messages
    for mut rx in receivers {
        let mut count = 0;
        while let Ok(Some(_)) = timeout(Duration::from_millis(100), rx.recv()).await {
            count += 1;
        }
        assert_eq!(count, 100);
    }
}

#[tokio::test]
async fn test_ring_buffer_concurrent_access() {
    let buffer = Arc::new(RingBuffer::new(1000));

    // Spawn writers
    let mut write_handles = vec![];
    for i in 0..10 {
        let buffer = buffer.clone();
        write_handles.push(tokio::spawn(async move {
            for j in 0..100 {
                buffer.push(format!("channel.{}.{}", i, j), vec![0u8; 100]);
            }
        }));
    }

    // Spawn readers concurrently
    let mut read_handles = vec![];
    for _ in 0..10 {
        let buffer = buffer.clone();
        read_handles.push(tokio::spawn(async move {
            for _ in 0..50 {
                let _ = buffer.get_after(0);
                tokio::time::sleep(Duration::from_micros(10)).await;
            }
        }));
    }

    // Wait for all operations
    for handle in write_handles {
        handle.await.unwrap();
    }
    for handle in read_handles {
        handle.await.unwrap();
    }

    // Buffer should have 1000 messages (capacity)
    assert_eq!(buffer.len(), 1000);
}

#[tokio::test]
async fn test_grants_concurrent_check() {
    let mut grants = GrantSet::new();
    grants.add(Grant::new(
        GrantType::Read,
        ChannelPattern::parse("project.*").unwrap(),
    ));
    grants.add(Grant::new(
        GrantType::Write,
        ChannelPattern::parse("project.123.*").unwrap(),
    ));

    let grants = Arc::new(grants);

    // Spawn 100 tasks checking permissions concurrently
    let mut handles = vec![];
    for i in 0..100 {
        let grants = grants.clone();
        handles.push(tokio::spawn(async move {
            let channel = catbus::channels::Channel::parse(&format!("project.{}", i)).unwrap();

            // All should be readable
            assert!(grants.can_read(&channel));

            // Only project.123.* should be writable
            let write_channel =
                catbus::channels::Channel::parse(&format!("project.123.sub{}", i)).unwrap();
            assert!(grants.can_write(&write_channel));

            let no_write =
                catbus::channels::Channel::parse(&format!("project.456.sub{}", i)).unwrap();
            assert!(!grants.can_write(&no_write));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_token_verification_under_load() {
    // Create a bunch of tokens
    let tokens: Vec<String> = (0..100)
        .map(|i| create_token(&format!("client-{}", i), &["project.*"]))
        .collect();

    // Verify all tokens concurrently
    let mut handles = vec![];
    for token in tokens {
        handles.push(tokio::spawn(async move {
            let parsed = SubToken::parse(&token, TEST_SECRET);
            assert!(parsed.is_ok());
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_channel_pattern_matching_under_load() {
    let patterns: Vec<ChannelPattern> = vec![
        ChannelPattern::parse("project.*").unwrap(),
        ChannelPattern::parse("user.alice.*").unwrap(),
        ChannelPattern::parse("events.public.*").unwrap(),
        ChannelPattern::parse("*").unwrap(),
    ];

    let patterns = Arc::new(patterns);

    // Test matching concurrently
    let mut handles = vec![];
    for i in 0..1000 {
        let patterns = patterns.clone();
        handles.push(tokio::spawn(async move {
            let channel =
                catbus::channels::Channel::parse(&format!("project.{}.updates", i)).unwrap();

            // Should match project.* and *
            assert!(patterns[0].matches(&channel));
            assert!(!patterns[1].matches(&channel));
            assert!(!patterns[2].matches(&channel));
            assert!(patterns[3].matches(&channel));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_subscription_fan_out_performance() {
    use catbus::channels::Channel;
    use catbus::server::{ClientConnection, ConnectionManager, TopicRouter};
    use std::time::Instant;

    let subscriber_counts = [10, 100, 500];

    for &count in &subscriber_counts {
        let connections = Arc::new(ConnectionManager::new());
        let router = TopicRouter::new(connections.clone());

        // Create subscribers
        for i in 0..count {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse("bench.*").unwrap(),
            ));

            let (tx, _rx) = mpsc::channel(1000);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("sub-{}", i)),
                grants,
                false,
                tx,
            ));

            conn.subscribe(ChannelPattern::parse("bench.*").unwrap());
            connections.index_subscription(conn.id, &ChannelPattern::parse("bench.*").unwrap());
            connections.add(conn);
        }

        // Benchmark publishing
        let channel = Channel::parse("bench.test").unwrap();
        let payload = vec![0u8; 256]; // 256 byte message

        let start = Instant::now();
        let iterations = 1000;

        for _ in 0..iterations {
            router.route(&channel, payload.clone()).await;
        }

        let elapsed = start.elapsed();
        let msgs_per_sec = iterations as f64 / elapsed.as_secs_f64();
        let deliveries_per_sec = (iterations * count) as f64 / elapsed.as_secs_f64();

        println!(
            "Subscribers: {}, Messages: {}, Time: {:?}, Msgs/s: {:.0}, Deliveries/s: {:.0}",
            count, iterations, elapsed, msgs_per_sec, deliveries_per_sec
        );
    }
}

#[tokio::test]
async fn test_subscription_index_consistency() {
    use catbus::server::{ClientConnection, ConnectionManager};

    let manager = Arc::new(ConnectionManager::new());

    // Add connections with overlapping subscription patterns
    let patterns = [
        "project.*",
        "project.123.*",
        "project.123.updates",
        "user.*",
    ];

    let mut conn_ids = vec![];

    for (i, pattern) in patterns.iter().enumerate() {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse(pattern).unwrap(),
        ));

        let (tx, _rx) = mpsc::channel(10);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("client-{}", i)),
            grants,
            false,
            tx,
        ));

        let pattern = ChannelPattern::parse(pattern).unwrap();
        conn.subscribe(pattern.clone());

        let id = conn.id;
        conn_ids.push(id);

        manager.add(conn);
        manager.index_subscription(id, &pattern);
    }

    // Verify routing finds correct subscribers
    let channel = catbus::channels::Channel::parse("project.123.updates").unwrap();
    let subscribers = manager.get_subscribers_for_channel(&channel);

    // Should match: project.*, project.123.*, project.123.updates (3 subscribers)
    assert_eq!(subscribers.len(), 3);

    // Remove one connection and verify index is updated
    manager.remove(conn_ids[0]); // Remove project.* subscriber

    let subscribers = manager.get_subscribers_for_channel(&channel);
    assert_eq!(subscribers.len(), 2);
}

#[tokio::test]
async fn test_many_concurrent_subscriptions() {
    use catbus::server::{ClientConnection, ConnectionManager};

    let manager = Arc::new(ConnectionManager::new());

    // Create 100 connections, each subscribing to different patterns
    let mut handles = vec![];
    for i in 0..100 {
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse(&format!("user.{}.*", i)).unwrap(),
            ));
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse("broadcast.*").unwrap(),
            ));

            let (tx, _rx) = mpsc::channel(10);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("user-{}", i)),
                grants,
                false,
                tx,
            ));

            // Subscribe to user-specific and broadcast channels
            let user_pattern = ChannelPattern::parse(&format!("user.{}.*", i)).unwrap();
            let broadcast_pattern = ChannelPattern::parse("broadcast.*").unwrap();

            conn.subscribe(user_pattern.clone());
            conn.subscribe(broadcast_pattern.clone());

            let id = conn.id;
            manager.add(conn);
            manager.index_subscription(id, &user_pattern);
            manager.index_subscription(id, &broadcast_pattern);

            id
        }));
    }

    // Wait for all connections
    let _conn_ids: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(manager.count(), 100);

    // Verify broadcast reaches all
    let broadcast_channel = catbus::channels::Channel::parse("broadcast.news").unwrap();
    let subscribers = manager.get_subscribers_for_channel(&broadcast_channel);
    assert_eq!(subscribers.len(), 100);

    // Verify user-specific only reaches one
    let user_channel = catbus::channels::Channel::parse("user.42.inbox").unwrap();
    let subscribers = manager.get_subscribers_for_channel(&user_channel);
    assert_eq!(subscribers.len(), 1);
}

#[tokio::test]
async fn test_publish_to_disconnecting_clients() {
    use catbus::channels::Channel;
    use catbus::server::{ClientConnection, ConnectionManager, TopicRouter};

    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create connections with small channel buffers
    let mut conn_ids = vec![];
    for i in 0..10 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("test.*").unwrap(),
        ));

        let (tx, rx) = mpsc::channel(1); // Very small buffer!
        let conn = Arc::new(ClientConnection::new(
            Some(format!("client-{}", i)),
            grants,
            false,
            tx,
        ));

        conn.subscribe(ChannelPattern::parse("test.*").unwrap());
        let id = conn.id;
        conn_ids.push(id);
        connections.add(conn);

        // Drop the receiver immediately to simulate disconnect
        drop(rx);
    }

    // Try to publish - should not panic or hang
    let channel = Channel::parse("test.message").unwrap();
    for _ in 0..100 {
        // This should handle send failures gracefully
        router.route(&channel, b"hello".to_vec()).await;
    }

    // Clean up
    for id in conn_ids {
        connections.remove(id);
    }

    assert_eq!(connections.count(), 0);
}
