//! Token types and validation
//!
//! Token prefixes:
//! - `sub-` : Stateless subscription token (self-contained, signed)
//! - `role-` : Stateful role token (ID lookup in storage)

use crate::auth::grants::GrantSet;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;
use thiserror::Error;

/// Token prefixes
pub const SUB_TOKEN_PREFIX: &str = "sub-";
pub const ROLE_TOKEN_PREFIX: &str = "role-";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("invalid token format")]
    InvalidFormat,

    #[error("invalid token prefix: expected '{expected}', got '{got}'")]
    InvalidPrefix { expected: String, got: String },

    #[error("invalid token signature")]
    InvalidSignature,

    #[error("token decode error: {0}")]
    DecodeError(String),

    #[error("token not found")]
    NotFound,
}

/// Admin key for full access
#[derive(Clone)]
pub struct AdminKey {
    key: String,
}

impl AdminKey {
    pub fn new(key: String) -> Self {
        Self { key }
    }

    pub fn from_env() -> Option<Self> {
        std::env::var("CATBUS_ADMIN_KEY").ok().map(Self::new)
    }

    pub fn matches(&self, token: &str) -> bool {
        // Constant-time comparison to prevent timing attacks
        if token.len() != self.key.len() {
            return false;
        }

        let mut result = 0u8;
        for (a, b) in token.bytes().zip(self.key.bytes()) {
            result |= a ^ b;
        }
        result == 0
    }

    /// Get the key (for internal use)
    pub fn as_str(&self) -> &str {
        &self.key
    }
}

impl fmt::Debug for AdminKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AdminKey([REDACTED])")
    }
}

/// Payload stored in a stateless subscription token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTokenPayload {
    /// Client ID (for intrinsic channel access)
    pub client_id: String,
    /// Grants encoded in the token
    pub grants: GrantSet,
}

/// A stateless subscription token
#[derive(Debug, Clone)]
pub struct SubToken {
    /// The full token string including prefix
    token: String,
    /// Decoded payload
    payload: SubTokenPayload,
}

impl SubToken {
    /// Create a new subscription token
    pub fn create(client_id: String, grants: GrantSet, secret: &[u8]) -> Self {
        let payload = SubTokenPayload { client_id, grants };
        let payload_json = serde_json::to_vec(&payload).expect("serialize payload");
        let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_json);

        // Sign: HMAC-SHA256(secret, payload)
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(payload_b64.as_bytes());
        let signature = mac.finalize().into_bytes();
        let signature_b64 = URL_SAFE_NO_PAD.encode(&signature[..16]); // Use first 16 bytes

        let token = format!("{}{}.{}", SUB_TOKEN_PREFIX, payload_b64, signature_b64);

        Self { token, payload }
    }

    /// Parse and verify a subscription token
    pub fn parse(token: &str, secret: &[u8]) -> Result<Self, TokenError> {
        if !token.starts_with(SUB_TOKEN_PREFIX) {
            return Err(TokenError::InvalidPrefix {
                expected: SUB_TOKEN_PREFIX.to_string(),
                got: token.chars().take(4).collect(),
            });
        }

        let content = &token[SUB_TOKEN_PREFIX.len()..];
        let parts: Vec<&str> = content.split('.').collect();

        if parts.len() != 2 {
            return Err(TokenError::InvalidFormat);
        }

        let payload_b64 = parts[0];
        let signature_b64 = parts[1];

        // Verify signature
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(payload_b64.as_bytes());
        let expected_sig = mac.finalize().into_bytes();
        let expected_sig_b64 = URL_SAFE_NO_PAD.encode(&expected_sig[..16]);

        if signature_b64 != expected_sig_b64 {
            return Err(TokenError::InvalidSignature);
        }

        // Decode payload
        let payload_json = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|e| TokenError::DecodeError(e.to_string()))?;

        let payload: SubTokenPayload = serde_json::from_slice(&payload_json)
            .map_err(|e| TokenError::DecodeError(e.to_string()))?;

        Ok(Self {
            token: token.to_string(),
            payload,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.token
    }

    pub fn client_id(&self) -> &str {
        &self.payload.client_id
    }

    pub fn grants(&self) -> &GrantSet {
        &self.payload.grants
    }
}

impl fmt::Display for SubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.token)
    }
}

/// A stateful role token (ID only, grants stored in database)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoleToken {
    /// The full token string including prefix
    token: String,
    /// The role ID (random bytes, base64 encoded)
    id: String,
}

impl RoleToken {
    /// Generate a new role token
    pub fn generate() -> Self {
        let mut rng = rand::rng();
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);

