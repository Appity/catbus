//! Catbus CLI entry point

mod cli;

use crate::cli::{Cli, Commands, RoleCommands};
use anyhow::{Context, Result};
use catbus::auth::{AdminKey, Grant, GrantSet, GrantType, RoleToken, SubToken};
use catbus::channels::ChannelPattern;
use catbus::server::{CatbusServer, CatbusServerConfig};
use catbus::storage::{PostgresConfig, PostgresStore, RoleStore};
use clap::Parser;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    // Helper to get database config lazily (only when needed)
    let get_db_config = || -> Result<PostgresConfig> {
        if let Some(url) = &cli.database_url {
            PostgresConfig::from_url(url).context("Invalid DATABASE_URL")
        } else {
            PostgresConfig::from_env().context("DATABASE_URL not set")
        }
    };

    match cli.command {
        Commands::Serve { bind, cert, key, secret } => {
            serve(get_db_config()?, bind, cert, key, secret, cli.admin_key).await
        }
        Commands::Grant { permission, patterns, to, client_id } => {
            grant(get_db_config, permission, patterns, to, client_id, cli.admin_key).await
        }
        Commands::Revoke { permission, patterns, from } => {
            revoke(get_db_config()?, permission, patterns, from).await
        }
        Commands::Role { command } => role(get_db_config()?, command).await,
        Commands::Init => init(get_db_config()?).await,
        Commands::Status => status(get_db_config()?).await,
    }
}

async fn serve(
    db_config: PostgresConfig,
    bind: String,
    cert_path: String,
    key_path: String,
    secret: String,
    admin_key: Option<String>,
) -> Result<()> {
    let store = Arc::new(PostgresStore::new(db_config).await?);

    let cert = std::fs::read_to_string(&cert_path)
        .with_context(|| format!("Failed to read certificate: {}", cert_path))?;
    let key = std::fs::read_to_string(&key_path)
        .with_context(|| format!("Failed to read private key: {}", key_path))?;

    let bind_addr = bind.parse().context("Invalid bind address")?;

    let config = CatbusServerConfig {
        bind_addr,
        cert_pem: cert,
        key_pem: key,
        token_secret: secret.into_bytes(),
        admin_key: admin_key.map(AdminKey::new),
    };

    let server = CatbusServer::new(config, store);

    info!("Starting Catbus server...");
    server.run().await?;

    Ok(())
}

async fn grant<F>(
    get_db_config: F,
    permissions: Vec<String>,
    patterns: Vec<String>,
    to: Option<String>,
    client_id: Option<String>,
    admin_key: Option<String>,
) -> Result<()>
where
    F: FnOnce() -> Result<PostgresConfig>,
{
    // Build grants
    let mut grants = GrantSet::new();

    for perm_str in &permissions {
        let grant_types = GrantType::parse_all(perm_str)
            .ok_or_else(|| anyhow::anyhow!("Invalid permission: {}", perm_str))?;

        for pattern_str in &patterns {
            let pattern = ChannelPattern::parse(pattern_str)
                .with_context(|| format!("Invalid pattern: {}", pattern_str))?;

            for grant_type in &grant_types {
                grants.add(Grant::new(*grant_type, pattern.clone()));
            }
        }
    }

    if let Some(role_token_str) = to {
        // Add to existing role - requires database
        let role_token = RoleToken::parse(&role_token_str)
            .context("Invalid role token")?;

        let db_config = get_db_config()?;
        let store = PostgresStore::new(db_config).await?;
        store.add_grants(role_token.id(), &grants).await?;

        println!("Added grants to role {}", role_token_str);
        for grant in grants.grants() {
            println!("  + {}", grant);
        }
    } else {
        // Create new sub token - no database needed
        let secret = admin_key
            .or_else(|| std::env::var("CATBUS_SECRET").ok())
            .context("CATBUS_SECRET or --admin-key required to create tokens")?;

        let cid = client_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let token = SubToken::create(cid.clone(), grants.clone(), secret.as_bytes());

        println!("{}", token);
        println!();
        println!("Client ID: {}", cid);
        println!("Grants:");
        for grant in grants.grants() {
            println!("  {}", grant);
        }
    }

    Ok(())
}

async fn revoke(
    db_config: PostgresConfig,
    permissions: Vec<String>,
    patterns: Vec<String>,
    from: String,
) -> Result<()> {
    let role_token = RoleToken::parse(&from).context("Invalid role token")?;

    // Build grants to remove
    let mut grants = GrantSet::new();

    for perm_str in &permissions {
        let grant_types = GrantType::parse_all(perm_str)
            .ok_or_else(|| anyhow::anyhow!("Invalid permission: {}", perm_str))?;

        for pattern_str in &patterns {
            let pattern = ChannelPattern::parse(pattern_str)
                .with_context(|| format!("Invalid pattern: {}", pattern_str))?;

            for grant_type in &grant_types {
                grants.add(Grant::new(*grant_type, pattern.clone()));
            }
        }
    }

    let store = PostgresStore::new(db_config).await?;
    store.remove_grants(role_token.id(), &grants).await?;

    println!("Removed grants from role {}", from);
    for grant in grants.grants() {
        println!("  - {}", grant);
    }

    Ok(())
}

async fn role(db_config: PostgresConfig, command: RoleCommands) -> Result<()> {
    let store = PostgresStore::new(db_config).await?;

    match command {
        RoleCommands::Create { name } => {
            let role_id = store.create_role(name.as_deref()).await?;
            let token = format!("role-{}", role_id);

            println!("{}", token);
            if let Some(n) = name {
                println!("Name: {}", n);
            }
        }
        RoleCommands::Show { token } => {
            let role_token = RoleToken::parse(&token).context("Invalid role token")?;
            let grants = store.get_grants(role_token.id()).await?;

            println!("Role: {}", token);
            println!("Grants:");
            if grants.is_empty() {
                println!("  (none)");
            } else {
                for grant in grants.grants() {
                    println!("  {}", grant);
                }
            }
        }
        RoleCommands::Revoke { token } => {
            let role_token = RoleToken::parse(&token).context("Invalid role token")?;
            store.revoke_role(role_token.id()).await?;

            println!("Revoked role: {}", token);
        }
        RoleCommands::List => {
            // TODO: Implement listing roles
            println!("Listing roles is not yet implemented");
        }
    }

    Ok(())
}

async fn init(db_config: PostgresConfig) -> Result<()> {
    let _store = PostgresStore::new(db_config).await?;
    println!("Database schema initialized successfully");
    Ok(())
}

async fn status(db_config: PostgresConfig) -> Result<()> {
    let _store = PostgresStore::new(db_config).await?;

    println!("Catbus Status");
    println!("=============");
    println!("Database: Connected");

    // TODO: Add more status info (role count, etc.)

    Ok(())
}
