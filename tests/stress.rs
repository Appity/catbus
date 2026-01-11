//! Stress tests for pathological cases
//!
//! These tests verify we don't have accidentally quadratic behavior.

use catbus::auth::{Grant, GrantSet, GrantType};
use catbus::channels::{Channel, ChannelPattern};
use std::time::Instant;

/// Test that permission checking doesn't degrade with many grants
#[test]
fn test_permission_check_not_quadratic() {
    let grant_counts = [100, 1_000, 10_000, 100_000];
    let mut times = vec![];

    for &count in &grant_counts {
        let mut grants = GrantSet::new();

        // Create many grants with unique patterns
        for i in 0..count {
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse(&format!("project.{}.updates", i)).unwrap(),
            ));
        }

        // Time how long it takes to check permissions
        let iterations = 10_000;
        let start = Instant::now();

        for i in 0..iterations {
            let channel = Channel::parse(&format!("project.{}.updates", i % count)).unwrap();
            let _ = grants.can_read(&channel);
        }

        let elapsed = start.elapsed();
        let per_check_ns = elapsed.as_nanos() / iterations as u128;
        times.push((count, per_check_ns));

        println!(
            "Grants: {:>6}, Checks: {}, Time: {:?}, Per check: {}ns",
            count, iterations, elapsed, per_check_ns
        );
    }

    // Verify we're not quadratic: time should grow roughly linearly with grant count
    // Allow 20x growth for 1000x more grants (should be ~linear or O(n log n))
    let (small_count, small_time) = times[0];
    let (large_count, large_time) = times[times.len() - 1];

    let count_ratio = large_count as f64 / small_count as f64;
    let time_ratio = large_time as f64 / small_time as f64;

    println!(
        "\nGrant count ratio: {:.0}x, Time ratio: {:.1}x",
        count_ratio, time_ratio
    );

    // If quadratic, time_ratio would be ~count_ratio².
    // We allow up to count_ratio * 10 to account for cache effects
    assert!(
        time_ratio < count_ratio * 10.0,
        "Permission checking appears to be quadratic! Time grew {:.1}x for {:.0}x more grants",
        time_ratio,
        count_ratio
    );
}

/// Test that pattern matching against many subscriptions isn't quadratic
#[test]
fn test_subscription_matching_not_quadratic() {
    use catbus::server::{ClientConnection, ConnectionManager};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    let subscriber_counts = [100, 1_000, 10_000];
    let mut times = vec![];

    for &count in &subscriber_counts {
        let manager = ConnectionManager::new();

        // Create many subscribers with unique patterns
        for i in 0..count {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse(&format!("user.{}.inbox", i)).unwrap(),
            ));

            let (tx, _rx) = mpsc::channel(1);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("user-{}", i)),
                grants,
                false,
                tx,
            ));

            let pattern = ChannelPattern::parse(&format!("user.{}.inbox", i)).unwrap();
            conn.subscribe(pattern.clone());
            let id = conn.id;
            manager.add(conn);
            manager.index_subscription(id, &pattern);
        }

        // Time how long it takes to find subscribers
        let iterations = 1_000;
        let start = Instant::now();

        for i in 0..iterations {
            let channel = Channel::parse(&format!("user.{}.inbox", i % count)).unwrap();
            let _ = manager.find_subscribers(&channel);
        }

        let elapsed = start.elapsed();
        let per_lookup_us = elapsed.as_micros() / iterations as u128;
        times.push((count, per_lookup_us));

        println!(
            "Subscribers: {:>6}, Lookups: {}, Time: {:?}, Per lookup: {}µs",
            count, iterations, elapsed, per_lookup_us
        );
    }

    // Check for quadratic behavior
    let (small_count, small_time) = times[0];
    let (large_count, large_time) = times[times.len() - 1];

    let count_ratio = large_count as f64 / small_count as f64;
    let time_ratio = large_time as f64 / small_time.max(1) as f64;

    println!(
        "\nSubscriber count ratio: {:.0}x, Time ratio: {:.1}x",
        count_ratio, time_ratio
    );

    // Current implementation is O(n) - it checks all connections
    // This test documents that behavior; we'd want to improve the index
    // For now, just ensure it's not worse than O(n)
    assert!(
        time_ratio < count_ratio * 5.0,
        "Subscriber lookup appears worse than linear!"
    );
}

