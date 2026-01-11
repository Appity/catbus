//! Channel naming and pattern matching
//!
//! Channels are dot-separated segments: `project.abc123.updates`
//! Each segment must match: [a-zA-Z0-9_-]+
//!
//! Wildcards are only allowed at the end:
//! - `project.abc123.*` matches `project.abc123.updates`, `project.abc123.steps.5`
//! - `project.*` matches anything under `project.`
//! - `*` matches everything (admin only)

use std::fmt;
use thiserror::Error;

/// Valid characters for a channel segment
fn is_valid_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Validate a single segment
fn is_valid_segment(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_valid_segment_char)
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("channel name cannot be empty")]
    Empty,

    #[error("invalid segment '{0}': must match [a-zA-Z0-9_-]+")]
    InvalidSegment(String),

    #[error("wildcard '*' can only appear as the last segment")]
    WildcardNotAtEnd,

    #[error("empty segment in channel name")]
    EmptySegment,
}

/// A validated channel name (no wildcards)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Channel {
    /// The full channel name, e.g. "project.abc123.updates"
    name: String,
    /// Precomputed segment boundaries for efficient matching
    /// Each entry is the start index of a segment
    segments: Vec<usize>,
}

impl Channel {
    /// Parse and validate a channel name
    pub fn parse(name: &str) -> Result<Self, ChannelError> {
        if name.is_empty() {
            return Err(ChannelError::Empty);
        }

        let mut segments = vec![0];

        for (i, part) in name.split('.').enumerate() {
            if part.is_empty() {
                return Err(ChannelError::EmptySegment);
            }

            if part == "*" {
                return Err(ChannelError::WildcardNotAtEnd);
            }

            if !is_valid_segment(part) {
                return Err(ChannelError::InvalidSegment(part.to_string()));
            }

            // Record segment start positions (after the first)
            if i > 0 {
                // +1 for the dot
                segments.push(segments.last().unwrap() + 1 + name.split('.').nth(i - 1).unwrap().len());
            }
        }

        Ok(Self {
            name: name.to_string(),
            segments,
        })
    }

    /// Get the channel name as a string slice
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// Get the number of segments
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Get a specific segment by index
    pub fn segment(&self, index: usize) -> Option<&str> {
        if index >= self.segments.len() {
            return None;
        }

        let start = self.segments[index];
        let end = if index + 1 < self.segments.len() {
            self.segments[index + 1] - 1 // -1 to exclude the dot
        } else {
            self.name.len()
        };

        Some(&self.name[start..end])
    }

    /// Check if this channel starts with a given prefix (for efficient routing)
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.name.starts_with(prefix)
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// A channel pattern that may include a trailing wildcard
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelPattern {
    /// The prefix before the wildcard (or full name if no wildcard)
    prefix: String,
    /// Whether this pattern ends with a wildcard
    is_wildcard: bool,
}

impl ChannelPattern {
    /// Parse a channel pattern (may end with .*)
    pub fn parse(pattern: &str) -> Result<Self, ChannelError> {
        if pattern.is_empty() {
            return Err(ChannelError::Empty);
        }

        // Special case: "*" matches everything
        if pattern == "*" {
            return Ok(Self {
                prefix: String::new(),
                is_wildcard: true,
            });
        }

        // Check for wildcard
        let (prefix, is_wildcard) = if pattern.ends_with(".*") {
            (&pattern[..pattern.len() - 2], true)
        } else if pattern.ends_with('*') {
            // Wildcard without dot prefix, e.g., "project*" - invalid
            return Err(ChannelError::WildcardNotAtEnd);
        } else {
            (pattern, false)
        };

        // Validate the prefix
        for (i, part) in prefix.split('.').enumerate() {
            if part.is_empty() && i > 0 {
                return Err(ChannelError::EmptySegment);
            }

            if part == "*" {
                return Err(ChannelError::WildcardNotAtEnd);
            }

            if !part.is_empty() && !is_valid_segment(part) {
                return Err(ChannelError::InvalidSegment(part.to_string()));
            }
        }

        Ok(Self {
            prefix: prefix.to_string(),
            is_wildcard,
        })
    }

