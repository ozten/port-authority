---
title: "feat: VM-to-host port negotiation"
type: feat
status: active
date: 2026-03-23
origin: port-broker-spec.md (Phase 2, Section 6 Option C, Section 9 VM CLI)
---

# VM-to-Host Port Negotiation

## Problem Statement

A VM needs to ask the host's portd whether a specific port is available and reserve it if so. Today this is impossible for two reasons:

1. **portd only listens on a Unix domain socket** — VMs cannot reach it over the network
2. **There is no "check availability" RPC** — the only option is `Reserve` with `exact_only=true`, which commits immediately rather than just checking

### User Story

> On the host I run `portd`. On a VM I want to ask portd if port 8321 is available to reserve. If it is, I reserve it. If it isn't, I ask again for 8322, etc.

This is a sequential probe-and-claim workflow driven from inside the VM.

### Origin in the Spec

From `port-broker-spec.md`:

- **Section 8 Phase 2:** "Add reverse SSH control. Optional VM agents."
- **Section 9 VM CLI:** "request ports, release ports, inspect"
- **Section 6 Option C:** "VM can initiate requests securely" (Reverse SSH control)
- **Section 14:** "Defer: VM daemons, reverse SSH (until needed)"

This sprint implements the VM CLI and the transport bridge that the spec deferred.

---

## Current Architecture Gaps

### Gap 1: No network listener

portd binds exclusively to a Unix domain socket (`~/.local/share/portd/portd.sock` or `$XDG_RUNTIME_DIR/portd.sock`). VMs communicate with the host over TCP/IP, so the socket is unreachable.

**Relevant code:** `crates/daemon/src/main.rs:104-112` — `UnixListener::bind()`

### Gap 2: No availability check RPC

The closest thing is `Reserve` with `exact_only=true`, but that has side effects:
- It creates a reservation row in SQLite
- It binds the port with a hold listener (`StdTcpListener::bind`)
- For VM owners (`vm:*`), it starts an SSH tunnel

There's no way to ask "is port X free?" without claiming it.

**Relevant code:** `crates/daemon/src/broker.rs:130-210` — `reserve()` method

### Gap 3: No scan/probe RPC

The user's workflow is "try 8321, if taken try 8322, etc." Doing this as N sequential Reserve+Release calls is wasteful. A single "find me the first available port in range X-Y" or "check these ports" RPC would be much more efficient.

**Relevant code:** `crates/daemon/src/broker.rs:440-501` — `allocate_port()` already has scan logic internally, but it's not exposed as a standalone query.

---

## Proposed Solution

### Transport: Reverse SSH Control (recommended, from spec Option C)

The spec's Option C describes **reverse SSH control** — the VM SSHes to the host and runs `portctl` commands remotely. This requires no new network listener on portd and preserves the existing UDS-only security model.

**From the spec (Section 6, Option C):**

> **Reverse SSH control** — VM can initiate requests securely

**How it works:**

1. The VM has SSH access to the host (it already must, since the host manages SSH tunnels *to* the VM)
2. `portctl` is installed on the host and accessible over SSH
3. The VM runs portctl commands on the host via `ssh host portctl ...`

**Example — the probe-and-claim workflow from the VM:**
```bash
#!/bin/bash
# Running inside the VM, executing portctl on the host via SSH
HOST_USER="ozten"
HOST_IP="192.168.64.1"

for port in $(seq 8321 8400); do
    result=$(ssh ${HOST_USER}@${HOST_IP} \
        "portctl reserve --owner vm:smith:web --target 127.0.0.1:8080 --port ${port} --exact --json" 2>&1)
    if echo "$result" | jq -e '.assigned_port' >/dev/null 2>&1; then
        echo "Reserved port $port"
        exit 0
    fi
done
echo "No ports available in range"
exit 1
```

