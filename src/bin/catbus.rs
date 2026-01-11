//! Catbus CLI - Token and role management tool

use anyhow::{Context, Result};
use catbus::auth::{Grant, GrantSet, GrantType, RoleToken, SubToken};
use catbus::channels::ChannelPattern;
use catbus::storage::{PostgresConfig, PostgresStore, RoleStore};
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "catbus")]
#[command(about = "Catbus CLI - Token and role management")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Database URL (required for role operations)
    #[arg(long, env = "DATABASE_URL", global = true)]
    database_url: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a token with grants
    ///
    /// Creates a stateless sub-token, or adds grants to an existing role.
    ///
    /// Examples:
    ///   catbus grant -p all 'project.*'
    ///   catbus grant -p read -p write 'user.123.*' 'events.*'
    ///   catbus grant -p read 'audit.*' --to role-abc123
    Grant {
        /// Permission type: read, write, create, or all (can be repeated)
        #[arg(short, long = "permission", value_parser = parse_permission, required = true)]
        permission: Vec<String>,

        /// Channel pattern(s) to grant access to
        #[arg(required = true)]
        patterns: Vec<String>,

        /// Add to existing role token instead of creating new sub token
        #[arg(long)]
        to: Option<String>,

        /// Client ID for the token (auto-generated if not specified)
        #[arg(long)]
        client_id: Option<String>,

        /// Token signing secret (required for creating sub tokens)
        #[arg(long, env = "CATBUS_SECRET")]
        secret: Option<String>,
    },

    /// Revoke grants from a role
    ///
    /// Examples:
    ///   catbus revoke -p write 'project.*' --from role-abc123
    Revoke {
        /// Permission type: read, write, create, or all (can be repeated)
        #[arg(short, long = "permission", value_parser = parse_permission, required = true)]
        permission: Vec<String>,

        /// Channel pattern(s) to revoke
        #[arg(required = true)]
        patterns: Vec<String>,

        /// Role token to revoke from
        #[arg(long, required = true)]
        from: String,
    },

    /// Manage roles
    #[command(subcommand)]
    Role(RoleCommands),

    /// Initialize the database schema
    Init,

    /// Show server/database status
    Status,
}

#[derive(Subcommand)]
enum RoleCommands {
    /// Create a new role
    Create {
        /// Optional name for the role
        #[arg(long)]
        name: Option<String>,
    },

    /// Show role details and grants
    Show {
        /// Role token (role-xxx)
        token: String,
    },

    /// Delete a role (immediately invalidates the token)
    Delete {
        /// Role token (role-xxx)
        token: String,
    },

    /// List all roles
    List,
}

fn parse_permission(s: &str) -> Result<String, String> {
    match s.to_lowercase().as_str() {
        "read" | "write" | "create" | "all" => Ok(s.to_lowercase()),
        _ => Err(format!(
            "Invalid permission: '{}'. Must be: read, write, create, or all",
            s
        )),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize minimal tracing for CLI
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
        .with(tracing_subscriber::fmt::layer().without_time())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Grant {
            permission,
            patterns,
            to,
            client_id,
            secret,
        } => grant(cli.database_url, permission, patterns, to, client_id, secret).await,

        Commands::Revoke {
            permission,
            patterns,
            from,
        } => revoke(cli.database_url, permission, patterns, from).await,

        Commands::Role(cmd) => role(cli.database_url, cmd).await,

        Commands::Init => init(cli.database_url).await,

        Commands::Status => status(cli.database_url).await,
    }
}

fn build_grants(permissions: &[String], patterns: &[String]) -> Result<GrantSet> {
    let mut grants = GrantSet::new();

    for perm_str in permissions {
        let grant_types = GrantType::parse_all(perm_str)
            .ok_or_else(|| anyhow::anyhow!("Invalid permission: {}", perm_str))?;

        for pattern_str in patterns {
            let pattern = ChannelPattern::parse(pattern_str)
                .with_context(|| format!("Invalid pattern: {}", pattern_str))?;

            for grant_type in &grant_types {
                grants.add(Grant::new(*grant_type, pattern.clone()));
            }
        }
    }

    Ok(grants)
}

fn get_db_config(database_url: Option<String>) -> Result<PostgresConfig> {
    let url = database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .context("DATABASE_URL is required for this operation")?;
    PostgresConfig::from_url(&url).context("Invalid DATABASE_URL")
}

async fn grant(
    database_url: Option<String>,
    permissions: Vec<String>,
    patterns: Vec<String>,
    to: Option<String>,
    client_id: Option<String>,
    secret: Option<String>,
) -> Result<()> {
    let grants = build_grants(&permissions, &patterns)?;

    if let Some(role_token_str) = to {
        // Add to existing role - requires database
        let role_token = RoleToken::parse(&role_token_str).context("Invalid role token")?;

        let db_config = get_db_config(database_url)?;
        let store = PostgresStore::new(db_config).await?;
        store.add_grants(role_token.id(), &grants).await?;

        println!("Added grants to role {}", role_token_str);
        for grant in grants.grants() {
            println!("  + {}", grant);
        }
    } else {
        // Create new sub token - no database needed
        let secret = secret
            .or_else(|| std::env::var("CATBUS_SECRET").ok())
            .context("--secret or CATBUS_SECRET is required to create tokens")?;

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
    database_url: Option<String>,
    permissions: Vec<String>,
    patterns: Vec<String>,
    from: String,
) -> Result<()> {
    let role_token = RoleToken::parse(&from).context("Invalid role token")?;
    let grants = build_grants(&permissions, &patterns)?;

    let db_config = get_db_config(database_url)?;
    let store = PostgresStore::new(db_config).await?;
    store.remove_grants(role_token.id(), &grants).await?;

    println!("Removed grants from role {}", from);
    for grant in grants.grants() {
        println!("  - {}", grant);
    }

    Ok(())
}

async fn role(database_url: Option<String>, command: RoleCommands) -> Result<()> {
    let db_config = get_db_config(database_url)?;
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

        RoleCommands::Delete { token } => {
            let role_token = RoleToken::parse(&token).context("Invalid role token")?;
            store.revoke_role(role_token.id()).await?;
            println!("Deleted role: {}", token);
        }

        RoleCommands::List => {
            // TODO: Implement listing roles
            println!("Role listing is not yet implemented");
        }
    }

    Ok(())
}

async fn init(database_url: Option<String>) -> Result<()> {
    let db_config = get_db_config(database_url)?;
    let _store = PostgresStore::new(db_config).await?;
    println!("Database schema initialized successfully");
    Ok(())
}

async fn status(database_url: Option<String>) -> Result<()> {
    let db_config = get_db_config(database_url)?;
    let _store = PostgresStore::new(db_config).await?;

    println!("Catbus Status");
    println!("=============");
    println!("Database: Connected");

    // TODO: Add more status info (role count, server connectivity, etc.)

    Ok(())
}
