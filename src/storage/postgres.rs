//! PostgreSQL storage backend

use crate::auth::GrantSet;
use crate::storage::{RoleStore, StorageError};
use async_trait::async_trait;
use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;
use tracing::{debug, error, info};

/// Postgres configuration
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
}

impl PostgresConfig {
    pub fn from_env() -> Option<Self> {
        // Try DATABASE_URL first
        if let Ok(url) = std::env::var("DATABASE_URL") {
            return Self::from_url(&url);
        }

        // Fall back to individual vars
        Some(Self {
            host: std::env::var("PGHOST").unwrap_or_else(|_| "localhost".to_string()),
            port: std::env::var("PGPORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(5432),
            user: std::env::var("PGUSER").ok()?,
            password: std::env::var("PGPASSWORD").ok(),
            database: std::env::var("PGDATABASE").ok()?,
        })
    }

    pub fn from_url(url: &str) -> Option<Self> {
        // Basic parsing of postgres://user:pass@host:port/database
        let url = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://"))?;

        let (auth, rest) = url.split_once('@')?;
        let (user, password) = if let Some((u, p)) = auth.split_once(':') {
            (u.to_string(), Some(p.to_string()))
        } else {
            (auth.to_string(), None)
        };

        let (host_port, database) = rest.split_once('/')?;
        let database = database.split('?').next()?.to_string();

        let (host, port) = if let Some((h, p)) = host_port.split_once(':') {
            (h.to_string(), p.parse().ok()?)
        } else {
            (host_port.to_string(), 5432)
        };

        Some(Self {
            host,
            port,
            user,
            password,
            database,
        })
    }
}

/// PostgreSQL storage for roles, grants, and LISTEN/NOTIFY
pub struct PostgresStore {
    pool: Pool,
    config: PostgresConfig,
}

impl PostgresStore {
    /// Create a new PostgresStore
    pub async fn new(config: PostgresConfig) -> Result<Self, StorageError> {
        let mut cfg = Config::new();
        cfg.host = Some(config.host.clone());
        cfg.port = Some(config.port);
        cfg.user = Some(config.user.clone());
        cfg.password = config.password.clone();
        cfg.dbname = Some(config.database.clone());

        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let store = Self { pool, config };
        store.ensure_schema().await?;

        Ok(store)
    }

    /// Ensure database schema exists
    async fn ensure_schema(&self) -> Result<(), StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        client
            .batch_execute(
                r#"
                CREATE TABLE IF NOT EXISTS catbus_roles (
                    id TEXT PRIMARY KEY,
                    name TEXT,
                    grants JSONB NOT NULL DEFAULT '[]',
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );

                CREATE INDEX IF NOT EXISTS catbus_roles_name_idx ON catbus_roles(name);

                -- Function to notify on changes
                CREATE OR REPLACE FUNCTION catbus_notify_change()
                RETURNS TRIGGER AS $$
                BEGIN
                    PERFORM pg_notify('catbus_changes', json_build_object(
                        'table', TG_TABLE_NAME,
                        'action', TG_OP,
                        'id', COALESCE(NEW.id, OLD.id)
                    )::text);
                    RETURN NEW;
                END;
                $$ LANGUAGE plpgsql;

                -- Trigger for role changes
                DROP TRIGGER IF EXISTS catbus_roles_notify ON catbus_roles;
                CREATE TRIGGER catbus_roles_notify
                    AFTER INSERT OR UPDATE OR DELETE ON catbus_roles
                    FOR EACH ROW EXECUTE FUNCTION catbus_notify_change();

                -- Message history table (for catch-up)
                CREATE TABLE IF NOT EXISTS catbus_messages (
                    id BIGSERIAL PRIMARY KEY,
                    channel TEXT NOT NULL,
                    payload JSONB NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );

                CREATE INDEX IF NOT EXISTS catbus_messages_channel_idx ON catbus_messages(channel);
                CREATE INDEX IF NOT EXISTS catbus_messages_created_idx ON catbus_messages(created_at);

                -- Trigger for message notification
                DROP TRIGGER IF EXISTS catbus_messages_notify ON catbus_messages;
                CREATE TRIGGER catbus_messages_notify
                    AFTER INSERT ON catbus_messages
                    FOR EACH ROW
                    EXECUTE FUNCTION catbus_notify_change();
                "#,
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        info!("Database schema initialized");
        Ok(())
    }

    /// Store a message for history (returns message ID)
    pub async fn store_message(
        &self,
        channel: &str,
        payload: &serde_json::Value,
    ) -> Result<i64, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let row = client
            .query_one(
                "INSERT INTO catbus_messages (channel, payload) VALUES ($1, $2) RETURNING id",
                &[&channel, payload],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(row.get(0))
    }

