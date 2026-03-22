# Host-Managed Port Brokerage for Local VM Development

## 1. Purpose

Design a development infrastructure system that allows multiple local VMs and the host machine to safely and predictably claim, reserve, and expose TCP ports for local development services.

The system should support:

- host-side reservation of ports for host-local services
- host-managed exposure of VM-local services onto host loopback ports
- collision avoidance across host and multiple VMs
- deterministic ownership and visibility of allocations
- a clean CLI for VM-side requests
- a long-running host daemon responsible for tunnel lifecycle
- optional future expansion to event-driven and bi-directional control

---

## 2. Goals

### Primary goals

- Provide a single source of truth for host port ownership.
- Allow VM services bound to `127.0.0.1` inside the VM to be exposed on the host.
- Avoid port collisions between:
  - host-local services
  - VM A forwarded services
  - VM B forwarded services
- Support preferred-port requests with fallback allocation.
- Keep the host daemon authoritative over reservations and forwarding lifecycle.
- Make the system usable from scripts and interactive developer workflows.

### Secondary goals

- Support automatic cleanup of stale reservations.
- Support optional long-running VM agents in the future.
- Allow future migration from manual CLI flow to event-driven orchestration.
- Keep security exposure low by default.

---

## 3. Non-goals

- Internet-facing service publication
- Service mesh functionality
- Container orchestration
- Full service discovery across a LAN
- TLS termination or HTTP routing as a first-class requirement
- Replacing SSH as a general remote admin transport

---

## 4. High-level model

The system is a **host-centric port broker**.

The host machine runs a long-lived daemon that:

- allocates host ports
- records reservations and ownership
- creates and supervises SSH tunnels for VM-backed services
- tracks health and lifecycle of active forwards
- exposes a control API to clients

---

## 5. Core concepts

### Reservation
A claim on a host port by an owner.

### Owner
Logical identity like `host:web` or `vm:smith:web`.

### Requested vs Assigned Port
- requested: preferred
- assigned: actual

### Target
Destination endpoint (host or VM).

### Lease
Optional time-bound reservation.

---

## 6. Architecture Options

### Option A: Host daemon only (Recommended)
- Host is authoritative
- No VM daemon required
- SSH tunnels managed by host

### Option B: Host + VM agents
- Event-driven
- More complex

### Option C: Reverse SSH control
- VM can initiate requests securely

### Option D: Full peer agents
- Most flexible
- Highest complexity

---

## 7. Transport

### Recommended
- SSH local forwarding

### Alternatives
- Direct VM IP (not recommended)
- Reverse SSH (future)

---

## 8. Recommended Path

### Phase 1
- Host daemon only
- SSH tunnels
- VM CLI
- Loopback bindings

### Phase 2
- Add reverse SSH control
- Optional VM agents

---

## 9. Components

### Host daemon
- allocation
- registry
- tunnel lifecycle

### VM CLI
- request ports
- release ports
- inspect

### Optional VM agent
- automation
- event-driven behavior

---

## 10. State Model

Each reservation includes:

- id
- owner
- requested port
- assigned port
- target
- state
- timestamps

States:
- pending
- active
- failed
- released

---

## 11. Allocation Policy

- exact match
- fallback to next available
- atomic operations required

---

## 12. Security

- bind to 127.0.0.1 by default
- use SSH for transport
- avoid exposing host APIs

---

## 13. Observability

- list reservations
- inspect state
- logs
- health status

---

## 14. Final Recommendation

Build:

- host daemon (authoritative)
- VM CLI
- SSH forwarding
- loopback-only bindings

Defer:

- VM daemons
- reverse SSH (until needed)

