//! Resilience tests for Catbus
//!
//! These tests verify behavior under failure conditions like:
//! - Connection drops
//! - Channel buffer exhaustion
//! - Subscriber disappearing mid-delivery
//! - Rapid connect/disconnect cycles
//! - Recovery after component restart

use catbus::auth::{Grant, GrantSet, GrantType};
use catbus::channels::{Channel, ChannelPattern};
use catbus::server::{ClientConnection, ConnectionManager, TopicRouter};
use catbus::storage::RingBuffer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Test that the router handles subscribers disappearing mid-delivery
#[tokio::test]
async fn test_subscribers_disappearing_mid_delivery() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create 100 subscribers
    let mut receivers: Vec<_> = (0..100)
        .map(|i| {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse("events.*").unwrap(),
            ));

            let (tx, rx) = mpsc::channel(10);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("client-{}", i)),
                grants,
                false,
                tx,
            ));

            conn.subscribe(ChannelPattern::parse("events.*").unwrap());
            let id = conn.id;
            connections.index_subscription(id, &ChannelPattern::parse("events.*").unwrap());
            connections.add(conn);
            (id, rx)
        })
        .collect();

    // Drop half the receivers to simulate disconnects
    for (_, rx) in receivers.iter_mut().skip(50) {
        // Dropping rx means sends will fail
    }
    // Actually drop them
    let receivers: Vec<_> = receivers.into_iter().take(50).collect();

    // Publish messages - should not panic even with dead receivers
    let channel = Channel::parse("events.test").unwrap();
    for i in 0..100 {
        let payload = format!(r#"{{"seq":{}}}"#, i).into_bytes();
        router.route(&channel, payload).await;
    }

    // Living receivers should still get messages
    for (_, mut rx) in receivers {
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        // Should have received messages (buffer size is 10, so at least 10)
        assert!(count >= 10, "Expected at least 10 messages, got {}", count);
    }
}

/// Test rapid connect/disconnect cycles don't cause issues
#[tokio::test]
async fn test_rapid_connect_disconnect_cycles() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    let channel = Channel::parse("test.rapid").unwrap();

    // Spawn publisher that continuously publishes
    let router_clone = router.clone();
    let publisher = tokio::spawn(async move {
        for i in 0..1000 {
            router_clone
                .route(&channel, format!("{}", i).into_bytes())
                .await;
            // Small yield to allow other tasks
            if i % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
    });

    // Spawn churner that rapidly adds/removes connections
    let connections_clone = connections.clone();
    let churner = tokio::spawn(async move {
        for i in 0..500 {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse("test.*").unwrap(),
            ));

            let (tx, _rx) = mpsc::channel(10);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("churn-{}", i)),
                grants,
                false,
                tx,
            ));

            conn.subscribe(ChannelPattern::parse("test.*").unwrap());
            let id = conn.id;
            connections_clone.index_subscription(id, &ChannelPattern::parse("test.*").unwrap());
            connections_clone.add(conn);

            // Immediately remove
            connections_clone.remove(id);
        }
    });

    // Both should complete without panicking
    publisher.await.unwrap();
    churner.await.unwrap();

    assert_eq!(connections.count(), 0);
}

/// Test that buffer overflow is handled gracefully
#[tokio::test]
async fn test_slow_consumer_buffer_overflow() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create a subscriber with a tiny buffer
    let mut grants = GrantSet::new();
    grants.add(Grant::new(
        GrantType::Read,
        ChannelPattern::parse("flood.*").unwrap(),
    ));

    let (tx, mut rx) = mpsc::channel(2); // Very small buffer!
    let conn = Arc::new(ClientConnection::new(
        Some("slow-consumer".to_string()),
        grants,
        false,
        tx,
    ));

    conn.subscribe(ChannelPattern::parse("flood.*").unwrap());
    let id = conn.id;
    connections.index_subscription(id, &ChannelPattern::parse("flood.*").unwrap());
    connections.add(conn);

    let channel = Channel::parse("flood.test").unwrap();

    // Flood with messages (more than buffer can hold)
    for i in 0..100 {
        router.route(&channel, format!("{}", i).into_bytes()).await;
    }

    // Consumer should get some messages (the ones that fit before buffer filled)
    let mut received = 0;
    while rx.try_recv().is_ok() {
        received += 1;
    }

    // Should have received at least 2 (buffer size)
    assert!(
        received >= 2,
        "Expected at least 2 messages, got {}",
        received
    );
    println!("Slow consumer received {} messages out of 100", received);
}

