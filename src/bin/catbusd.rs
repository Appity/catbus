//! Catbus daemon - WebTransport and WebSocket message bus server

use anyhow::{Context, Result};
use catbus::auth::AdminKey;
use catbus::server::{CatbusServer, CatbusServerConfig, WsState, run_websocket_server};
use catbus::storage::{PostgresConfig, PostgresStore};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "catbusd")]
#[command(about = "Catbus WebTransport and WebSocket message bus daemon")]
#[command(version)]
struct Args {
    /// Address to bind WebTransport to (UDP/QUIC)
    #[arg(short, long, default_value = "0.0.0.0:4433", env = "CATBUS_BIND")]
    bind: String,

    /// Address to bind WebSocket to (TCP/HTTP)
    #[arg(long, env = "CATBUS_WS_BIND")]
    ws_bind: Option<String>,

    /// Path to TLS certificate (PEM)
    #[arg(long, env = "CATBUS_CERT")]
    cert: String,

    /// Path to TLS private key (PEM)
    #[arg(long, env = "CATBUS_KEY")]
    key: String,

    /// Token signing secret
    #[arg(long, env = "CATBUS_SECRET")]
    secret: String,

    /// Database URL
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

    /// Admin key for full access
    #[arg(long, env = "CATBUS_ADMIN_KEY")]
    admin_key: Option<String>,

    /// Run as daemon (background)
    #[arg(short, long)]
    daemon: bool,

    /// PID file path (only with --daemon)
    #[arg(long, env = "CATBUS_PIDFILE")]
    pidfile: Option<PathBuf>,

    /// Log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Daemonize if requested
    if args.daemon {
        daemonize(&args)?;
    }

    run_server(args).await
}

fn daemonize(args: &Args) -> Result<()> {
    use std::fs::File;
    use std::io::Write;

    // Fork and detach
    match unsafe { libc::fork() } {
        -1 => return Err(anyhow::anyhow!("Fork failed")),
        0 => {
            // Child process - continue
        }
        pid => {
            // Parent process - write pidfile and exit
            if let Some(pidfile) = &args.pidfile {
                let mut f = File::create(pidfile)
                    .with_context(|| format!("Failed to create pidfile: {:?}", pidfile))?;
                writeln!(f, "{}", pid)?;
            }
            info!(pid = pid, "Daemon started");
            std::process::exit(0);
        }
    }

    // Create new session
    if unsafe { libc::setsid() } == -1 {
        return Err(anyhow::anyhow!("setsid failed"));
    }

    // Change to root directory
    std::env::set_current_dir("/")?;

    // Close standard file descriptors
    unsafe {
        libc::close(0);
        libc::close(1);
        libc::close(2);
    }

    Ok(())
}

async fn run_server(args: Args) -> Result<()> {
    // Parse database config
    let db_config =
        PostgresConfig::from_url(&args.database_url).context("Invalid DATABASE_URL")?;

    // Create store
    let store = Arc::new(PostgresStore::new(db_config).await?);

    // Read TLS files
    let cert = std::fs::read_to_string(&args.cert)
        .with_context(|| format!("Failed to read certificate: {}", args.cert))?;
    let key = std::fs::read_to_string(&args.key)
        .with_context(|| format!("Failed to read private key: {}", args.key))?;

    // Parse bind address
    let bind_addr = args.bind.parse().context("Invalid bind address")?;

    // Build config
    let admin_key = args.admin_key.map(AdminKey::new);
    let token_secret = args.secret.into_bytes();

    let config = CatbusServerConfig {
        bind_addr,
        cert_pem: cert,
        key_pem: key,
        token_secret: token_secret.clone(),
        admin_key: admin_key.clone(),
    };

    // Create WebTransport server
    let server = CatbusServer::new(config, store.clone());

    info!(addr = %args.bind, "Catbus daemon starting (WebTransport)");

    // Optionally start WebSocket server
    let ws_handle = if let Some(ws_bind) = &args.ws_bind {
        let ws_addr = ws_bind.parse().context("Invalid WebSocket bind address")?;

        // Create WebSocket state sharing the same connection manager and router
        let ws_state = WsState {
            connections: server.connections().clone(),
            router: server.router(),
            role_store: store.clone(),
            token_secret,
            admin_key,
        };

        info!(addr = %ws_bind, "Catbus daemon starting (WebSocket)");

        Some(tokio::spawn(async move {
            if let Err(e) = run_websocket_server(ws_addr, ws_state).await {
                warn!(error = %e, "WebSocket server error");
            }
        }))
    } else {
        None
    };

    // Run server with graceful shutdown on signals
    tokio::select! {
        result = server.run() => {
            result?;
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received, stopping server");
        }
    }

    // Abort WebSocket server if running
    if let Some(handle) = ws_handle {
        handle.abort();
    }

    // Cleanup pidfile if it exists
    if let Some(pidfile) = &args.pidfile {
        if pidfile.exists() {
            if let Err(e) = std::fs::remove_file(pidfile) {
                warn!(error = %e, "Failed to remove pidfile");
            }
        }
    }

    info!("Catbus daemon stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
