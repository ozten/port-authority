mod output;

use anyhow::Context;
use clap::{Parser, Subcommand};
use hyper_util::rt::TokioIo;
use port_authority_core::proto::port_broker_client::PortBrokerClient;
use port_authority_core::proto::{
    InspectRequest, ListRequest, ReleaseRequest, ReserveRequest, inspect_request, release_request,
};
use std::path::PathBuf;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

#[derive(Parser)]
#[command(name = "portctl", about = "Port Authority CLI", version)]
struct Cli {
    /// Daemon socket path
    #[arg(short, long, default_value_t = default_socket_path())]
    socket: String,

    /// Output as JSON (for scripting)
    #[arg(short, long)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Reserve a port and start an SSH tunnel
    Reserve {
        /// Owner identity (e.g., vm:smith:web)
        #[arg(short, long)]
        owner: String,

        /// Target in host:port format (e.g., smith:8080)
        #[arg(short, long)]
        target: String,

        /// Preferred host port
        #[arg(short, long)]
        port: Option<u32>,

        /// Fail if preferred port is unavailable
        #[arg(long)]
        exact: bool,

        /// Lease duration in seconds (0 = indefinite)
        #[arg(short, long)]
        lease: Option<u32>,
    },

    /// Release a port reservation
    Release {
        /// Release by port number
        #[arg(short, long)]
        port: Option<u32>,

        /// Release by reservation ID
        #[arg(short, long)]
        id: Option<String>,
    },

    /// List active reservations
    List {
        /// Filter by owner prefix
        #[arg(short, long)]
        owner: Option<String>,
    },

    /// Show detailed info for a reservation
    Inspect {
        /// Inspect by port number
        #[arg(short, long)]
        port: Option<u32>,

        /// Inspect by reservation ID
        #[arg(short, long)]
        id: Option<String>,
    },

    /// Show daemon health and statistics
    Status,
}

fn default_socket_path() -> String {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{}/portd.sock", runtime_dir)
    } else if let Ok(home) = std::env::var("HOME") {
        format!("{}/.local/share/portd/portd.sock", home)
    } else {
        "/tmp/portd.sock".to_string()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let channel = connect_to_daemon(&cli.socket).await?;
    let mut client = PortBrokerClient::new(channel);

    match cli.command {
        Commands::Reserve {
            owner,
            target,
            port,
            exact,
            lease,
        } => {
            let (target_host, target_port) = parse_target(&target)?;
            let response = client
                .reserve(ReserveRequest {
                    owner,
                    preferred_port: port,
                    target_host,
                    target_port,
                    lease_seconds: lease,
                    exact_only: exact,
                })
                .await
                .context("reserve failed")?;

            let resp = response.into_inner();
            if cli.json {
                output::print_json(&resp)?;
            } else {
                println!(
                    "Reserved port {} (id: {}, state: {})",
                    resp.assigned_port,
                    resp.reservation_id,
                    output::state_name(resp.state),
                );
            }
        }

        Commands::Release { port, id } => {
            let identifier = match (port, id) {
                (Some(p), _) => Some(release_request::Identifier::Port(p)),
                (_, Some(id)) => Some(release_request::Identifier::ReservationId(id)),
                (None, None) => {
                    anyhow::bail!("specify --port or --id");
                }
            };
            let response = client
                .release(ReleaseRequest { identifier })
                .await
                .context("release failed")?;

            let resp = response.into_inner();
            if cli.json {
                output::print_json(&resp)?;
            } else if resp.success {
                println!("Released successfully");
            } else {
                println!("Release failed: {}", resp.message);
            }
        }

        Commands::List { owner } => {
            let response = client
                .list(ListRequest {
                    owner_filter: owner,
                    state_filter: None,
                })
                .await
                .context("list failed")?;

            let resp = response.into_inner();
            if cli.json {
                output::print_json(&resp)?;
            } else {
                output::print_reservation_table(&resp.reservations);
            }
        }

        Commands::Inspect { port, id } => {
            let identifier = match (port, id) {
                (Some(p), _) => Some(inspect_request::Identifier::Port(p)),
                (_, Some(id)) => Some(inspect_request::Identifier::ReservationId(id)),
                (None, None) => {
                    anyhow::bail!("specify --port or --id");
                }
            };
            let response = client
                .inspect(InspectRequest { identifier })
                .await
                .context("inspect failed")?;

            let resp = response.into_inner();
            if cli.json {
                output::print_json(&resp)?;
            } else {
                output::print_inspect(&resp);
            }
        }

        Commands::Status => {
            // Use list as a health check — if we can connect and get a response, daemon is up
            let response = client
                .list(ListRequest {
                    owner_filter: None,
                    state_filter: None,
                })
                .await
                .context("status check failed — is portd running?")?;

            let resp = response.into_inner();
            if cli.json {
                let status = serde_json::json!({
                    "status": "ok",
                    "active_reservations": resp.reservations.len(),
                });
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("portd is running");
                println!("Active reservations: {}", resp.reservations.len());
            }
        }
    }

    Ok(())
}

/// Parse a target string like "smith:8080" into (host, port).
fn parse_target(target: &str) -> anyhow::Result<(String, u32)> {
    let parts: Vec<&str> = target.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        anyhow::bail!(
            "invalid target format: expected host:port, got '{}'",
            target
        );
    }
    let port: u32 = parts[0]
        .parse()
        .with_context(|| format!("invalid port in target '{}'", target))?;
    let host = parts[1].to_string();
    Ok((host, port))
}

/// Connect to the daemon over a Unix domain socket.
async fn connect_to_daemon(socket_path: &str) -> anyhow::Result<Channel> {
    let socket_path = PathBuf::from(socket_path);

    // Check socket exists before trying to connect
    if !socket_path.exists() {
        anyhow::bail!(
            "daemon socket not found at {}. Is portd running?",
            socket_path.display()
        );
    }

    let channel = Endpoint::try_from("http://[::]:50051")? // URI is ignored for UDS
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = socket_path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .context("failed to connect to portd")?;

    Ok(channel)
}