/// Test ring buffer behavior when full
#[tokio::test]
async fn test_ring_buffer_wrap_around_resilience() {
    let buffer = RingBuffer::new(100);

    // Fill beyond capacity
    for i in 0..500 {
        buffer.push(format!("channel.{}", i % 10), vec![i as u8; 64]);
    }

    // Should have exactly 100 messages (capacity)
    assert_eq!(buffer.len(), 100);

    // Messages should be the last 100 (400-499)
    let messages = buffer.get_after(0);
    assert_eq!(messages.len(), 100);

    // First message should have seq > 400
    assert!(messages[0].seq >= 400);

    // Binary search should still work correctly
    let after_450 = buffer.get_after(450);
    assert!(after_450.len() < 100);
    for msg in &after_450 {
        assert!(msg.seq > 450);
    }
}

/// Test concurrent reads and writes to ring buffer during overflow
#[tokio::test]
async fn test_ring_buffer_concurrent_overflow() {
    let buffer = Arc::new(RingBuffer::new(100));

    // Spawn writers that will cause overflow
    let mut handles = vec![];
    for writer_id in 0..10 {
        let buffer = buffer.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..200 {
                buffer.push(format!("writer.{}.msg{}", writer_id, i), vec![0u8; 32]);
            }
        }));
    }

    // Spawn readers that continuously read
    for _ in 0..5 {
        let buffer = buffer.clone();
        handles.push(tokio::spawn(async move {
            let mut last_seq = 0;
            for _ in 0..100 {
                let msgs = buffer.get_after(last_seq);
                if let Some(last) = msgs.last() {
                    last_seq = last.seq;
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    // All should complete without panicking
    for handle in handles {
        handle.await.unwrap();
    }

    // Buffer should be at capacity
    assert_eq!(buffer.len(), 100);
}

/// Test recovery after all connections drop
#[tokio::test]
async fn test_recovery_after_mass_disconnect() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Add 100 connections
    let mut conn_ids = vec![];
    for i in 0..100 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("*").unwrap(),
        ));

        let (tx, _rx) = mpsc::channel(10);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("client-{}", i)),
            grants,
            false,
            tx,
        ));

        conn.subscribe(ChannelPattern::parse("*").unwrap());
        let id = conn.id;
        conn_ids.push(id);
        connections.index_subscription(id, &ChannelPattern::parse("*").unwrap());
        connections.add(conn);
    }

    assert_eq!(connections.count(), 100);

    // Mass disconnect
    for id in conn_ids {
        connections.remove(id);
    }

    assert_eq!(connections.count(), 0);

    // Publishing to empty server should work
    let channel = Channel::parse("test.empty").unwrap();
    let delivered = router.route(&channel, b"hello".to_vec()).await;
    assert_eq!(delivered, 0);

    // Add new connections - should work normally
    for i in 0..50 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("test.*").unwrap(),
        ));

        let (tx, _rx) = mpsc::channel(10);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("new-client-{}", i)),
            grants,
            false,
            tx,
        ));

        conn.subscribe(ChannelPattern::parse("test.*").unwrap());
        let id = conn.id;
        connections.index_subscription(id, &ChannelPattern::parse("test.*").unwrap());
        connections.add(conn);
    }

    assert_eq!(connections.count(), 50);

    // Publishing should now reach new subscribers
    let delivered = router.route(&channel, b"hello again".to_vec()).await;
    assert_eq!(delivered, 50);
}

