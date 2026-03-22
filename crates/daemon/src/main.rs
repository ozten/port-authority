mod broker;
mod config;
mod db;
mod grpc;
mod lease;
mod tunnel;

use anyhow::Context;
use std::path::Path;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Mutex;
use tracing::{error, info};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let config = config::Config::load().context("failed to load config")?;

    // Initialize tracing
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.daemon.log_level));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!("portd starting");

    // Initialize database
    let pool = db::init_pool(&config.daemon.db_path).await?;

    // Load SSH config for VM tunneling
    let ssh_config = config::Config::load_ssh_config().context("failed to load SSH config")?;

    // Create tunnel manager
    let tunnel_manager = tunnel::TunnelManager::new(ssh_config, config.tunnel);
    let tunnel_manager = Arc::new(Mutex::new(tunnel_manager));

    // Create the broker
    let broker = broker::Broker::new(
        pool.clone(),
        config.allocation.port_range_start,
        config.allocation.port_range_end,
    );
    let broker = Arc::new(Mutex::new(broker));

    // Shutdown coordination
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Lease cleanup: expired reservations notify us so we can tear down tunnels
    let (expired_tx, mut expired_rx) = tokio::sync::mpsc::unbounded_channel();
    let lease_cleanup = lease::LeaseCleanup::new(pool, config.lease);
    let lease_handle = tokio::spawn(lease_cleanup.run(expired_tx, shutdown_rx.clone()));

    // Expired-reservation tunnel teardown task
    let tunnel_mgr_for_expired = Arc::clone(&tunnel_manager);
    let broker_for_expired = Arc::clone(&broker);
    let expired_handle = tokio::spawn(async move {
        while let Some(expired_list) = expired_rx.recv().await {
            for expired in expired_list {
                let mut tm = tunnel_mgr_for_expired.lock().await;
                tm.stop_tunnel(&expired.id);
                // Also free hold listener in broker
                let mut b = broker_for_expired.lock().await;
                b.release_hold_listener(expired.assigned_port);
            }
        }
    });

    // Health check background task
    let tunnel_mgr_for_health = Arc::clone(&tunnel_manager);
    let health_handle = tokio::spawn(tunnel::TunnelManager::run_health_checks(
        tunnel_mgr_for_health,
    ));

    // Ensure the socket parent directory exists with proper permissions
    let socket_path = &config.daemon.socket_path;
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket directory: {}", parent.display()))?;
    }

    // Remove stale socket file if it exists
    if socket_path.exists() {
        std::fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket: {}", socket_path.display()))?;
    }

    // Bind the Unix listener
    let uds = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind UDS at {}", socket_path.display()))?;

    // Set socket permissions (owner-only)
    set_socket_permissions(socket_path)?;

    info!(socket = %socket_path.display(), "listening on UDS");

    // Set up signal handlers for graceful shutdown
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    // Start the gRPC server
    let server = grpc::serve(uds, broker.clone(), tunnel_manager.clone());

    // Wait for either the server to finish or a shutdown signal
    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!(error = %e, "gRPC server error");
                return Err(e);
            }
        }
        _ = sigint.recv() => {
            info!("received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            info!("received SIGTERM, shutting down");
        }
    }

    // Signal shutdown to background tasks
    let _ = shutdown_tx.send(true);
    health_handle.abort();
    expired_handle.abort();
    let _ = lease_handle.await;

    // Cleanup: remove socket file
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }

    info!("portd stopped");
    Ok(())
}

#[cfg(unix)]
fn set_socket_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o660);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("failed to set socket permissions: {}", path.display()))?;
    Ok(())
}
