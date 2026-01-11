//! Storage backends for Catbus
//!
//! - Postgres: Durable storage for roles, grants, and message history
//! - Ring buffer: In-memory catch-up buffer for recent messages

mod postgres;
mod ringbuf;

pub use postgres::{PostgresConfig, PostgresStore};
pub use ringbuf::{BufferedMessage, RingBuffer};

use crate::auth::GrantSet;
pub use async_trait::async_trait;
use thiserror::Error;

/// Storage errors
#[derive(Debug, Clone, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Trait for role token storage
#[async_trait]
pub trait RoleStore: Send + Sync {
    /// Create a new role, returns the role ID
    async fn create_role(&self, name: Option<&str>) -> Result<String, StorageError>;

    /// Get grants for a role
    async fn get_grants(&self, role_id: &str) -> Result<GrantSet, StorageError>;

    /// Add grants to a role
    async fn add_grants(&self, role_id: &str, grants: &GrantSet) -> Result<(), StorageError>;

    /// Remove grants from a role
    async fn remove_grants(&self, role_id: &str, grants: &GrantSet) -> Result<(), StorageError>;

    /// Revoke (delete) a role entirely
    async fn revoke_role(&self, role_id: &str) -> Result<(), StorageError>;

    /// Check if a role exists
    async fn role_exists(&self, role_id: &str) -> Result<bool, StorageError>;
}
