---
title: "feat: VM-to-host port negotiation"
type: feat
status: active
date: 2026-03-23
origin: docs/plans/NEXT_SPRINT.md
deepened: 2026-03-23
---

# VM-to-Host Port Negotiation

## Enhancement Summary

**Deepened on:** 2026-03-23
**Agents used:** Security Sentinel, Architecture Strategist, Performance Oracle, Best Practices Researcher, Framework Docs Researcher, Pattern Recognition Specialist, Code Simplicity Reviewer, Data Integrity Guardian

### Key Changes from Deepening

1. **CheckAvailability demoted from Must Have to deferred** — `start_port` on Reserve does atomic scan-and-claim, making a separate read-only check redundant for the user story
2. **`start_port` promoted to core deliverable** — eliminates O(N * 200ms) SSH probe loop; one call instead of N
3. **Security hardening added** — `ForceCommand` required for reverse SSH; owner verification on Release RPC; `held_by_owner` field removed from CheckAvailability (information leak)
4. **SSH ControlMaster documented** — 10-20x latency improvement with zero code changes
5. **Phases collapsed from 4 to 2** — the entire feature is ~50-80 LOC of changes
6. **Exit codes simplified to 0/1** — matches existing anyhow-based CLI pattern; use `--json` for structured error details
7. **TCP listener guidance corrected** — two `tokio::spawn` tasks (not merged streams); hybrid auth (UDS unauthenticated, TCP requires bearer token)
8. **`allocate_port()` query optimized** — range-scoped SQL instead of full-table HashSet load
9. **Pre-existing security issues flagged** — Release RPC has no owner check; `target_host` is unrestricted (SSRF vector)

### Conflicts Reconciled

| Topic | Simplicity Reviewer | Architecture Reviewer | Resolution |
|---|---|---|---|
| CheckAvailability | Cut entirely | Keep, but secondary to start_port | Defer — `portctl inspect --port` covers the "who holds this?" case |
| Exit codes | 0/1 only | 0/1/2 minimum | 0/1 + `--json` for details. Matches existing CLI pattern |
| Phases | 1 PR | 3 phases resequenced | 2 phases: core feature + security hardening |

---

## Overview

Enable VMs to reserve ports on the host's `portd` daemon via a single atomic call. Add a `start_port` field to `ReserveRequest` so the VM can say "give me the first available port >= 8321" in one SSH command, eliminating the need for a probe loop.

## Problem Statement

A VM user runs `portd` on their host. From inside a VM they want to ask portd for an available port starting at 8321, reserve it, and get back the assigned port. This is impossible today because:

1. **portd only listens on UDS** — `crates/daemon/src/main.rs:105-106` binds `UnixListener` exclusively
2. **No scan-from-offset** — `allocate_port()` at `broker.rs:437-496` has scan logic but isn't exposed with a custom start point

The transport problem (gap 1) is solved by reverse SSH — the VM runs `ssh host portctl reserve ...` with no daemon changes needed. The scan problem (gap 2) is solved by adding `start_port` to `ReserveRequest`.

## Proposed Solution

### Transport: Reverse SSH

The VM SSHes to the host and runs `portctl` commands remotely. No daemon changes.

```
VM ──SSH──> Host ──portctl──> portd (UDS)
                                │
                  portd ──SSH──> VM (tunnel setup)
```

**Why reverse SSH over TCP:**
- Zero daemon changes — portd keeps its UDS-only security model
- SSH provides authentication and encryption for free
- No new attack surface — no TCP listener to misconfigure
- `start_port` eliminates the probe loop, so SSH's per-call latency (~50-200ms) is acceptable: one call, not N

<details>
<summary>SSH circularity note</summary>

