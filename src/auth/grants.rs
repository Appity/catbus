//! Permission grants

use crate::channels::{Channel, ChannelPattern};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Types of permissions that can be granted
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantType {
    /// Receive events from matching channels
    Read,
    /// Push events to matching channels
    Write,
    /// Create new channels in that namespace
    Create,
}

impl GrantType {
    /// Parse from string, including "all" which expands to all types
    pub fn parse_all(s: &str) -> Option<Vec<GrantType>> {
        match s.to_lowercase().as_str() {
            "read" => Some(vec![GrantType::Read]),
            "write" => Some(vec![GrantType::Write]),
            "create" => Some(vec![GrantType::Create]),
            "all" => Some(vec![GrantType::Read, GrantType::Write, GrantType::Create]),
            _ => None,
        }
    }
}

impl fmt::Display for GrantType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrantType::Read => write!(f, "read"),
            GrantType::Write => write!(f, "write"),
            GrantType::Create => write!(f, "create"),
        }
    }
}

/// A single grant: permission type + channel pattern
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub grant_type: GrantType,
    #[serde(with = "channel_pattern_serde")]
    pub pattern: ChannelPattern,
}

impl Grant {
    pub fn new(grant_type: GrantType, pattern: ChannelPattern) -> Self {
        Self { grant_type, pattern }
    }

    /// Check if this grant allows the given operation on the given channel
    pub fn allows(&self, grant_type: GrantType, channel: &Channel) -> bool {
        self.grant_type == grant_type && self.pattern.matches(channel)
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.grant_type, self.pattern)
    }
}

/// A collection of grants for efficient permission checking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantSet {
    grants: Vec<Grant>,
}

impl GrantSet {
    pub fn new() -> Self {
        Self { grants: Vec::new() }
    }

    /// Add a grant to the set
    pub fn add(&mut self, grant: Grant) {
        // Avoid duplicates
        if !self.grants.iter().any(|g| g == &grant) {
            self.grants.push(grant);
        }
    }

    /// Add multiple grants
    pub fn add_all(&mut self, grants: impl IntoIterator<Item = Grant>) {
        for grant in grants {
            self.add(grant);
        }
    }

    /// Remove a grant from the set
    pub fn remove(&mut self, grant: &Grant) {
        self.grants.retain(|g| g != grant);
    }

    /// Check if any grant allows the given operation on the given channel
    pub fn allows(&self, grant_type: GrantType, channel: &Channel) -> bool {
        self.grants.iter().any(|g| g.allows(grant_type, channel))
    }

    /// Check if allowed to read from a channel
    pub fn can_read(&self, channel: &Channel) -> bool {
        self.allows(GrantType::Read, channel)
    }

    /// Check if allowed to write to a channel
    pub fn can_write(&self, channel: &Channel) -> bool {
        self.allows(GrantType::Write, channel)
    }

    /// Check if allowed to create channels under this pattern
    pub fn can_create(&self, channel: &Channel) -> bool {
        self.allows(GrantType::Create, channel)
    }

    /// Get all grants
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// Check if the set is empty
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// Get unique patterns for a grant type (for subscription routing)
    pub fn patterns_for(&self, grant_type: GrantType) -> Vec<&ChannelPattern> {
        self.grants
            .iter()
            .filter(|g| g.grant_type == grant_type)
            .map(|g| &g.pattern)
            .collect()
    }
}

impl FromIterator<Grant> for GrantSet {
    fn from_iter<T: IntoIterator<Item = Grant>>(iter: T) -> Self {
        let mut set = GrantSet::new();
        set.add_all(iter);
        set
    }
}

/// Serde helper for ChannelPattern
mod channel_pattern_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(pattern: &ChannelPattern, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&pattern.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ChannelPattern, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ChannelPattern::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_allows() {
        let pattern = ChannelPattern::parse("project.abc.*").unwrap();
        let grant = Grant::new(GrantType::Read, pattern);

        let channel_ok = Channel::parse("project.abc.updates").unwrap();
        let channel_bad = Channel::parse("project.xyz.updates").unwrap();

        assert!(grant.allows(GrantType::Read, &channel_ok));
        assert!(!grant.allows(GrantType::Read, &channel_bad));
        assert!(!grant.allows(GrantType::Write, &channel_ok)); // Wrong type
    }

    #[test]
    fn test_grant_set() {
        let mut set = GrantSet::new();

        set.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("project.abc.*").unwrap(),
        ));
        set.add(Grant::new(
            GrantType::Write,
            ChannelPattern::parse("client.me.*").unwrap(),
        ));

        let channel1 = Channel::parse("project.abc.updates").unwrap();
        let channel2 = Channel::parse("client.me.typing").unwrap();
        let channel3 = Channel::parse("other.stuff").unwrap();

        assert!(set.can_read(&channel1));
        assert!(!set.can_write(&channel1));

        assert!(!set.can_read(&channel2));
        assert!(set.can_write(&channel2));

        assert!(!set.can_read(&channel3));
        assert!(!set.can_write(&channel3));
    }

    #[test]
    fn test_grant_type_parse_all() {
        assert_eq!(GrantType::parse_all("read"), Some(vec![GrantType::Read]));
        assert_eq!(GrantType::parse_all("WRITE"), Some(vec![GrantType::Write]));
        assert_eq!(
            GrantType::parse_all("all"),
            Some(vec![GrantType::Read, GrantType::Write, GrantType::Create])
        );
        assert_eq!(GrantType::parse_all("invalid"), None);
    }
}
