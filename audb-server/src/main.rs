mod connection;
mod daemon;
mod pool;
mod socket_server;

use anyhow::Result;
use clap::Parser;
use pool::ConnectionPool;
use std::sync::Arc;
use tracing::info;

#[derive(Parser)]
#[command(name = "audb-server")]
#[command(about = "Aurora Debug Bridge Server Daemon", long_about = None)]
struct Args {
    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Start server (daemon or foreground based on args)
    if args.foreground {
        // Initialize logging to stdout for foreground mode
        tracing_subscriber::fmt()
            .with_target(false)
            .with_thread_ids(false)
            .init();

        info!("Aurora Debug Bridge Server starting");
        info!("Running in foreground mode");

        // Start tokio runtime and run server
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_server())?;
    } else {
        // Daemonize FIRST, then start tokio runtime
        daemon::daemonize_and_run()?;
    }

    Ok(())
}

async fn run_server() -> Result<()> {
    // Create connection pool
    let pool = Arc::new(ConnectionPool::new());

    // Load devices from config and add to pool
    if let Ok(devices) = audb_core::features::config::device_store::DeviceStore::list_enabled() {
        for device in devices {
            pool.add_device(device).await;
        }
    }

    // Setup signal handlers
    let shutdown_signal = setup_signal_handlers()?;

    // Start Unix socket server with connection pool
    socket_server::start_server(pool, shutdown_signal).await?;

    info!("Server shutdown complete");
    Ok(())
}

fn setup_signal_handlers() -> Result<tokio::sync::mpsc::Receiver<()>> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);

    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt())
            .expect("Failed to register SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down gracefully");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT, shutting down gracefully");
            }
        }

        tx.send(()).await.ok();
    });

    Ok(rx)
}