When a VM reserves a port via `ssh host portctl reserve --owner vm:smith:web`, portd SSHes *back* into the VM to establish the tunnel. This creates `VM→SSH→host→portd→SSH→VM`. It works but the original SSH session blocks until the return tunnel is established or fails (~1-3 seconds total). If the return SSH fails, the error is confusing — the user thinks SSH works (they're connected!) but the *reverse* direction fails.

</details>

### Research Insights: SSH Transport

**SSH ControlMaster (10-20x latency improvement, zero code changes):**

Configure on the VM's `~/.ssh/config`:
```
Host portd-host
    HostName 192.168.64.1
    User ozten
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 600
```

| Approach | Single call | 10-port probe (without start_port) |
|---|---|---|
| No ControlMaster | 50-200ms | 0.5-2s |
| With ControlMaster | 5-10ms | 50-100ms |
| `start_port` (atomic) | 50-200ms (one call) | N/A |

ControlMaster benefits all VM-to-host operations (list, inspect, release), not just reserve. Document this as a recommended optimization.

**Watch RPC over SSH:** `ssh host portctl watch` works — SSH keeps the connection open and streams stdout. But it's fragile: no reconnection semantics, no gRPC backpressure. If Watch-from-VM becomes a real need, that's the trigger to add the TCP listener.

### Core Feature: `start_port` on ReserveRequest

Extend `ReserveRequest` with a `start_port` field for atomic scan-and-claim:

```protobuf
message ReserveRequest {
  string owner = 1;
  optional uint32 preferred_port = 2;
  string target_host = 3;
  uint32 target_port = 4;
  optional uint32 lease_seconds = 5;
  bool exact_only = 6;
  optional uint32 start_port = 7;  // NEW: scan from this port upward
}
```

**Semantics:**
- `start_port` set, `preferred_port` not: scan upward from `start_port`, reserve the first available
- Both set: try `preferred_port` first, fall back to scanning from `start_port`
- Neither: existing behavior (scan from `port_range_start`)

**Why not a separate FindAvailablePort RPC:** It would duplicate 90% of Reserve's logic (owner validation, lease handling, idempotency check, hold-listener binding, tunnel startup for VM owners). Adding a field to the existing message is simpler and keeps the proto surface small. (See Architecture review)

**Why not CheckAvailability as a prerequisite:** The user story is "find and reserve an available port," not "check if a port is available." `start_port` does atomic scan-and-claim — no TOCTOU race, no probe loop, one SSH call. CheckAvailability is useful for debugging ("who holds port 8321?") but `portctl inspect --port 8321` already answers that question. Defer CheckAvailability until a concrete use case demands it. (See Simplicity review)

### Research Insights: `allocate_port()` Optimization

The current implementation loads ALL non-released ports into a `HashSet`, then scans linearly. With `start_port`, we can scope the query to only the scan range:

```rust
// Before: loads all reservations regardless of range
let rows = sqlx::query("SELECT assigned_port FROM reservations WHERE state != 'released'")

// After: only loads ports in the scan range
let rows = sqlx::query(
    "SELECT assigned_port FROM reservations \
     WHERE assigned_port >= ? AND assigned_port <= ? AND state != 'released' \
     ORDER BY assigned_port"
).bind(scan_start as i64).bind(end as i64)
```

The partial unique index `idx_assigned_port_active ON reservations(assigned_port) WHERE state != 'released'` supports this range scan efficiently. At <1000 reservations the difference is negligible, but the targeted query is cleaner regardless of scale. (See Performance Oracle, Data Integrity Guardian)

## Technical Approach

### Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ HOST                                                          │
│                                                               │
│  ┌─────────┐    UDS     ┌───────────┐    SQLite  ┌────────┐ │
│  │ portctl  │◄─────────►│   portd    │◄──────────►│ DB     │ │
│  │ (CLI)    │           │  (daemon)  │            └────────┘ │
│  └────▲─────┘           └─────┬──────┘                       │
│       │                       │ SSH                           │
│       │ SSH                   ▼                               │
│  ┌────┴─────────────────────────────────────────────┐        │
│  │ VM                                                │        │
│  │                                                   │        │
│  │  ssh portd-host portctl reserve \                 │        │
│  │    --owner vm:smith:web \                         │        │
│  │    --target 127.0.0.1:8080 \                     │        │
│  │    --start-port 8321 --json                       │        │
│  └──────────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────────┘
```

### Implementation Phases

#### Phase 1: `start_port` + Documentation (Core Feature)

**Deliverables:**
- `start_port` field on `ReserveRequest`
- `portctl reserve --start-port <PORT>` CLI flag
- Updated `allocate_port()` with custom scan start and range-scoped query
- VM example script and SSH setup documentation

**Files to modify:**

| File | Change | LOC |
|---|---|---|
| `proto/portd.proto` | Add `optional uint32 start_port = 7` to `ReserveRequest` | ~1 |
| `crates/daemon/src/broker.rs` | Modify `allocate_port()` signature to accept `start_port: Option<u16>`, use range-scoped query | ~20 |
| `crates/daemon/src/grpc.rs` | Pass `start_port` from request to broker | ~3 |
| `crates/cli/src/main.rs` | Add `--start-port` flag to `Reserve` subcommand | ~5 |
| `crates/daemon/src/broker.rs` (tests) | Add tests for start_port: scan from offset, wrap-around, exhausted range | ~40 |

**Total: ~70 LOC**

**Implementation detail — `allocate_port()` change:**

```rust
// broker.rs — modified signature
async fn allocate_port(
    &self,
    preferred: Option<u16>,
    exact_only: bool,
    start_port: Option<u16>,  // NEW
) -> Result<u16, PortError> {
    let (range_start, range_end) = self.port_range;
    let scan_start = start_port.unwrap_or(range_start);

    // If preferred port given, try it first (existing behavior unchanged)
    if let Some(pref) = preferred {
        // ... existing preferred port logic ...
    }

    // Range-scoped query: only load ports in [scan_start, range_end]
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT assigned_port FROM reservations \
         WHERE assigned_port >= ? AND assigned_port <= ? AND state != 'released' \
         ORDER BY assigned_port"
    )
    .bind(scan_start as i64)
    .bind(range_end as i64)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| PortError::Database(e.to_string()))?;

    let taken: HashSet<u16> = rows.iter().map(|r| r.0 as u16).collect();

    // Scan from start_port upward, then wrap around
    for port in scan_start..=range_end {
        if !taken.contains(&port) && Self::can_bind(port) {
            return Ok(port);
        }
    }
    // Wrap-around: scan from range_start to scan_start
    if scan_start > range_start {
        let wrap_rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT assigned_port FROM reservations \
             WHERE assigned_port >= ? AND assigned_port < ? AND state != 'released'"
        )
        .bind(range_start as i64)
        .bind(scan_start as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PortError::Database(e.to_string()))?;

        let wrap_taken: HashSet<u16> = wrap_rows.iter().map(|r| r.0 as u16).collect();
        for port in range_start..scan_start {
            if !wrap_taken.contains(&port) && Self::can_bind(port) {
                return Ok(port);
            }
        }
    }

    Err(PortError::PortNotAvailable)
}
```

**Concurrency safety:** The `Broker` is behind `Arc<Mutex<Broker>>`. `reserve()` takes `&mut self`, ensuring exclusive access during the entire allocate→bind→INSERT sequence. The single-writer SQLite pool (`max_connections=1`) provides an additional serialization layer. No concurrent caller can interleave between the query and the INSERT. (See Data Integrity Guardian)

**Pattern compliance:** This follows the existing `reserve()` → `allocate_port()` call chain at `broker.rs:171`. The only change is the `allocate_port()` signature. All existing callers pass `None` for `start_port`. (See Pattern Recognition Specialist)

**VM example script:**

```bash
#!/bin/bash
# Running inside the VM — single atomic call
HOST="ozten@portd-host"  # uses ControlMaster from ~/.ssh/config

