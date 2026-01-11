//! CLI command definitions

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "catbus")]
#[command(about = "WebTransport-based message bus", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Database URL
    #[arg(long, env = "DATABASE_URL", global = true)]
    pub database_url: Option<String>,

    /// Admin key for authentication
    #[arg(long, env = "CATBUS_ADMIN_KEY", global = true)]
    pub admin_key: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the Catbus server
    Serve {
        /// Address to bind to
        #[arg(short, long, default_value = "0.0.0.0:4433")]
        bind: String,

        /// Path to TLS certificate (PEM)
        #[arg(long, env = "CATBUS_CERT")]
        cert: String,

        /// Path to TLS private key (PEM)
        #[arg(long, env = "CATBUS_KEY")]
        key: String,

        /// Token signing secret
        #[arg(long, env = "CATBUS_SECRET")]
        secret: String,
    },

    /// Grant permissions (creates a token)
    ///
    /// Examples:
    ///   catbus grant -p all 'project.*'
    ///   catbus grant -p read -p write 'user.123.*' 'user.456.*'
    ///   catbus grant -p read 'events.*' --to role-abc123
    Grant {
        /// Permission type: read, write, create, or all (can be repeated)
        #[arg(short, long = "permission", value_parser = parse_grant_type, required = true)]
        permission: Vec<String>,

        /// Channel pattern(s) to grant access to
        #[arg(required = true)]
        patterns: Vec<String>,

        /// Add to existing role token instead of creating new sub token
        #[arg(long)]
        to: Option<String>,

        /// Client ID for intrinsic channels (only for new sub tokens)
        #[arg(long)]
        client_id: Option<String>,
    },

    /// Revoke permissions from a role
    ///
    /// Examples:
    ///   catbus revoke -p write 'project.*' --from role-abc123
    Revoke {
        /// Permission type: read, write, create, or all (can be repeated)
        #[arg(short, long = "permission", value_parser = parse_grant_type, required = true)]
        permission: Vec<String>,

        /// Channel pattern(s) to revoke
        #[arg(required = true)]
        patterns: Vec<String>,

        /// Role token to revoke from
        #[arg(long, required = true)]
        from: String,
    },

    /// Manage roles
    Role {
        #[command(subcommand)]
        command: RoleCommands,
    },

    /// Initialize the database schema
    Init,

    /// Show server status and statistics
    Status,
}

#[derive(Subcommand)]
pub enum RoleCommands {
    /// Create a new role
    Create {
        /// Optional name for the role
        #[arg(long)]
        name: Option<String>,
    },

    /// Show role details
    Show {
        /// Role token
        token: String,
    },

    /// Revoke (delete) a role
    Revoke {
        /// Role token
        token: String,
    },

    /// List all roles
    List,
}

fn parse_grant_type(s: &str) -> Result<String, String> {
    match s.to_lowercase().as_str() {
        "read" | "write" | "create" | "all" => Ok(s.to_lowercase()),
        _ => Err(format!("Invalid permission type: {}. Must be read, write, create, or all", s)),
    }
}
