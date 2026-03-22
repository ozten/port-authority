# Port Authority

Host-managed port brokerage for local VM development. Reserve host ports, forward them into VMs over SSH, and avoid collisions — all from a single daemon.

## Install

```sh
cargo install --path crates/daemon
cargo install --path crates/cli
```

## Quick Start

Start the daemon:

```sh
portd
```

Reserve a host port:

```sh
portctl reserve --owner host:web --target 127.0.0.1:8080
```

Reserve a port forwarded into a VM:

```sh
portctl reserve --owner vm:smith:web --target 127.0.0.1:3000
```

List active reservations:

```sh
portctl list
```

Release a port:

```sh
portctl release --port 10000
```

## Architecture

```
┌──────────┐     UDS/gRPC      ┌──────────────┐     SSH tunnel     ┌──────┐
│  portctl │ ◄───────────────► │     portd    │ ◄────────────────► │  VM  │
└──────────┘                    │              │                     └──────┘
                                │  broker      │
                                │  tunnels     │
                                │  lease mgr   │
                                │  SQLite      │
                                └──────────────┘
```

**portd** is the host daemon — single source of truth for port ownership. It allocates ports, manages SSH tunnels, runs health checks, and cleans up expired leases.

**portctl** is the CLI client. It talks to portd over a Unix domain socket using gRPC.

## Configuration

Config files live in `~/.config/portd/`.

### portd.toml

```toml
[daemon]
socket_path = "/run/user/1000/portd.sock"
db_path = "~/.local/share/portd/portd.db"
log_level = "info"

[allocation]
port_range_start = 10000
port_range_end = 60000

[tunnel]
health_check_interval_secs = 30
max_reconnect_attempts = 5
ssh_keepalive_interval_secs = 30

[lease]
cleanup_interval_secs = 60
released_ttl_secs = 86400
```

### ssh.toml

Define VMs for SSH tunneling:

```toml
[vms.smith]
host = "192.168.64.2"
port = 22
user = "admin"
key = "~/.ssh/id_ed25519"

[vms.jones]
host = "192.168.64.3"
port = 22
user = "admin"
key = "~/.ssh/id_ed25519"
```

The VM name maps to the second segment of the owner string: `vm:smith:web` uses the `smith` VM config.

## CLI Reference

### portctl reserve

```
portctl reserve --owner <OWNER> --target <HOST:PORT> [--port <PORT>] [--exact] [--lease <SECS>]
```

- `--owner` — Identity like `host:web` or `vm:smith:api`
- `--target` — Destination endpoint in `host:port` format
- `--port` — Preferred host port (falls back to next available)
- `--exact` — Fail if preferred port is unavailable
- `--lease` — Auto-expire after N seconds (0 = indefinite)

### portctl release

```
portctl release --port <PORT>
portctl release --id <RESERVATION_ID>
```

### portctl list

```
portctl list [--owner <PREFIX>]
```

### portctl inspect

```
portctl inspect --port <PORT>
portctl inspect --id <RESERVATION_ID>
```

Shows reservation details and tunnel health (alive, uptime, reconnect count).

### portctl status

```
portctl status
```

### JSON output

Add `--json` to any command for machine-readable output:

```sh
portctl list --json | jq '.reservations[].assigned_port'
```

## How It Works

1. **Host reservations** (`host:*` owners) go directly to `active` — no tunnel needed, the port is just held.
2. **VM reservations** (`vm:*` owners) start as `pending`, then portd SSHs into the VM, sets up local port forwarding, and transitions to `active`. If SSH fails, the reservation moves to `failed`.
3. **Health checks** probe each tunnel every 30s via TCP connect. Dead tunnels are reconnected with exponential backoff (up to 5 attempts).
4. **Leases** auto-expire reservations. A background task runs every 60s to release expired reservations and purge old released records after 24h.
5. **Port allocation** uses bind-and-hold to prevent TOCTOU races. Preferred ports fall back to next-available scanning.

## Project Structure

```
crates/
  core/       Shared types, proto definitions, error types
  daemon/     portd — broker, tunnel manager, lease cleanup, gRPC server
  cli/        portctl — CLI client
proto/
  portd.proto gRPC service definition
migrations/
  001_initial.sql  SQLite schema
```

## License

MIT