result=$(ssh "$HOST" portctl reserve \
    --owner vm:smith:web \
    --target 127.0.0.1:8080 \
    --start-port 8321 \
    --json 2>/dev/null)

if [ $? -ne 0 ]; then
    echo "Error: could not reach portd" >&2
    exit 1
fi

state=$(echo "$result" | jq -r '.state')
port=$(echo "$result" | jq -r '.assigned_port')

case "$state" in
    active|pending)
        echo "Reserved port $port"
        ;;
    failed)
        # Tunnel setup failed — release and report
        id=$(echo "$result" | jq -r '.reservation_id')
        ssh "$HOST" portctl release --id "$id" 2>/dev/null
        echo "Reservation failed (tunnel could not be established)" >&2
        exit 1
        ;;
    *)
        echo "Unexpected state: $state" >&2
        exit 1
        ;;
esac
```

**Success criteria:**
- [ ] `portctl reserve --start-port 8321 --owner ... --target ...` finds and reserves first available port >= 8321
- [ ] Existing `--port` / `--exact` behavior unchanged (no regression)
- [ ] Tests: scan from start_port, wrap-around, exhausted range, start_port + preferred_port combo
- [ ] Example script documented and working from test VM
- [ ] SSH ControlMaster setup documented

#### Phase 2: Security Hardening (Pre-existing Issues)

These are pre-existing vulnerabilities that the reverse SSH transport amplifies. Address before broader VM adoption.

**Deliverables:**
- Owner verification on `release` RPCs
- `ForceCommand` / `command=` SSH restriction documentation (hard requirement)
- `target_host` validation against VM's known IP

**Files to modify:**

| File | Change |
|---|---|
| `crates/daemon/src/broker.rs` | Add `owner` parameter to `release_by_id()` and `release_by_port()`; verify caller owns the reservation |
| `crates/daemon/src/grpc.rs` | Pass owner from Release request to broker (add `owner` field to `ReleaseRequest`) |
| `proto/portd.proto` | Add `optional string owner = 3` to `ReleaseRequest` |
| `crates/daemon/src/grpc.rs` | Validate `target_host` in Reserve handler against VM's known IP from `ssh.toml` |

**Security findings from audit:**

| ID | Severity | Finding | Remediation |
|---|---|---|---|
| S-1 | Critical | Release RPC has no owner check — any caller can release any reservation (`grpc.rs:234-280`) | Add owner parameter; verify caller owns the reservation |
| S-2 | Critical | Reverse SSH gives VMs shell access to host unless restricted | Document `ForceCommand` in `authorized_keys` as a hard requirement, not optional |
| S-3 | High | `target_host` is unrestricted — SSRF vector (`grpc.rs:36-43` only checks length) | Validate against VM's known IP from `ssh.toml` |
| S-4 | Medium | Unbounded Watch stream connections — DoS vector | Add semaphore limit (defer to separate sprint) |

**ForceCommand setup (hard requirement for reverse SSH):**

```bash
# On the host's ~/.ssh/authorized_keys for the VM user:
command="/usr/local/bin/portctl $SSH_ORIGINAL_COMMAND",no-port-forwarding,no-X11-forwarding,no-agent-forwarding ssh-ed25519 AAAA... vm-smith-key
```

Or in `/etc/ssh/sshd_config`:
```
Match User portctl-vm
    ForceCommand /usr/local/bin/portctl $SSH_ORIGINAL_COMMAND
    AllowTcpForwarding no
