use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use port_authority_core::error::PortError;
use russh::client;
use russh::keys::key;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::{SshConfig, TunnelConfig, VmSshConfig};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Snapshot of health information for a single tunnel.
#[derive(Debug, Clone)]
pub struct TunnelHealthInfo {
    pub alive: bool,
    pub last_check: Option<Instant>,
    pub started_at: Instant,
    pub reconnect_count: u32,
}

/// Internal bookkeeping for one active tunnel.
struct TunnelEntry {
    /// Set to `true` to signal the forwarding task to shut down.
    shutdown: Arc<AtomicBool>,
    /// Join handle for the listener task.
    task: tokio::task::JoinHandle<()>,
    /// Shared health state updated by the health-check loop.
    health: Arc<Mutex<TunnelHealthInfo>>,
    /// Parameters needed to re-establish the tunnel on reconnect.
    params: TunnelParams,
}

#[derive(Clone)]
struct TunnelParams {
    vm_name: String,
    assigned_port: u16,
    target_host: String,
    target_port: u16,
}

// ---------------------------------------------------------------------------
// Minimal russh client handler -- accepts all host keys
// ---------------------------------------------------------------------------

struct SshHandler;

#[async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Local dev tunnelling -- always accept.
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// TunnelManager
// ---------------------------------------------------------------------------

pub struct TunnelManager {
    tunnels: HashMap<String, TunnelEntry>,
    ssh_config: SshConfig,
    tunnel_config: TunnelConfig,
}

impl TunnelManager {
    pub fn new(ssh_config: SshConfig, tunnel_config: TunnelConfig) -> Self {
        Self {
            tunnels: HashMap::new(),
            ssh_config,
            tunnel_config,
        }
    }

    /// Start a new SSH port-forwarding tunnel.
    ///
    /// Binds `127.0.0.1:{assigned_port}` on the host and forwards every
    /// incoming TCP connection through the SSH session to
    /// `{target_host}:{target_port}` inside the VM.
    pub async fn start_tunnel(
        &mut self,
        reservation_id: &str,
        vm_name: &str,
        assigned_port: u16,
        target_host: &str,
        target_port: u16,
    ) -> Result<(), PortError> {
        let vm_ssh = self
            .ssh_config
            .vms
            .get(vm_name)
            .ok_or_else(|| PortError::VmNotConfigured(vm_name.to_string()))?
            .clone();

        let ssh_handle = connect_and_authenticate(&vm_ssh, &self.tunnel_config).await?;
        let ssh_handle = Arc::new(ssh_handle);

        let shutdown = Arc::new(AtomicBool::new(false));
        let health = Arc::new(Mutex::new(TunnelHealthInfo {
            alive: true,
            last_check: None,
            started_at: Instant::now(),
            reconnect_count: 0,
        }));

        let task = spawn_forwarding_task(
            Arc::clone(&ssh_handle),
            assigned_port,
            target_host.to_string(),
            target_port,
            shutdown.clone(),
            health.clone(),
            self.tunnel_config.max_connections_per_tunnel,
        )
        .await?;

        let params = TunnelParams {
            vm_name: vm_name.to_string(),
            assigned_port,
            target_host: target_host.to_string(),
            target_port,
        };

        info!(
            reservation_id,
            vm_name, assigned_port, target_host, target_port, "tunnel started"
        );

        self.tunnels.insert(
            reservation_id.to_string(),
            TunnelEntry {
                shutdown,
                task,
                health,
                params,
            },
        );

        Ok(())
    }

    /// Stop and clean up the tunnel associated with `reservation_id`.
    pub fn stop_tunnel(&mut self, reservation_id: &str) {
        if let Some(entry) = self.tunnels.remove(reservation_id) {
            info!(reservation_id, "stopping tunnel");
            entry.shutdown.store(true, Ordering::SeqCst);
            entry.task.abort();
        } else {
            debug!(reservation_id, "stop_tunnel called but no active tunnel found");
        }
    }

    /// Return a snapshot of the current health info for a tunnel.
    pub async fn get_health(&self, reservation_id: &str) -> Option<TunnelHealthInfo> {
        let entry = self.tunnels.get(reservation_id)?;
        Some(entry.health.lock().await.clone())
    }

    /// Run periodic health checks for all active tunnels.
    ///
    /// This method never returns under normal operation. Spawn it as a
    /// background task via `tokio::spawn`.
    ///
    /// The manager is wrapped in an `Arc<Mutex<TunnelManager>>` so the
    /// background task can inspect and mutate state.
    pub async fn run_health_checks(manager: Arc<Mutex<TunnelManager>>) {
        // Read interval once; it won't change at runtime.
        let interval_secs = {
            let mgr = manager.lock().await;
            mgr.tunnel_config.health_check_interval_secs
        };
        let interval = Duration::from_secs(interval_secs);

        loop {
            tokio::time::sleep(interval).await;

            // Collect the set of reservation IDs and their ports + health handles
            // so we can release the manager lock while probing.
            let probes: Vec<(String, u16, Arc<Mutex<TunnelHealthInfo>>)> = {
                let mgr = manager.lock().await;
                mgr.tunnels
                    .iter()
                    .map(|(id, entry)| {
                        (id.clone(), entry.params.assigned_port, entry.health.clone())
                    })
                    .collect()
            };

            let timeout = Duration::from_secs(5);
            for (reservation_id, port, health) in &probes {
                let alive = tcp_health_probe(*port, timeout).await;

                let mut h = health.lock().await;
                h.alive = alive;
                h.last_check = Some(Instant::now());

                if !alive {
                    warn!(reservation_id = %reservation_id, port, "health check failed");
                }
            }

            // Attempt reconnection for dead tunnels.
            Self::attempt_reconnections(Arc::clone(&manager)).await;
        }
    }