/// Test that wildcard pattern matching scales reasonably
#[test]
fn test_wildcard_matching_performance() {
    let pattern_counts = [10, 100, 1_000, 10_000];
    let mut times = vec![];

    for &count in &pattern_counts {
        // Create patterns of varying specificity
        let patterns: Vec<ChannelPattern> = (0..count)
            .map(|i| {
                ChannelPattern::parse(&format!("level1.level2.level3.{}.data", i)).unwrap()
            })
            .collect();

        // Add some wildcard patterns
        let wildcards: Vec<ChannelPattern> = vec![
            ChannelPattern::parse("level1.*").unwrap(),
            ChannelPattern::parse("level1.level2.*").unwrap(),
            ChannelPattern::parse("level1.level2.level3.*").unwrap(),
        ];

        // Test channel that matches wildcards but not exact patterns
        let channel = Channel::parse("level1.level2.level3.nomatch.data").unwrap();

        let iterations = 100_000;
        let start = Instant::now();

        for _ in 0..iterations {
            // Check all patterns
            for p in &patterns {
                let _ = p.matches(&channel);
            }
            for w in &wildcards {
                let _ = w.matches(&channel);
            }
        }

        let elapsed = start.elapsed();
        let per_iteration_us = elapsed.as_micros() / iterations as u128;
        times.push((count, per_iteration_us));

        println!(
            "Patterns: {:>6}, Iterations: {}, Time: {:?}, Per iteration: {}µs",
            count, iterations, elapsed, per_iteration_us
        );
    }

    // Verify linear scaling
    let (small_count, small_time) = times[0];
    let (large_count, large_time) = times[times.len() - 1];

    let count_ratio = large_count as f64 / small_count as f64;
    let time_ratio = large_time as f64 / small_time.max(1) as f64;

    println!(
        "\nPattern count ratio: {:.0}x, Time ratio: {:.1}x",
        count_ratio, time_ratio
    );

    // Should be linear - we're just iterating through patterns
    assert!(
        time_ratio < count_ratio * 2.0,
        "Pattern matching scaling is worse than expected!"
    );
}

/// Test grant set operations with many overlapping patterns
#[test]
fn test_grant_set_with_overlapping_patterns() {
    let mut grants = GrantSet::new();

    // Add hierarchical grants that overlap
    // This is a realistic scenario: user has access to multiple project subtrees
    for project in 0..1000 {
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse(&format!("org.acme.project.{}.docs.*", project)).unwrap(),
        ));
        grants.add(Grant::new(
            GrantType::Write,
            ChannelPattern::parse(&format!("org.acme.project.{}.data.*", project)).unwrap(),
        ));
    }

    // Also add some broader grants
    grants.add(Grant::new(
        GrantType::Read,
        ChannelPattern::parse("org.acme.public.*").unwrap(),
    ));

    println!("Total grants: {}", grants.grants().len());

    // Test permission checks
    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        let project = i % 1000;

        // Check read on docs (should succeed)
        let doc_channel = Channel::parse(&format!("org.acme.project.{}.docs.readme", project)).unwrap();
        assert!(grants.can_read(&doc_channel));

        // Check write on data (should succeed)
        let data_channel = Channel::parse(&format!("org.acme.project.{}.data.file", project)).unwrap();
        assert!(grants.can_write(&data_channel));

        // Check write on docs (should fail)
        assert!(!grants.can_write(&doc_channel));

        // Check read on public (should succeed)
        let public_channel = Channel::parse("org.acme.public.announcements").unwrap();
        assert!(grants.can_read(&public_channel));
    }

    let elapsed = start.elapsed();
    println!(
        "Completed {} iterations of 4 checks each in {:?} ({:.2}µs per iteration)",
        iterations,
        elapsed,
        elapsed.as_micros() as f64 / iterations as f64
    );
}