```

This restricts the VM's SSH key to only run `portctl` commands — no shell access, no port forwarding. Without this, a compromised VM can read `~/.config/portd/ssh.toml` (which contains SSH keys for ALL other VMs) and pivot laterally. (See Security Sentinel)

**Success criteria:**
- [ ] `portctl release --port 8321` fails unless caller provides matching `--owner`
- [ ] `portctl reserve --target 10.0.0.5:22` fails if `10.0.0.5` is not the requesting VM's IP
- [ ] SSH `ForceCommand` setup documented with verification steps
- [ ] Existing single-user workflows unchanged (owner check is optional/skipped for UDS with `0o600`)

## Deferred Work

### CheckAvailability RPC

**Status:** Deferred — not needed for the user story. `start_port` does atomic scan-and-claim; `portctl inspect --port` answers "who holds this port?"

**If revisited later:**
- Use DB-only checks (no `can_bind()`) — the database is authoritative for portd reservations. `can_bind()` creates unnecessary port churn and a brief side-effect in what should be a read-only operation. (See Security Sentinel, Data Integrity Guardian)
- Do NOT include `held_by_owner` in the response — this leaks reservation identity to unauthenticated callers. The existing `Inspect` and `List` RPCs already expose this information for authorized users. (See Security Sentinel)
- Use `fetch_optional` (not `fetch_one` or `fetch_all`) — the partial unique index guarantees at most one non-released row per port, and the "no reservation" case returns `None`. This matches the existing `get_reservation_by_port()` pattern at `broker.rs:422-434`. (See Data Integrity Guardian)
- Follow the List/Inspect read-only RPC pattern — immutable lock (`let broker = self.broker.lock().await`), `&self` broker method. (See Pattern Recognition Specialist)

### TCP Listener

**Status:** Deferred — add only if Watch-from-VM or sub-ms latency becomes a real need.

**When implemented, key decisions (from research):**

| Topic | Decision | Rationale |
|---|---|---|
| Server topology | Two `tokio::spawn` tasks sharing cloned service, NOT merged streams | Allows different middleware per transport (auth on TCP, none on UDS). `PortBrokerServer` implements `Clone` since the inner service holds `Arc` references. (See Framework Docs, Best Practices) |
| Authentication | Bearer token via `tonic::service::Interceptor` on TCP server only | Hybrid: UDS stays frictionless (filesystem `0o600` permissions); TCP requires token in gRPC metadata. Simplest auth for a dev tool. (See Best Practices) |
| Shutdown | `CancellationToken` from `tokio_util::sync` | Cleaner than current `watch::channel` pattern. Cloneable, supports child tokens, integrates with `serve_with_incoming_shutdown`. (See Framework Docs) |
| Config | `tcp_listen: Option<SocketAddr>` in `DaemonConfig` with `#[serde(default)]` | Matches existing `Option<String>` pattern for optional fields. `None` by default (opt-in). (See Pattern Recognition) |
| HTTP/2 keepalive | `http2_keepalive_interval(30s)`, `http2_keepalive_timeout(10s)` | Detects dead VM connections in ~40s rather than TCP's default ~2 hours. Important for VM suspend/resume. (See Performance Oracle) |