    /// Try to reconnect any tunnels whose last health check reported them as
    /// dead, up to `max_reconnect_attempts`.
    async fn attempt_reconnections(manager: Arc<Mutex<TunnelManager>>) {
        // Gather candidates while holding the lock briefly.
        let candidates: Vec<(String, TunnelParams, Arc<Mutex<TunnelHealthInfo>>, u32)> = {
            let mgr = manager.lock().await;
            let max_attempts = mgr.tunnel_config.max_reconnect_attempts;
            let mut out = Vec::new();
            for (id, entry) in &mgr.tunnels {
                // Use try_lock to avoid blocking in async context.
                if let Ok(h) = entry.health.try_lock() {
                    if !h.alive && h.reconnect_count < max_attempts {
                        out.push((
                            id.clone(),
                            entry.params.clone(),
                            entry.health.clone(),
                            h.reconnect_count,
                        ));
                    }
                }
            }
            out
        };

        for (reservation_id, params, health, attempt) in candidates {
            let backoff = Duration::from_secs(1u64 << attempt.min(5));
            tokio::time::sleep(backoff).await;

            info!(
                reservation_id = %reservation_id,
                attempt = attempt + 1,
                "attempting tunnel reconnection"
            );

            // Read config under lock, then release before doing I/O.
            let (vm_ssh, max_conns, tunnel_config_clone) = {
                let mgr = manager.lock().await;
                let vm_ssh = match mgr.ssh_config.vms.get(&params.vm_name) {
                    Some(v) => v.clone(),
                    None => {
                        error!(vm = %params.vm_name, "VM gone from config during reconnect");
                        continue;
                    }
                };
                let max_conns = mgr.tunnel_config.max_connections_per_tunnel;
                let tc = TunnelConfig {
                    health_check_interval_secs: mgr.tunnel_config.health_check_interval_secs,
                    health_check_timeout_secs: mgr.tunnel_config.health_check_timeout_secs,
                    max_reconnect_attempts: mgr.tunnel_config.max_reconnect_attempts,
                    max_connections_per_tunnel: mgr.tunnel_config.max_connections_per_tunnel,
                    ssh_keepalive_interval_secs: mgr.tunnel_config.ssh_keepalive_interval_secs,
                };
                (vm_ssh, max_conns, tc)
            };

            let reconnect_result =
                match connect_and_authenticate(&vm_ssh, &tunnel_config_clone).await {
                    Ok(ssh_handle) => {
                        let ssh_handle = Arc::new(ssh_handle);
                        let shutdown = Arc::new(AtomicBool::new(false));
                        match spawn_forwarding_task(
                            ssh_handle,
                            params.assigned_port,
                            params.target_host.clone(),
                            params.target_port,
                            shutdown.clone(),
                            health.clone(),
                            max_conns,
                        )
                        .await
                        {
                            Ok(task) => Some((shutdown, task)),
                            Err(e) => {
                                error!(
                                    reservation_id = %reservation_id,
                                    error = %e,
                                    "failed to bind listener on reconnect"
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            reservation_id = %reservation_id,
                            error = %e,
                            "SSH reconnect failed"
                        );
                        None
                    }
                };

            let mut mgr = manager.lock().await;
            if let Some(entry) = mgr.tunnels.get_mut(&reservation_id) {
                let mut h = health.lock().await;
                h.reconnect_count += 1;

                if let Some((shutdown, task)) = reconnect_result {
                    // Tear down old task.
                    entry.shutdown.store(true, Ordering::SeqCst);
                    entry.task.abort();
                    entry.shutdown = shutdown;
                    entry.task = task;
                    h.alive = true;
                    info!(reservation_id = %reservation_id, "tunnel reconnected successfully");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SSH connect + authenticate helper
// ---------------------------------------------------------------------------

async fn connect_and_authenticate(
    vm_ssh: &VmSshConfig,
    tunnel_config: &TunnelConfig,
) -> Result<client::Handle<SshHandler>, PortError> {
    let key_pair = russh_keys::load_secret_key(&vm_ssh.key, None).map_err(|e| {
        PortError::SshConnectionFailed(
            vm_ssh.host.clone(),
            format!("failed to load SSH key {}: {e}", vm_ssh.key.display()),
        )
    })?;

    let mut config = client::Config::default();
    config.keepalive_interval = Some(Duration::from_secs(
        tunnel_config.ssh_keepalive_interval_secs,
    ));
    let config = Arc::new(config);

    let addr = (vm_ssh.host.as_str(), vm_ssh.port);
    let mut handle = client::connect(config, addr, SshHandler)
        .await
        .map_err(|e| {
            PortError::SshConnectionFailed(vm_ssh.host.clone(), format!("connect: {e}"))
        })?;

    let authed: bool = handle
        .authenticate_publickey(vm_ssh.user.clone(), Arc::new(key_pair))
        .await
        .map_err(|e| {
            PortError::SshConnectionFailed(vm_ssh.host.clone(), format!("auth: {e}"))
        })?;

    if !authed {
        return Err(PortError::SshConnectionFailed(
            vm_ssh.host.clone(),
            "public-key authentication rejected".to_string(),
        ));
    }

    info!(host = %vm_ssh.host, port = vm_ssh.port, user = %vm_ssh.user, "SSH session established");
    Ok(handle)
}

// ---------------------------------------------------------------------------
// Local forwarding task
// ---------------------------------------------------------------------------

/// Bind a TCP listener on 127.0.0.1:{port} and spawn a task that accepts
/// connections, forwarding each through the SSH session.
async fn spawn_forwarding_task(
    ssh_handle: Arc<client::Handle<SshHandler>>,
    local_port: u16,
    target_host: String,
    target_port: u16,
    shutdown: Arc<AtomicBool>,
    health: Arc<Mutex<TunnelHealthInfo>>,
    max_connections: usize,
) -> Result<tokio::task::JoinHandle<()>, PortError> {
    let listener = TcpListener::bind(("127.0.0.1", local_port))
        .await
        .map_err(|e| {
            PortError::SshConnectionFailed(
                "127.0.0.1".to_string(),
                format!("failed to bind port {local_port}: {e}"),
            )
        })?;

    info!(local_port, %target_host, target_port, "forwarding listener bound");

    let handle = tokio::spawn(async move {
        let active_connections = Arc::new(AtomicUsize::new(0));

        loop {
            if shutdown.load(Ordering::SeqCst) {
                debug!(local_port, "tunnel shutdown signal received");
                break;
            }

            let accept = tokio::select! {
                a = listener.accept() => a,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    // Periodically re-check shutdown flag.
                    continue;
                }
            };

            let (tcp_stream, peer) = match accept {
                Ok(v) => v,
                Err(e) => {
                    error!(local_port, error = %e, "accept failed");
                    let mut h = health.lock().await;
                    h.alive = false;
                    break;
                }
            };

            let current = active_connections.load(Ordering::SeqCst);
            if current >= max_connections {
                warn!(
                    local_port,
                    current, max_connections, "connection limit reached, dropping"
                );
                drop(tcp_stream);
                continue;
            }

            debug!(local_port, %peer, "accepted connection");

            let ssh = Arc::clone(&ssh_handle);
            let host = target_host.clone();
            let port = target_port;
            let conns = Arc::clone(&active_connections);
            let shutdown_flag = shutdown.clone();

            conns.fetch_add(1, Ordering::SeqCst);

            tokio::spawn(async move {
                if let Err(e) =
                    forward_one(tcp_stream, &ssh, &host, port, &shutdown_flag).await
                {
                    debug!(error = %e, "forwarding session ended");
                }
                conns.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });

    Ok(handle)
}

/// Forward a single TCP connection through the SSH channel.
async fn forward_one(
    mut tcp_stream: tokio::net::TcpStream,
    ssh_handle: &client::Handle<SshHandler>,
    target_host: &str,
    target_port: u16,
    shutdown: &AtomicBool,
) -> Result<(), anyhow::Error> {
    let channel = ssh_handle
        .channel_open_direct_tcpip(
            target_host,
            target_port as u32,
            "127.0.0.1",
            0_u32, // originator port -- not meaningful here
        )
        .await?;

    let mut stream = channel.into_stream();
    let (mut ssh_read, mut ssh_write) = tokio::io::split(&mut stream);
    let (mut tcp_read, mut tcp_write) = tcp_stream.split();

    // Bidirectional copy: TCP <-> SSH channel.
    let client_to_server = async {
        let mut buf = [0u8; 32 * 1024];
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let n = tcp_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            ssh_write.write_all(&buf[..n]).await?;
        }
        let _ = ssh_write.shutdown().await;
        Ok::<(), anyhow::Error>(())
    };

    let server_to_client = async {
        let mut buf = [0u8; 32 * 1024];
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let n = ssh_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tcp_write.write_all(&buf[..n]).await?;
        }
        let _ = tcp_write.shutdown().await;
        Ok::<(), anyhow::Error>(())
    };

    tokio::select! {
        r = client_to_server => { r?; }
        r = server_to_client => { r?; }
    }

    debug!("forwarding session complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Health probe helper
// ---------------------------------------------------------------------------

/// Try a TCP connect to 127.0.0.1:{port} with a timeout. Returns `true` if
/// the connection succeeds.
async fn tcp_health_probe(port: u16, timeout: Duration) -> bool {
    tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}