/// Test that the ring buffer doesn't degrade with size for realistic operations
#[test]
fn test_ring_buffer_performance_at_capacity() {
    use catbus::storage::RingBuffer;

    let capacities = [1_000, 10_000, 100_000];
    let mut times = vec![];

    // Realistic catch-up: client wants last N messages, not half the buffer
    let catch_up_size = 100;

    for &capacity in &capacities {
        let buffer = RingBuffer::new(capacity);

        // Fill to capacity
        for i in 0..capacity {
            buffer.push(format!("channel.{}", i), vec![0u8; 100]);
        }

        assert_eq!(buffer.len(), capacity);

        // Time operations at capacity
        let iterations = 10_000;
        let start = Instant::now();

        for i in 0..iterations {
            // Push (causes eviction)
            buffer.push(format!("channel.new.{}", i), vec![0u8; 100]);

            // Realistic catch-up: get last 100 messages (like a reconnecting client)
            let _ = buffer.get_after(buffer.current_seq() - catch_up_size);
        }

        let elapsed = start.elapsed();
        let per_op_us = elapsed.as_micros() / iterations as u128;
        times.push((capacity, per_op_us));

        println!(
            "Capacity: {:>7}, Catch-up: {}, Operations: {}, Time: {:?}, Per op: {}µs",
            capacity, catch_up_size, iterations, elapsed, per_op_us
        );
    }

    // With binary search + fixed catch-up size, should be nearly constant time
    let (small_cap, small_time) = times[0];
    let (large_cap, large_time) = times[times.len() - 1];

    let cap_ratio = large_cap as f64 / small_cap as f64;
    let time_ratio = large_time as f64 / small_time.max(1) as f64;

    println!(
        "\nCapacity ratio: {:.0}x, Time ratio: {:.1}x",
        cap_ratio, time_ratio
    );

    // Should scale sub-linearly since we're reading from middle
    assert!(
        time_ratio < cap_ratio,
        "Ring buffer performance degraded too much with size!"
    );
}

/// Simulate realistic workload: many connections, varied patterns
#[test]
fn test_realistic_workload() {
    use catbus::server::{ClientConnection, ConnectionManager, TopicRouter};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    let manager = Arc::new(ConnectionManager::new());
    let router = TopicRouter::new(manager.clone());

    // Create realistic mix of subscribers:
    // - 1000 users with personal channels
    // - 100 project channels with 10 subscribers each
    // - 1 broadcast channel with all subscribers

    // Personal channels
    for i in 0..1000 {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse(&format!("user.{}.inbox", i)).unwrap(),
        ));
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("broadcast.*").unwrap(),
        ));

        let (tx, _rx) = mpsc::channel(100);
        let conn = Arc::new(ClientConnection::new(
            Some(format!("user-{}", i)),
            grants,
            false,
            tx,
        ));

        let personal = ChannelPattern::parse(&format!("user.{}.inbox", i)).unwrap();
        let broadcast = ChannelPattern::parse("broadcast.*").unwrap();
        conn.subscribe(personal.clone());
        conn.subscribe(broadcast.clone());

        let id = conn.id;
        manager.add(conn);
        manager.index_subscription(id, &personal);
        manager.index_subscription(id, &broadcast);
    }

    // Project subscribers (10 users per project)
    for project in 0..100 {
        for user in 0..10 {
            let mut grants = GrantSet::new();
            grants.add(Grant::new(
                GrantType::Read,
                ChannelPattern::parse(&format!("project.{}.updates", project)).unwrap(),
            ));

            let (tx, _rx) = mpsc::channel(100);
            let conn = Arc::new(ClientConnection::new(
                Some(format!("project-{}-user-{}", project, user)),
                grants,
                false,
                tx,
            ));

            let pattern = ChannelPattern::parse(&format!("project.{}.updates", project)).unwrap();
            conn.subscribe(pattern.clone());
            let id = conn.id;
            manager.add(conn);
            manager.index_subscription(id, &pattern);
        }
    }

    println!("Total connections: {}", manager.count());

    // Simulate workload
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let iterations = 1000;
        let start = Instant::now();

        for i in 0..iterations {
            // Personal message (1 recipient)
            let personal = Channel::parse(&format!("user.{}.inbox", i % 1000)).unwrap();
            router.route(&personal, b"personal msg".to_vec()).await;

            // Project update (10 recipients)
            let project = Channel::parse(&format!("project.{}.updates", i % 100)).unwrap();
            router.route(&project, b"project update".to_vec()).await;

            // Broadcast (1000+ recipients)
            if i % 100 == 0 {
                let broadcast = Channel::parse("broadcast.announcement").unwrap();
                router.route(&broadcast, b"broadcast".to_vec()).await;
            }
        }

        let elapsed = start.elapsed();
        println!(
            "Workload: {} iterations, Time: {:?}, Per iteration: {:.2}µs",
            iterations,
            elapsed,
            elapsed.as_micros() as f64 / iterations as f64
        );
    });
}