**Example implementation pattern:**

```rust
// Two servers, same service, different middleware
let service = PortBrokerService::new(broker.clone(), tunnel_manager.clone());

// UDS: no auth
let uds_svc = PortBrokerServer::new(service.clone());
let uds_server = Server::builder()
    .add_service(uds_svc)
    .serve_with_incoming_shutdown(uds_stream, shutdown.clone().cancelled());

// TCP: bearer token auth
let tcp_svc = PortBrokerServer::with_interceptor(service, auth_check);
let tcp_server = Server::builder()
    .http2_keepalive_interval(Some(Duration::from_secs(30)))
    .http2_keepalive_timeout(Some(Duration::from_secs(10)))
    .add_service(tcp_svc)
    .serve_with_shutdown(tcp_addr, shutdown.clone().cancelled());

tokio::select! {
    r = tokio::spawn(uds_server) => r??,
    r = tokio::spawn(tcp_server) => r??,
    _ = signal::ctrl_c() => {}
}
shutdown.cancel();
```

### Structured Exit Codes

**Status:** Deferred — current anyhow-based CLI pattern (0 success, non-zero failure) is sufficient. Use `--json` for structured error details. If a concrete scripting need arises for distinguishing "port unavailable" from "daemon unreachable," add `process::exit()` in the specific subcommand's match arm only. (See Pattern Recognition, Simplicity Review)

### Read-Only SQLite Pool

**Status:** Deferred — future optimization. SQLite WAL mode supports concurrent readers. A second `max_connections=1` pool opened with `mode=ro` would let CheckAvailability (if added) execute without waiting for writer transactions. Not needed at current scale (<1000 reservations). (See Performance Oracle)

## System-Wide Impact

### Interaction Graph

- `portctl reserve --start-port` → existing gRPC `reserve()` handler → `broker.reserve()` → modified `allocate_port(start)` → same downstream as today (SQLite INSERT, hold listener, tunnel for VM owners)
- The `allocate_port()` change (custom start + range-scoped query) affects the scan entry point and query scope but not the allocation logic itself

### Error Propagation

- `Reserve` with `start_port`: same error paths as existing `Reserve`. `PortNotAvailable` means the entire range from `start_port` to `port_range_end` is exhausted (wrap-around included)
- Over reverse SSH: errors conveyed as exit code + stderr text (anyhow chain) or structured JSON via `--json` flag. The VM script parses JSON from stdout

### State Lifecycle Risks

- `start_port` on Reserve: no new state risk. Same transactional INSERT as existing Reserve
- **Orphaned reservations on VM crash**: existing risk, not introduced by this feature. Mitigated by lease expiry. Recommend `--lease 3600` in VM scripts
- **Post-daemon-restart zombie reservations**: existing limitation. DB retains reservations but hold listeners are in-memory only. `allocate_port()` correctly skips these ports (conservative). Consider a startup reconciliation step in a future sprint

## Acceptance Criteria

### Must Have

