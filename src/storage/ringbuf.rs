//! In-memory ring buffer for recent messages (catch-up on reconnect)

use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// A message stored in the ring buffer
#[derive(Debug, Clone)]
pub struct BufferedMessage {
    pub seq: u64,
    pub channel: String,
    pub payload: Vec<u8>,
    pub timestamp: std::time::Instant,
}

/// Ring buffer for recent messages per channel prefix
pub struct RingBuffer {
    /// Maximum number of messages to retain
    capacity: usize,
    /// Messages in order
    messages: RwLock<VecDeque<BufferedMessage>>,
    /// Next sequence number
    next_seq: AtomicU64,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            messages: RwLock::new(VecDeque::with_capacity(capacity)),
            next_seq: AtomicU64::new(1),
        }
    }

    /// Add a message to the buffer
    pub fn push(&self, channel: String, payload: Vec<u8>) -> u64 {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);

        let msg = BufferedMessage {
            seq,
            channel,
            payload,
            timestamp: std::time::Instant::now(),
        };

        let mut messages = self.messages.write();

        // Remove oldest if at capacity
        while messages.len() >= self.capacity {
            messages.pop_front();
        }

        messages.push_back(msg);

        seq
    }

    /// Get all messages after a given sequence number
    ///
    /// Uses binary search since messages are ordered by sequence number.
    pub fn get_after(&self, after_seq: u64) -> Vec<BufferedMessage> {
        let messages = self.messages.read();

        if messages.is_empty() {
            return Vec::new();
        }

        // Binary search to find the first message with seq > after_seq
        let start_idx = self.binary_search_after(&messages, after_seq);

        // Collect from start_idx to end
        messages.iter().skip(start_idx).cloned().collect()
    }

    /// Get all messages after a sequence number for a specific channel pattern
    ///
    /// Uses binary search to find starting point, then filters by channel.
    pub fn get_after_for_channel(&self, after_seq: u64, channel_prefix: &str) -> Vec<BufferedMessage> {
        let messages = self.messages.read();

        if messages.is_empty() {
            return Vec::new();
        }

        // Binary search to find the first message with seq > after_seq
        let start_idx = self.binary_search_after(&messages, after_seq);

        // Filter remaining messages by channel prefix
        messages
            .iter()
            .skip(start_idx)
            .filter(|m| m.channel.starts_with(channel_prefix))
            .cloned()
            .collect()
    }

    /// Binary search to find index of first message with seq > target
    fn binary_search_after(&self, messages: &VecDeque<BufferedMessage>, target_seq: u64) -> usize {
        if messages.is_empty() {
            return 0;
        }

        // Check bounds first
        if let Some(first) = messages.front() {
            if first.seq > target_seq {
                return 0;
            }
        }
        if let Some(last) = messages.back() {
            if last.seq <= target_seq {
                return messages.len();
            }
        }

        // Binary search
        let mut left = 0;
        let mut right = messages.len();

        while left < right {
            let mid = left + (right - left) / 2;
            if messages[mid].seq <= target_seq {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        left
    }

    /// Get the current sequence number (for "since" tracking)
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst) - 1
    }

    /// Get the oldest sequence number still in the buffer
    pub fn oldest_seq(&self) -> Option<u64> {
        self.messages.read().front().map(|m| m.seq)
    }

    /// Clear all messages
    pub fn clear(&self) {
        self.messages.write().clear();
    }

    /// Get current buffer size
    pub fn len(&self) -> usize {
        self.messages.read().len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.messages.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_basic() {
        let buf = RingBuffer::new(3);

        let seq1 = buf.push("channel.a".to_string(), b"msg1".to_vec());
        let seq2 = buf.push("channel.b".to_string(), b"msg2".to_vec());
        let seq3 = buf.push("channel.a".to_string(), b"msg3".to_vec());

        assert_eq!(buf.len(), 3);
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);

        let msgs = buf.get_after(0);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_ring_buffer_overflow() {
        let buf = RingBuffer::new(2);

        buf.push("a".to_string(), b"1".to_vec());
        buf.push("b".to_string(), b"2".to_vec());
        buf.push("c".to_string(), b"3".to_vec());

        assert_eq!(buf.len(), 2);

        let msgs = buf.get_after(0);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].seq, 2); // msg 1 was evicted
        assert_eq!(msgs[1].seq, 3);
    }

    #[test]
    fn test_ring_buffer_filter_channel() {
        let buf = RingBuffer::new(10);

        buf.push("project.abc.updates".to_string(), b"1".to_vec());
        buf.push("project.xyz.updates".to_string(), b"2".to_vec());
        buf.push("project.abc.steps".to_string(), b"3".to_vec());
        buf.push("user.123.inbox".to_string(), b"4".to_vec());

        let msgs = buf.get_after_for_channel(0, "project.abc");
        assert_eq!(msgs.len(), 2);
    }
}