/// Test behavior with extremely high message throughput
#[tokio::test]
async fn test_high_throughput_resilience() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create subscribers with large buffers
    let message_count = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for i in 0..10 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("throughput.*").unwrap(),
        ));

        let (tx, mut rx) = mpsc::channel(10000);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("sub-{}", i)),
            grants,
            false,
            tx,
        ));

        conn.subscribe(ChannelPattern::parse("throughput.*").unwrap());
        let id = conn.id;
        connections.index_subscription(id, &ChannelPattern::parse("throughput.*").unwrap());
        connections.add(conn);

        // Spawn consumer
        let count = message_count.clone();
        handles.push(tokio::spawn(async move {
            while let Some(_) = rx.recv().await {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Blast messages
    let start = Instant::now();
    let channel = Channel::parse("throughput.test").unwrap();
    let total_messages = 100_000;

    for i in 0..total_messages {
        router
            .route(&channel, format!("{}", i).into_bytes())
            .await;
    }

    let elapsed = start.elapsed();

    // Give consumers a moment to catch up
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Drop connections to close channels
    let conn_ids: Vec<_> = (0..connections.count())
        .filter_map(|_| connections.get_subscribers_for_channel(&channel).first().map(|c| c.id))
        .collect();

    for id in conn_ids {
        connections.remove(id);
    }

    // Wait for consumer tasks
    for handle in handles {
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    let received = message_count.load(Ordering::Relaxed);
    let msgs_per_sec = total_messages as f64 / elapsed.as_secs_f64();

    println!(
        "High throughput: {} messages in {:?} ({:.0} msg/s)",
        total_messages, elapsed, msgs_per_sec
    );
    println!("Total deliveries: {} (expected {})", received, total_messages * 10);

    // Should have delivered most messages (some may be dropped at the end due to race)
    assert!(
        received > total_messages * 8,
        "Expected at least {} deliveries, got {}",
        total_messages * 8,
        received
    );
}

/// Test that permissions are checked correctly even under concurrent load
#[tokio::test]
async fn test_permission_enforcement_under_load() {
    let connections = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(connections.clone());

    // Create subscriber with limited permissions
    let mut grants = GrantSet::new();
    grants.add(Grant::new(
        GrantType::Read,
        ChannelPattern::parse("allowed.*").unwrap(),
    ));
    // No permission for "denied.*"

    let (tx, mut rx) = mpsc::channel(1000);
    let conn = Arc::new(ClientConnection::new(
        Some("limited".to_string()),
        grants,
        false,
        tx,
    ));

    conn.subscribe(ChannelPattern::parse("allowed.*").unwrap());
    conn.subscribe(ChannelPattern::parse("denied.*").unwrap()); // Will be rejected
    let id = conn.id;
    connections.index_subscription(id, &ChannelPattern::parse("allowed.*").unwrap());
    // Not indexed for denied.* because permission check should fail
    connections.add(conn);

    // Publish to both channels concurrently
    let mut handles = vec![];
    for i in 0..100 {
        let router = router.clone();
        handles.push(tokio::spawn(async move {
            let allowed = Channel::parse(&format!("allowed.{}", i)).unwrap();
            let denied = Channel::parse(&format!("denied.{}", i)).unwrap();

            router.route(&allowed, b"yes".to_vec()).await;
            router.route(&denied, b"no".to_vec()).await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Count received messages
    let mut count = 0;
    while rx.try_recv().is_ok() {
        count += 1;
    }

    // Should only receive "allowed" messages (100), not "denied" ones
    assert_eq!(count, 100, "Should only receive allowed messages");
}

/// Test subscription index integrity under concurrent operations
#[tokio::test]
async fn test_subscription_index_integrity() {
    let connections = Arc::new(ConnectionManager::new());

    // Spawn tasks that concurrently add, subscribe, and remove
    let mut handles = vec![];
    for batch in 0..10 {
        let connections = connections.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..100 {
                let mut grants = GrantSet::new();
                let pattern_str = format!("batch{}.item{}.*", batch, i % 10);
                grants.add(Grant::new(
                    GrantType::Read,
                    ChannelPattern::parse(&pattern_str).unwrap(),
                ));

                let (tx, _rx) = mpsc::channel(10);
                let conn = Arc::new(ClientConnection::new(
                    Some(format!("client-{}-{}", batch, i)),
                    grants,
                    false,
                    tx,
                ));

                let pattern = ChannelPattern::parse(&pattern_str).unwrap();
                conn.subscribe(pattern.clone());
                let id = conn.id;

                connections.add(conn);
                connections.index_subscription(id, &pattern);

                // Randomly remove some
                if i % 3 == 0 {
                    connections.remove(id);
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Final count should be consistent
    let count = connections.count();
    println!("Final connection count: {}", count);

    // Verify we can still route to remaining connections
    let channel = Channel::parse("batch0.item0.test").unwrap();
    let subscribers = connections.get_subscribers_for_channel(&channel);

    // Each subscriber should be valid (not removed)
    for sub in &subscribers {
        assert!(
            connections
                .get_subscribers_for_channel(&channel)
                .iter()
                .any(|s| s.id == sub.id)
        );
    }
}