**Advantages:**
- Zero changes to portd — it keeps its UDS-only listener
- Security model unchanged — SSH provides authentication and encryption
- Works today if `portctl` is on the host's PATH
- The VM already needs SSH connectivity to the host (that's how portd manages tunnels)
- No new attack surface (no TCP listener to secure)

**Disadvantages:**
- Each command forks an SSH connection (latency ~50-200ms per call)
- Not suitable for high-frequency polling
- Requires SSH key setup from VM→host (reverse direction from host→VM tunnels)

**When to use:** This is the right choice when the VM needs occasional port operations (reserve on startup, release on shutdown). It's the spec's recommended Phase 2 approach.

### Transport Alternative: TCP Listener for portd

If reverse SSH latency is unacceptable (e.g., tight probe loops), add an optional TCP listener alongside the existing Unix domain socket.

**Config:**
```toml
[daemon]
# Existing UDS (always on)
socket_path = "~/.local/share/portd/portd.sock"
# New: optional TCP listener for VM access
tcp_listen = "192.168.64.1:50051"  # bind to VM bridge interface only
```

**Implementation notes:**
- tonic supports serving on multiple listeners — use `Router::serve_with_incoming` with a `select!` over both UDS and TCP streams
- TCP listener should be opt-in (off by default) to preserve the current security model
- Bind to the VM bridge interface only (e.g., `192.168.64.1`) rather than `0.0.0.0`
- No auth initially (same trust model as UDS — the host network is trusted), but document the risk

**Relevant code to modify:**
- `crates/daemon/src/main.rs` — add TCP listener alongside UDS
- `crates/daemon/src/config.rs` — add `tcp_listen: Option<SocketAddr>`

**When to use:** If you need sub-millisecond RPC latency from the VM, or want a native gRPC client on the VM without SSH overhead.

**VM-side client for TCP approach:**

Add a `--host` flag to `portctl`:
```bash
# From inside the VM:
portctl --host 192.168.64.1:50051 reserve --owner vm:smith:web --target 127.0.0.1:8080 --port 8321 --exact
```

`portctl` already uses tonic — just add a CLI flag to switch between `unix:` and `http://` channel addresses.

**Relevant code:** `crates/cli/src/main.rs` — `connect()` function

---

## New RPCs

### Feature 1: CheckAvailability RPC

A lightweight, read-only RPC that checks whether a port is available without reserving it.

**Proto addition:**
```protobuf
rpc CheckAvailability(CheckAvailabilityRequest) returns (CheckAvailabilityResponse);

message CheckAvailabilityRequest {
    uint32 port = 1;
}

message CheckAvailabilityResponse {
    bool available = 1;
    // If not available, who holds it
    optional string held_by_owner = 2;
}
```

**Implementation:** Query the reservations table for a non-released reservation on that port, then call `can_bind()` to check OS-level availability. No side effects.

**Note:** This is inherently racy (TOCTOU) — port could be taken between Check and Reserve. But for the VM use case this is acceptable: the VM checks, then immediately reserves with `exact_only=true`. If the reserve fails (race), the VM moves to the next port.

### Feature 2: FindAvailablePort RPC (nice-to-have)

A single RPC that does the scan-and-claim in one shot, eliminating the need for the VM to loop.

**Proto addition:**
```protobuf
rpc FindAvailablePort(FindAvailablePortRequest) returns (FindAvailablePortResponse);

message FindAvailablePortRequest {
    string owner = 1;
    uint32 start_port = 2;          // Start of scan range
    optional uint32 end_port = 3;   // End of scan range (default: allocation range end)
    string target_host = 4;
    uint32 target_port = 5;
    optional uint32 lease_seconds = 6;
}

message FindAvailablePortResponse {
    string reservation_id = 1;
    uint32 assigned_port = 2;       // The first available port found and reserved
    ReservationState state = 3;
}
```

**Implementation:** This is essentially `Reserve` without `preferred_port` but with a custom scan start. The existing `allocate_port()` already scans upward — this just exposes that with a configurable start point.

**Alternative:** This might not be needed if `Reserve` is enhanced to accept a `start_port` field instead of `preferred_port`. The current `preferred_port` tries the exact port first, then scans upward. A `start_port` semantic would skip the exact-match attempt and go straight to scanning.

---

## Acceptance Criteria

### Must Have
- [ ] VM can reserve/release/inspect ports on the host's portd
- [ ] Transport works (either reverse SSH or TCP listener)
- [ ] Existing UDS functionality unchanged (no regression)

### Should Have
- [ ] `CheckAvailability` RPC — lightweight port availability query
- [ ] `portctl check --port <PORT>` CLI command
- [ ] Probe-and-claim workflow documented with example script

### Nice to Have
- [ ] `FindAvailablePort` RPC — scan-and-claim in one shot
- [ ] `portctl find --owner <OWNER> --start-port <PORT> --target <HOST:PORT>` CLI command
- [ ] TCP listener as alternative transport (if reverse SSH is insufficient)
- [ ] `portctl --host <addr>` flag for TCP mode

---

## Implementation Order

### Path A: Reverse SSH (minimal changes)
1. **Document the reverse SSH workflow** — write a VM-side script example
2. **CheckAvailability RPC** — add to proto + broker + grpc + CLI
3. **FindAvailablePort RPC** — optimization, can defer

### Path B: TCP listener (if SSH latency is a problem)
1. **TCP listener** — add to daemon config + main.rs
2. **`portctl --host` flag** — enables VM-side native gRPC
3. **CheckAvailability RPC** — same as above
4. **FindAvailablePort RPC** — same as above

---

## Dependencies & Risks

| Risk | Mitigation |
|---|---|
| Reverse SSH requires VM→host SSH keys | Document setup; may already exist for dev workflows |
| SSH fork overhead (~100ms per command) | Acceptable for occasional operations; use TCP listener if not |
| TCP listener exposes portd to network | Bind to VM bridge interface only; opt-in config; document security posture |
| CheckAvailability is racy (TOCTOU) | Acceptable; Reserve with exact_only is the atomic commit |
| Two listeners complicates shutdown | Use tokio::select! with shared shutdown signal |
| VM can't resolve host IP | Document expected network topology; use well-known bridge IP |

## Sources

- Original spec: `port-broker-spec.md` — Section 6 (Option C), Section 8 (Phase 2), Section 9 (VM CLI), Section 14 (Defer list)
- Existing UDS setup: `crates/daemon/src/main.rs:104-112`
- Port allocation scan: `crates/daemon/src/broker.rs:440-501`
- CLI connection: `crates/cli/src/main.rs`
- TCP listener for tonic: `tonic::transport::Server::serve()` accepts `TcpListener`