    /// Get recent messages on a channel (for catch-up)
    pub async fn get_recent_messages(
        &self,
        channel_pattern: &str,
        limit: i64,
        after_id: Option<i64>,
    ) -> Result<Vec<(i64, String, serde_json::Value)>, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let pattern = channel_pattern.replace('*', "%");

        let rows = if let Some(after) = after_id {
            client
                .query(
                    "SELECT id, channel, payload FROM catbus_messages
                     WHERE channel LIKE $1 AND id > $2
                     ORDER BY id ASC LIMIT $3",
                    &[&pattern, &after, &limit],
                )
                .await
        } else {
            client
                .query(
                    "SELECT id, channel, payload FROM catbus_messages
                     WHERE channel LIKE $1
                     ORDER BY id DESC LIMIT $2",
                    &[&pattern, &limit],
                )
                .await
        }
        .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(rows
            .iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect())
    }

    /// Get a connection string for direct connections (needed for LISTEN)
    fn connection_string(&self) -> String {
        let mut s = format!(
            "host={} port={} user={} dbname={}",
            self.config.host, self.config.port, self.config.user, self.config.database
        );
        if let Some(ref pass) = self.config.password {
            s.push_str(&format!(" password={}", pass));
        }
        s
    }

    /// Subscribe to LISTEN notifications
    /// Returns a handle that can be used to receive notifications
    pub async fn listen(&self) -> Result<tokio_postgres::Client, StorageError> {
        let (client, connection) = tokio_postgres::connect(&self.connection_string(), NoTls)
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        // Spawn connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!(error = %e, "Postgres LISTEN connection error");
            }
        });

        client
            .execute("LISTEN catbus_changes", &[])
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(client)
    }

    /// Publish a message via NOTIFY (for cross-instance communication)
    pub async fn notify(&self, channel: &str, payload: &str) -> Result<(), StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        client
            .execute(
                "SELECT pg_notify($1, $2)",
                &[&format!("catbus:{}", channel), &payload],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(())
    }

    /// Clean up old messages
    pub async fn cleanup_messages(&self, older_than_hours: i32) -> Result<u64, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let result = client
            .execute(
                "DELETE FROM catbus_messages WHERE created_at < NOW() - $1 * INTERVAL '1 hour'",
                &[&older_than_hours],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(result)
    }
}

#[async_trait]
impl RoleStore for PostgresStore {
    async fn create_role(&self, name: Option<&str>) -> Result<String, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        // Generate role ID
        let role_token = crate::auth::RoleToken::generate();
        let id = role_token.id().to_string();

        client
            .execute(
                "INSERT INTO catbus_roles (id, name) VALUES ($1, $2)",
                &[&id, &name],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        debug!(role_id = %id, name = ?name, "Created role");
        Ok(id)
    }

    async fn get_grants(&self, role_id: &str) -> Result<GrantSet, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let row = client
            .query_opt("SELECT grants FROM catbus_roles WHERE id = $1", &[&role_id])
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?
            .ok_or_else(|| StorageError::NotFound(format!("Role not found: {}", role_id)))?;

        let grants_json: serde_json::Value = row.get(0);
        let grants: GrantSet = serde_json::from_value(grants_json)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        Ok(grants)
    }

    async fn add_grants(&self, role_id: &str, grants: &GrantSet) -> Result<(), StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        // Get existing grants
        let mut existing = self.get_grants(role_id).await?;

        // Add new grants
        for grant in grants.grants() {
            existing.add(grant.clone());
        }

        let grants_json = serde_json::to_value(&existing)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        client
            .execute(
                "UPDATE catbus_roles SET grants = $1, updated_at = NOW() WHERE id = $2",
                &[&grants_json, &role_id],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(())
    }

    async fn remove_grants(&self, role_id: &str, grants: &GrantSet) -> Result<(), StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        // Get existing grants
        let mut existing = self.get_grants(role_id).await?;

        // Remove specified grants
        for grant in grants.grants() {
            existing.remove(grant);
        }

        let grants_json = serde_json::to_value(&existing)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        client
            .execute(
                "UPDATE catbus_roles SET grants = $1, updated_at = NOW() WHERE id = $2",
                &[&grants_json, &role_id],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(())
    }

    async fn revoke_role(&self, role_id: &str) -> Result<(), StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let result = client
            .execute("DELETE FROM catbus_roles WHERE id = $1", &[&role_id])
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        if result == 0 {
            return Err(StorageError::NotFound(format!("Role not found: {}", role_id)));
        }

        debug!(role_id = %role_id, "Revoked role");
        Ok(())
    }

    async fn role_exists(&self, role_id: &str) -> Result<bool, StorageError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        let row = client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM catbus_roles WHERE id = $1)",
                &[&role_id],
            )
            .await
            .map_err(|e| StorageError::Database(e.to_string()))?;

        Ok(row.get(0))
    }
}