- [x] `optional uint32 start_port = 7` on `ReserveRequest` in `proto/portd.proto`
- [x] `allocate_port()` accepts `start_port: Option<u16>` and scans from that offset
- [x] `allocate_port()` uses range-scoped SQL query instead of full-table load
- [x] `portctl reserve --start-port <PORT>` CLI flag
- [x] Existing `--port` / `--exact` behavior unchanged (no regression)
- [x] Tests: scan from start_port, wrap-around, exhausted range, combo with preferred_port
- [ ] VM example script documented (using `start_port` for single-call reserve)
- [ ] SSH setup guide: ControlMaster config, `ForceCommand` restriction

### Should Have

- [ ] Owner verification on `release` RPCs (security hardening)
- [ ] `target_host` validation against VM's known IP from `ssh.toml`
- [ ] `ForceCommand` SSH restriction documented as hard requirement

### Nice to Have (Defer)

- [ ] CheckAvailability RPC (DB-only, no `held_by_owner`)
- [ ] TCP listener with bearer token auth
- [ ] `portctl --host <addr>` flag for TCP mode
- [ ] Structured exit codes (0/1/2)

## Dependencies & Prerequisites

| Dependency | Status | Notes |
|---|---|---|
| `tonic_build` proto compilation | Ready | `crates/core/build.rs` handles this |
| SQLite partial unique index | Merged | `migrations/20260323000000_partial_unique_index.sql` — covers range scan |
| VM SSH tunneling | Working | `crates/daemon/src/tunnel.rs` |
| `can_bind()` utility | Exists | `broker.rs:499-501` — used during allocation, not for read-only checks |

## Risk Analysis & Mitigation

| Risk | Severity | Mitigation |
|---|---|---|
| Reverse SSH requires VM→host SSH keys | Low | Document step-by-step setup with ControlMaster |
| VM SSH gives shell access without ForceCommand | **Critical** | Document as hard requirement; compromised VM + shell access → lateral movement via `ssh.toml` keys |
| Release RPC has no owner check (pre-existing) | **Critical** | Phase 2 adds owner verification |
| `target_host` unrestricted — SSRF (pre-existing) | High | Phase 2 validates against `ssh.toml` VM IPs |
| SSH PATH doesn't include `portctl` | Low | ForceCommand uses full path; document |
| `allocate_port()` refactor breaks existing behavior | Low | Existing test suite covers current behavior; add start_port tests |
| Orphaned reservations on VM crash | Low | Recommend `--lease 3600` in VM scripts; existing expiry handles cleanup |

## Sources & References

### Origin

- **Design document:** [docs/plans/NEXT_SPRINT.md](docs/plans/NEXT_SPRINT.md) — original spec-derived design exploring transport options and RPC additions

### Internal References

- Daemon UDS binding: `crates/daemon/src/main.rs:105-106`
- gRPC serve function: `crates/daemon/src/grpc.rs:482-506`
- Broker reserve: `crates/daemon/src/broker.rs:128-258`
- Port allocation scan: `crates/daemon/src/broker.rs:437-496`
- can_bind utility: `crates/daemon/src/broker.rs:499-501`
- Release (no owner check): `crates/daemon/src/grpc.rs:234-280`, `broker.rs:296-336`
- target_host validation (length only): `crates/daemon/src/grpc.rs:36-43`
- CLI connection: `crates/cli/src/main.rs:294-317`
- Config structure: `crates/daemon/src/config.rs:17-25`
- Proto definition: `proto/portd.proto:6-12`
- Tunnel manager: `crates/daemon/src/tunnel.rs:417-496`
- Socket permissions: `crates/daemon/src/main.rs:152-157`
- SSH config (key paths for all VMs): `crates/daemon/src/config.rs:69-81`

### Spec References

- `port-broker-spec.md` Section 6 Option C (Reverse SSH control)
- `port-broker-spec.md` Section 8 Phase 2 (VM agents)
- `port-broker-spec.md` Section 9 (VM CLI)
- `port-broker-spec.md` Section 14 (Defer list)

### External References

- [tonic dual-transport pattern (issue #1080)](https://github.com/hyperium/tonic/issues/1080)
- [tonic authentication example](https://github.com/hyperium/tonic/blob/master/examples/src/authentication/server.rs)
- [SSH ControlMaster multiplexing](https://en.wikibooks.org/wiki/OpenSSH/Cookbook/Multiplexing)
- [tokio_util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html)