    /// Check if this pattern matches a channel
    ///
    /// This is O(1) for prefix comparison - just a string starts_with check.
    pub fn matches(&self, channel: &Channel) -> bool {
        if self.is_wildcard {
            if self.prefix.is_empty() {
                // Pattern is "*", matches everything
                return true;
            }
            // Pattern is "prefix.*", channel must start with "prefix."
            channel.name.starts_with(&self.prefix)
                && channel.name.len() > self.prefix.len()
                && channel.name.as_bytes()[self.prefix.len()] == b'.'
        } else {
            // Exact match
            channel.name == self.prefix
        }
    }

    /// Get the prefix (without wildcard)
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Check if this is a wildcard pattern
    pub fn is_wildcard(&self) -> bool {
        self.is_wildcard
    }
}

impl fmt::Display for ChannelPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_wildcard {
            if self.prefix.is_empty() {
                write!(f, "*")
            } else {
                write!(f, "{}.*", self.prefix)
            }
        } else {
            write!(f, "{}", self.prefix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_parse_valid() {
        assert!(Channel::parse("project").is_ok());
        assert!(Channel::parse("project.abc123").is_ok());
        assert!(Channel::parse("project.abc123.updates").is_ok());
        assert!(Channel::parse("user.user-123.inbox").is_ok());
        assert!(Channel::parse("agent.worker_5.status").is_ok());
    }

    #[test]
    fn test_channel_parse_invalid() {
        assert!(Channel::parse("").is_err());
        assert!(Channel::parse("project..updates").is_err());
        assert!(Channel::parse("project.*.updates").is_err());
        assert!(Channel::parse("project.abc 123").is_err());
        assert!(Channel::parse("project.abc@123").is_err());
    }

    #[test]
    fn test_pattern_parse_valid() {
        assert!(ChannelPattern::parse("project").is_ok());
        assert!(ChannelPattern::parse("project.abc123").is_ok());
        assert!(ChannelPattern::parse("project.*").is_ok());
        assert!(ChannelPattern::parse("project.abc123.*").is_ok());
        assert!(ChannelPattern::parse("*").is_ok());
    }

    #[test]
    fn test_pattern_parse_invalid() {
        assert!(ChannelPattern::parse("").is_err());
        assert!(ChannelPattern::parse("project*").is_err()); // Missing dot before *
        assert!(ChannelPattern::parse("project.*.updates").is_err()); // Wildcard not at end
    }

    #[test]
    fn test_pattern_matching() {
        let pattern_all = ChannelPattern::parse("*").unwrap();
        let pattern_project = ChannelPattern::parse("project.*").unwrap();
        let pattern_specific = ChannelPattern::parse("project.abc123.*").unwrap();
        let pattern_exact = ChannelPattern::parse("project.abc123.updates").unwrap();

        let channel1 = Channel::parse("project.abc123.updates").unwrap();
        let channel2 = Channel::parse("project.abc123.steps").unwrap();
        let channel3 = Channel::parse("project.xyz.updates").unwrap();
        let channel4 = Channel::parse("user.123.inbox").unwrap();

        // "*" matches everything
        assert!(pattern_all.matches(&channel1));
        assert!(pattern_all.matches(&channel4));

        // "project.*" matches project.anything
        assert!(pattern_project.matches(&channel1));
        assert!(pattern_project.matches(&channel3));
        assert!(!pattern_project.matches(&channel4));

        // "project.abc123.*" matches project.abc123.anything
        assert!(pattern_specific.matches(&channel1));
        assert!(pattern_specific.matches(&channel2));
        assert!(!pattern_specific.matches(&channel3));

        // Exact match
        assert!(pattern_exact.matches(&channel1));
        assert!(!pattern_exact.matches(&channel2));
    }

    #[test]
    fn test_pattern_no_partial_match() {
        // "project.*" should NOT match "projects.abc" (different prefix)
        let pattern = ChannelPattern::parse("project.*").unwrap();
        let channel = Channel::parse("projects.abc").unwrap();
        assert!(!pattern.matches(&channel));
    }
}