        let id = URL_SAFE_NO_PAD.encode(bytes);
        let token = format!("{}{}", ROLE_TOKEN_PREFIX, id);

        Self { token, id }
    }

    /// Parse a role token (no verification needed, just format check)
    pub fn parse(token: &str) -> Result<Self, TokenError> {
        if !token.starts_with(ROLE_TOKEN_PREFIX) {
            return Err(TokenError::InvalidPrefix {
                expected: ROLE_TOKEN_PREFIX.to_string(),
                got: token.chars().take(5).collect(),
            });
        }

        let id = &token[ROLE_TOKEN_PREFIX.len()..];

        // Validate it's valid base64
        URL_SAFE_NO_PAD
            .decode(id)
            .map_err(|e| TokenError::DecodeError(e.to_string()))?;

        Ok(Self {
            token: token.to_string(),
            id: id.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.token
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for RoleToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.token)
    }
}

/// Unified token type
#[derive(Debug, Clone)]
pub enum Token {
    Admin,
    Sub(SubToken),
    Role(RoleToken),
}

impl Token {
    /// Parse any token type
    pub fn parse(token: &str, secret: &[u8], admin_key: Option<&AdminKey>) -> Result<Self, TokenError> {
        // Check admin key first
        if let Some(admin) = admin_key {
            if admin.matches(token) {
                return Ok(Token::Admin);
            }
        }

        // Try sub token
        if token.starts_with(SUB_TOKEN_PREFIX) {
            return SubToken::parse(token, secret).map(Token::Sub);
        }

        // Try role token
        if token.starts_with(ROLE_TOKEN_PREFIX) {
            return RoleToken::parse(token).map(Token::Role);
        }

        Err(TokenError::InvalidFormat)
    }

    /// Get the client ID if this is a sub token
    pub fn client_id(&self) -> Option<&str> {
        match self {
            Token::Sub(t) => Some(t.client_id()),
            _ => None,
        }
    }

    /// Check if this is an admin token
    pub fn is_admin(&self) -> bool {
        matches!(self, Token::Admin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::grants::{Grant, GrantType};
    use crate::channels::ChannelPattern;

    const TEST_SECRET: &[u8] = b"test-secret-key-for-signing";

    #[test]
    fn test_sub_token_create_parse() {
        let mut grants = GrantSet::new();
        grants.add(Grant::new(
            GrantType::Read,
            ChannelPattern::parse("project.abc.*").unwrap(),
        ));

        let token = SubToken::create("client-123".to_string(), grants, TEST_SECRET);
        assert!(token.as_str().starts_with(SUB_TOKEN_PREFIX));

        let parsed = SubToken::parse(token.as_str(), TEST_SECRET).unwrap();
        assert_eq!(parsed.client_id(), "client-123");
        assert!(!parsed.grants().is_empty());
    }

    #[test]
    fn test_sub_token_invalid_signature() {
        let grants = GrantSet::new();
        let token = SubToken::create("client-123".to_string(), grants, TEST_SECRET);

        // Try to parse with wrong secret
        let result = SubToken::parse(token.as_str(), b"wrong-secret");
        assert!(matches!(result, Err(TokenError::InvalidSignature)));
    }

    #[test]
    fn test_role_token_generate_parse() {
        let token = RoleToken::generate();
        assert!(token.as_str().starts_with(ROLE_TOKEN_PREFIX));

        let parsed = RoleToken::parse(token.as_str()).unwrap();
        assert_eq!(parsed.id(), token.id());
    }

    #[test]
    fn test_admin_key() {
        let admin = AdminKey::new("my-secret-admin-key".to_string());
        assert!(admin.matches("my-secret-admin-key"));
        assert!(!admin.matches("wrong-key"));
        assert!(!admin.matches("my-secret-admin-key-extra"));
    }

    #[test]
    fn test_token_parse_unified() {
        let admin = AdminKey::new("admin-key".to_string());
        let grants = GrantSet::new();
        let sub = SubToken::create("client".to_string(), grants, TEST_SECRET);
        let role = RoleToken::generate();

        // Admin
        let t = Token::parse("admin-key", TEST_SECRET, Some(&admin)).unwrap();
        assert!(t.is_admin());

        // Sub
        let t = Token::parse(sub.as_str(), TEST_SECRET, Some(&admin)).unwrap();
        assert!(matches!(t, Token::Sub(_)));

        // Role
        let t = Token::parse(role.as_str(), TEST_SECRET, Some(&admin)).unwrap();
        assert!(matches!(t, Token::Role(_)));
    }
}
