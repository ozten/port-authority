---
title: "feat: SvelteKit Port Management UI"
type: feat
status: active
date: 2026-03-23
---

# SvelteKit Port Management UI

## Enhancement Summary

**Deepened on:** 2026-03-23
**Research agents used:** architecture-strategist, security-sentinel, performance-oracle, kieran-typescript-reviewer, julik-frontend-races-reviewer, code-simplicity-reviewer, pattern-recognition-specialist, framework-docs-researcher, best-practices-researcher, frontend-design skill, Context7 (SvelteKit, nice-grpc, shadcn-svelte)

### Key Improvements
1. **Security hardening** — CSRF protection via `Origin` header validation + `SameSite=Strict` token; SSE connection limit (HIGH priority)
2. **Race condition mitigation** — Per-reservation UI state machine, merge-not-replace reconciliation with `updated_at` comparison, EventSource lifecycle management
3. **Type safety pipeline** — ts-proto `oneof=unions` + `useDate=false` flags, proto→view model mapper layer, separate SSE payload types for JSON round-trip
4. **Concrete implementation patterns** — nice-grpc singleton with `globalThis` + `Symbol.for`, SSE ReadableStream bridge, Svelte 5 `$state` class-based store

### New Considerations Discovered
- `UNAVAILABLE` gRPC status is overloaded (daemon down vs SSH failure) — needs disambiguation
- Owner validation in daemon doesn't enforce `host:`/`vm:` prefix — need `UnknownOwner` variant
- SSE events beat POST responses on UDS (microsecond latency) — requires client-side state machine
- `visibilitychange` fires on every tab switch — needs debounce with 5s hidden threshold
- Shared Watch fan-out + EventEmitter must live on `globalThis` to survive HMR

---

## Overview

Build a local SvelteKit web application in `./ui` that provides a visual dashboard for managing port-authority (portd) reservations. The UI runs on the host machine, connects to the portd gRPC daemon over its Unix domain socket via server-side API routes, and provides real-time updates through SSE bridging the Watch streaming RPC.

## Problem Statement / Motivation

The `portctl` CLI is effective for scripting and quick operations, but managing many port reservations across multiple VMs benefits from a visual dashboard with:
- At-a-glance status of all reservations and tunnel health
- Real-time updates without polling
- Easier filtering, grouping, and inspection
- A lower barrier to entry than memorizing CLI flags

## Proposed Solution

A SvelteKit 5 app using `adapter-node` for local deployment, with:
- **Server-side gRPC bridge**: `$lib/server/grpc.ts` connects to portd via `nice-grpc` + `@grpc/grpc-js` over `unix:$XDG_RUNTIME_DIR/portd.sock`
- **SSE streaming**: A `+server.ts` endpoint bridges the gRPC `Watch()` stream to the browser via Server-Sent Events
- **shadcn-svelte + Tailwind CSS v4**: Developer-tool aesthetic with dark mode
- **TypeScript throughout**: Proto-generated types via `ts-proto`

## Technology Stack

| Layer | Choice | Rationale |
|---|---|---|
| Framework | SvelteKit (Svelte 5, runes) | Modern, fast, file-based routing, native SSE |
| gRPC client | `nice-grpc` (wraps `@grpc/grpc-js`) | TypeScript-first, async iterables for streams |
| Proto codegen | `ts-proto` | Full type safety, nice-grpc compatible |
| Real-time | SSE via `+server.ts` ReadableStream | Native SvelteKit, matches unidirectional Watch RPC |
| UI components | shadcn-svelte + Tailwind CSS v4 | Data tables, dark mode, code ownership |
| Deployment | `adapter-node` | Local server, graceful shutdown |
| State | Svelte 5 `$state` runes | Modern, reactive, no boilerplate |

### Research Insights: Technology Choices

**ts-proto flags (verified via Context7 + best-practices research):**
```bash
protoc \
  --plugin=./node_modules/.bin/protoc-gen-ts_proto \
  --ts_proto_out=./src/lib/server/generated \
  --ts_proto_opt=outputServices=nice-grpc \
  --ts_proto_opt=outputServices=generic-definitions \
  --ts_proto_opt=oneof=unions \
  --ts_proto_opt=useDate=false \
  --ts_proto_opt=useOptionals=messages \
  --ts_proto_opt=esModuleInterop=true \
  --ts_proto_opt=env=node \
  --ts_proto_opt=forceLong=long \
  --ts_proto_opt=importSuffix=.js \
  --proto_path=../proto \
  ../proto/portd.proto
```

- `outputServices` must be specified **twice** (nice-grpc + generic-definitions) or `createClient` won't work
- `oneof=unions` is critical: `ReleaseRequest.identifier` and `InspectRequest.identifier` become proper discriminated unions with `$case` discriminator
- `useDate=false` preferred over `true` — `Date` has only millisecond precision, proto `Timestamp` has nanosecond; use `{ seconds, nanos }` and convert manually
- `useOptionals=messages` gives ergonomic `?` on nested messages while keeping scalar fields explicit

**nice-grpc UDS connection (verified via Context7):**
```typescript
// Correct format: unix: followed by triple-slash + absolute path
const channel = createChannel('unix:///run/user/1000/portd.sock');
// Also valid: unix: with single-slash path
const channel2 = createChannel('unix:/run/user/1000/portd.sock');
// WRONG: unix:// (double slash without third)
```

**shadcn-svelte dark mode (verified via Context7):**
- Uses `mode-watcher` library with `<ModeWatcher />` in root layout
- Tailwind v4 requires `@custom-variant dark (&:is(.dark *));` in `app.css`
- Data table component uses TanStack Table with `createSvelteTable` and `FlexRender`

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Browser                           │
│                                                      │
│  ┌──────────────┐  ┌───────────┐  ┌──────────────┐ │
│  │  Dashboard    │  │  Reserve  │  │   Inspect    │ │
│  │  +page.svelte │  │   Form    │  │   Detail     │ │
│  └──────┬───────┘  └─────┬─────┘  └──────┬───────┘ │
│         │                │               │          │
│  ┌──────▼────────────────▼───────────────▼───────┐  │
│  │     ReservationStore (class with $state)      │  │
│  │     Map<id, ReservationViewModel>             │  │
│  └──────┬────────────────┬───────────────┬───────┘  │
│         │ EventSource    │ fetch POST    │ fetch GET │
└─────────┼────────────────┼───────────────┼──────────┘
          │                │               │
┌─────────▼────────────────▼───────────────▼──────────┐
│              SvelteKit Server                        │
│                                                      │
│  /api/watch/+server.ts   (SSE → Watch RPC)          │
│  /api/reservations/+server.ts (List, Reserve, Release│
│  /api/reservations/[id]/+server.ts (Inspect)        │
│                                                      │
│  hooks.server.ts  ← Origin validation, CSRF         │
│  $lib/server/grpc.ts  ← nice-grpc singleton         │
│  $lib/server/watch-bridge.ts ← fan-out on globalThis│
│  $lib/server/generated/  ← ts-proto types           │
│  $lib/mappers/reservation.ts ← proto→view model     │
└──────────────────────┬──────────────────────────────┘
                       │ gRPC over UDS
                       │ unix:///run/user/1000/portd.sock
                       ▼
              ┌────────────────┐
              │     portd      │
              │  (Rust daemon) │
              └────────────────┘
```

### Research Insights: Architecture

**Security (from security-sentinel — HIGH priority):**
- Localhost TCP binding is NOT equivalent to Unix socket permissions. Any website open in the browser can issue `fetch("http://localhost:3000/api/reservations")`. This is the "localhost as a security boundary" anti-pattern.
- **Required mitigation:** `Origin` header validation in `hooks.server.ts` — reject requests where `Origin` is not `http://localhost:*` or `http://127.0.0.1:*`. This blocks cross-origin attacks from malicious websites.
- SvelteKit's built-in CSRF protection only covers form submissions (`application/x-www-form-urlencoded`), not JSON API endpoints. Add explicit CSRF check for JSON mutations.

**SSE fan-out lifecycle (from races reviewer + architecture reviewer):**
- The fan-out mechanism (EventEmitter or `Set<WritableStreamDefaultWriter>`) must live on `globalThis` alongside the gRPC client, not at module scope. Otherwise HMR in dev will orphan old listeners.
- When the gRPC Watch stream dies, the fan-out must broadcast a `type: reconnect` control message to all SSE clients.
- Limit concurrent SSE connections to 10 (server-side counter) to prevent resource exhaustion.

## Project Structure

```
ui/
├── package.json
├── svelte.config.js
├── vite.config.ts
├── tailwind.config.ts
├── tsconfig.json
├── proto-gen.sh                    # Script to generate TS from portd.proto
├── static/
│   └── favicon.svg
├── src/
│   ├── app.html
│   ├── app.css                     # Tailwind directives + dark mode variant
│   ├── hooks.server.ts             # Origin validation, CSRF, SSE connection limit
│   ├── routes/
│   │   ├── +layout.svelte          # Shell: nav, daemon status, dark mode toggle
│   │   ├── +layout.server.ts       # Check daemon connectivity on load
│   │   ├── +page.svelte            # Dashboard (main view)
│   │   ├── +page.server.ts         # Load: List() all reservations
│   │   ├── reserve/
│   │   │   └── +page.svelte        # Reserve form (full page)
│   │   ├── reservations/
│   │   │   └── [id]/
│   │   │       ├── +page.svelte    # Inspect detail view
│   │   │       └── +page.server.ts # Load: Inspect() reservation
│   │   └── api/
│   │       ├── watch/
│   │       │   └── +server.ts      # SSE endpoint bridging Watch RPC
│   │       ├── reservations/
│   │       │   └── +server.ts      # POST=Reserve, DELETE=Release
│   │       └── inspect/
│   │           └── [id]/
│   │               └── +server.ts  # GET=Inspect (for polling health)
│   └── lib/
│       ├── server/
│       │   ├── grpc.ts             # gRPC client singleton (nice-grpc)
│       │   ├── watch-bridge.ts     # Shared Watch stream fan-out (globalThis)
│       │   └── generated/          # ts-proto output from portd.proto
│       │       └── portd.ts
│       ├── mappers/
│       │   └── reservation.ts      # Proto types → view model types
│       ├── components/
│       │   ├── ui/                 # shadcn-svelte components
│       │   ├── ReservationTable.svelte
│       │   ├── StatusBadge.svelte
│       │   ├── TunnelHealthIndicator.svelte
│       │   ├── ReserveForm.svelte
│       │   ├── ReleaseDialog.svelte
│       │   ├── LeaseCountdown.svelte
│       │   ├── DaemonStatus.svelte
│       │   └── EmptyState.svelte
│       ├── stores/
│       │   └── reservations.svelte.ts  # Svelte 5 $state class-based store
│       ├── types/
│       │   └── index.ts            # App-level type aliases + view models
│       └── utils/
│           ├── format.ts           # Port display, timestamps, durations
│           ├── owner.ts            # Owner string parsing/validation
│           ├── errors.ts           # gRPC error → user message mapping
│           └── sse.ts              # EventSource wrapper with lifecycle mgmt
```

### Research Insights: Project Structure

**Type flow pipeline (from TypeScript reviewer):**
```
portd.proto
    |  (ts-proto codegen)
    v
$lib/server/generated/portd.ts  — generated types, never edited
    |  (nice-grpc client in $lib/server/grpc.ts)
    v
Server functions return proto types
    |  ($lib/mappers/reservation.ts)
    v
ReservationViewModel  — what components see
    |  (page data or store)
    v
Svelte components — never import from $lib/server/generated/
```

**Owner type with `UnknownOwner` fallback (from TypeScript reviewer):**
```typescript
interface HostOwner { readonly type: "host"; readonly service: string; }
interface VmOwner { readonly type: "vm"; readonly vm: string; readonly service: string; }
interface UnknownOwner { readonly type: "unknown"; readonly raw: string; }
type ParsedOwner = HostOwner | VmOwner | UnknownOwner;
```
The daemon's `validate_owner` does NOT enforce the `host:`/`vm:` prefix structure — it only validates character set. Owners like `"foo"` or `"host:"` are valid. The `UnknownOwner` fallback prevents SSR crashes.

**SSE payload types are NOT the same as proto types (from TypeScript reviewer):**
After `JSON.stringify` → `JSON.parse`, `Date` becomes `string`. Define a separate `SSEReservationEvent` interface for client-side consumption:
```typescript
interface SSEReservationEvent {
  reservationId: string;
  oldState: number;
  newState: number;
  timestamp?: string; // ISO string after JSON round-trip, NOT Date
  message?: string;
}
```

## Features & Acceptance Criteria

### Phase 1: Foundation (Server-Side Bridge + Dashboard)

**1.1 — Proto Codegen Pipeline**
- [ ] `proto-gen.sh` runs `protoc` with `ts-proto` plugin against `../proto/portd.proto`
- [ ] Flags: `outputServices=nice-grpc`, `outputServices=generic-definitions`, `oneof=unions`, `useDate=false`, `useOptionals=messages`
- [ ] Generated TypeScript in `src/lib/server/generated/portd.ts`
- [ ] npm script: `"proto:gen": "bash proto-gen.sh"`
- [ ] Generated code is committed (not gitignored) — it IS the type contract

**1.2 — gRPC Client Singleton**
- [ ] `$lib/server/grpc.ts` creates a nice-grpc channel + client using `globalThis` + `Symbol.for('portd.grpc')` pattern
- [ ] Target: `unix:${process.env.XDG_RUNTIME_DIR || '/run/user/1000'}/portd.sock`
- [ ] Singleton survives HMR in dev
- [ ] Exports typed functions: `listReservations()`, `reserve()`, `release()`, `inspect()`, `watchEvents()`
- [ ] Return types explicit on every function (no inference from client calls — `any` firewall)
- [ ] Graceful error handling when daemon is unreachable (socket missing / connection refused)
- [ ] Shutdown hook: `process.on('sveltekit:shutdown', () => channel.close())`

### Research Insights: gRPC Singleton Pattern

```typescript
// $lib/server/grpc.ts
import { createChannel, createClient, type Channel } from 'nice-grpc';
import { PortBrokerDefinition } from './generated/portd.js';

const GRPC_KEY = Symbol.for('portd.grpc');

interface GrpcSingleton {
  channel: Channel;
  client: ReturnType<typeof createClient<typeof PortBrokerDefinition>>;
}

function getGrpc(): GrpcSingleton {
  const existing = (globalThis as any)[GRPC_KEY] as GrpcSingleton | undefined;
  if (existing) return existing;

  const socketPath = process.env.XDG_RUNTIME_DIR
    ? `unix:${process.env.XDG_RUNTIME_DIR}/portd.sock`
    : 'unix:///run/user/1000/portd.sock';

  const channel = createChannel(socketPath);
  const singleton: GrpcSingleton = {
    channel,
    client: createClient(PortBrokerDefinition, channel),
  };
  (globalThis as any)[GRPC_KEY] = singleton;
  return singleton;
}

export const grpc = getGrpc();
```

**1.3 — Dashboard Page**
- [ ] `+page.server.ts` calls `List()` RPC, maps through `$lib/mappers/reservation.ts`, returns `ReservationViewModel[]`
- [ ] `+page.svelte` renders a data table (shadcn-svelte Table + TanStack Table) with columns:
  - Assigned Port (prominent, monospace)
  - Owner (parsed: type badge + name)
  - Target (`host:port`)
  - State (badge with icon + text + color)
  - Lease (countdown or "indefinite")
  - Created (relative time)
  - Actions (inspect, release)
- [ ] Empty state with message and link to Reserve form
- [ ] Loading skeleton while data loads

**1.4 — Application Shell + Security**
- [ ] Root `+layout.svelte` with:
  - Navigation: Dashboard, Reserve
  - Daemon status indicator (green dot = connected, red = disconnected)
  - Dark mode toggle (defaults to dark, `<ModeWatcher defaultMode="dark" />`)
  - App title: "Port Authority"
- [ ] `+layout.server.ts` checks daemon connectivity on initial load
- [ ] `hooks.server.ts`: validate `Origin` header on all requests — reject if not `localhost` or `127.0.0.1`
- [ ] Responsive layout (works on standard monitor widths, not mobile-optimized)

### Phase 2: Mutations (Reserve + Release)

**2.1 — Reserve Form**
- [ ] Structured owner input:
  - Radio/toggle: Host / VM
  - If VM: text field for VM name
  - Text field for service name
  - Assembled into `host:<service>` or `vm:<vmname>:<service>`
- [ ] Target fields: host (default `127.0.0.1`), port (required)
- [ ] Optional: preferred port, exact only toggle
- [ ] Optional: lease duration (toggle indefinite vs seconds input)
- [ ] Client-side validation mirroring daemon rules:
  - Owner: `^[a-zA-Z0-9:\-_.]{1,128}$`
  - Target host: 1-253 chars, restricted to `[a-zA-Z0-9.-]` or valid IP
  - Ports: 1-65535
- [ ] Server-side validation in `+server.ts` before forwarding to daemon (defense-in-depth)
- [ ] Submit via `POST /api/reservations`
- [ ] Success: redirect to dashboard with toast showing assigned port
- [ ] Error handling with user-friendly messages (see error mapping table)
- [ ] Idempotent reserve: if same reservation returned, show "Reservation already exists" with link

**2.2 — Release Action**
- [ ] "Release" button on each reservation row and in detail view
- [ ] Confirmation dialog for VM reservations ("This will tear down the SSH tunnel. Active connections will drop.")
- [ ] Host reservations: release without confirmation (less disruptive)
- [ ] Submit via `DELETE /api/reservations` with `{ reservation_id }` body
- [ ] Per-reservation UI state machine (see Race Condition Mitigation below)
- [ ] Idempotent: releasing an already-released reservation shows success

### Research Insights: Race Condition Mitigation (from races reviewer — HIGH priority)

On a Unix domain socket, the gRPC call and broadcast happen in microseconds. **SSE events will frequently arrive before the HTTP POST/DELETE response.** This requires a client-side state machine per reservation:

```typescript
// UI-only transient states (not proto states)
const UI_IDLE = Symbol("idle");
const UI_RELEASING = Symbol("releasing");
type UIState = typeof UI_IDLE | typeof UI_RELEASING;
```

**Rules:**
1. POST response handler checks: "Am I still in `UI_RELEASING`? If SSE already moved me to `released`, do nothing."
2. SSE handler checks: "Am I in `UI_RELEASING`? Move to `released` and suppress duplicate toast."
3. POST error handler: if SSE has already reconciled, only show error toast — don't touch state.

**Reconciliation must be merge-not-replace:**
```typescript
function mergeIntoStore(serverReservations: ReservationViewModel[]) {
  const byId = new Map(serverReservations.map(r => [r.id, r]));
  for (const [id, serverR] of byId) {
    const local = store.get(id);
    // Only overwrite if server data is newer
    if (!local || serverR.updatedAt >= local.updatedAt) {
      store.set(id, serverR);
    }
  }
}
```
This prevents a stale `List()` response (initiated before a release) from overwriting a fresh SSE-delivered state.

**2.3 — Failed Reservation Recovery**
- [ ] "Release & Retry" button on failed VM reservations
- [ ] Releases the failed reservation, then creates a new one with the same parameters
- [ ] Single-click convenience action

### Phase 3: Real-Time Updates (SSE + Watch Bridge)

**3.1 — SSE Endpoint (`/api/watch/+server.ts`)**
- [ ] `GET` handler returns `ReadableStream` with `text/event-stream` headers
- [ ] Server-side: one shared gRPC `Watch()` stream per process, fan-out to SSE clients
- [ ] Fan-out mechanism (`Set<WritableStreamDefaultWriter>` or `EventEmitter`) lives on `globalThis` to survive HMR
- [ ] Each SSE event is JSON-encoded `ReservationEvent` with string state names (not proto enum integers)
- [ ] On client disconnect (AbortSignal), clean up the fan-out subscription
- [ ] Handle gRPC Watch lag: send `event: refresh\ndata: {}\n\n` telling client to do full `List()`
- [ ] Send periodic SSE comment (`: keepalive\n\n`) every 15s to prevent timeout
- [ ] Limit to 10 concurrent SSE connections (server-side counter in `hooks.server.ts`)

### Research Insights: SSE Endpoint Pattern (from SvelteKit docs + framework research)

```typescript
// src/routes/api/watch/+server.ts
import type { RequestHandler } from './$types';
import { getWatchBridge } from '$lib/server/watch-bridge';

export const GET: RequestHandler = async ({ request }) => {
  const bridge = getWatchBridge();
  const encoder = new TextEncoder();

  const stream = new ReadableStream({
    start(controller) {
      const send = (data: unknown) => {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(data)}\n\n`));
      };

      const sendRefresh = () => {
        controller.enqueue(encoder.encode(`event: refresh\ndata: {}\n\n`));
      };

      const keepalive = setInterval(() => {
        controller.enqueue(encoder.encode(`: keepalive\n\n`));
      }, 15_000);

      const unsubscribe = bridge.subscribe(send, sendRefresh);

      // Cleanup on client disconnect
      request.signal.addEventListener('abort', () => {
        clearInterval(keepalive);
        unsubscribe();
        controller.close();
      });
    },
  });

  return new Response(stream, {
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      'Connection': 'keep-alive',
    },
  });
};
```

**3.2 — Client-Side SSE Consumer**
- [ ] Custom `EventSource` wrapper in `$lib/utils/sse.ts` that:
  - Always `close()` before creating new instance (prevent duplicate connections)
  - Handles native auto-reconnect by explicitly managing lifecycle
  - Debounces `visibilitychange` — only reconcile if hidden for >5 seconds
- [ ] Events update the reactive store via `store.applyEvent()` (merge, not replace)
- [ ] On SSE error/reconnect: do a full `List()` fetch to reconcile stale state
- [ ] Use in-flight guard on reconciliation (prevent overlapping `List()` calls)
- [ ] Toast notifications for state transitions
- [ ] On `visibilitychange` to visible: check `needsReconciliation` flag, only reconnect if hidden >5s

### Research Insights: EventSource Lifecycle (from races reviewer)

```typescript
// $lib/utils/sse.ts
let eventSource: EventSource | null = null;
let reconciliationInFlight = false;
let needsReconciliation = false;
let visibilityTimeout: ReturnType<typeof setTimeout> | null = null;

export function connectSSE(onEvent: (e: SSEReservationEvent) => void) {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
  eventSource = new EventSource('/api/watch');
  eventSource.onmessage = (e) => onEvent(JSON.parse(e.data));
  eventSource.addEventListener('refresh', () => reconcile());
  eventSource.onerror = () => {
    eventSource?.close();
    eventSource = null;
    reconcile();
    setTimeout(() => connectSSE(onEvent), 3000); // reconnect with backoff
  };
}

async function reconcile() {
  if (reconciliationInFlight) return;
  reconciliationInFlight = true;
  try {
    const fresh = await fetch('/api/reservations').then(r => r.json());
    store.mergeMany(fresh); // merge, not replace
  } finally {
    reconciliationInFlight = false;
  }
}

// Debounced visibility handler
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') {
    if (visibilityTimeout) {
      clearTimeout(visibilityTimeout);
      visibilityTimeout = null;
    } else if (needsReconciliation) {
      needsReconciliation = false;
      connectSSE(handler);
      reconcile();
    }
  } else {
    visibilityTimeout = setTimeout(() => {
      needsReconciliation = true;
      visibilityTimeout = null;
    }, 5000);
  }
});
```

**3.3 — Periodic Reconciliation (Safety Net)**
- [ ] Every 30 seconds (if SSE connected), call reconcile function (guarded against in-flight overlap)
- [ ] Prevents silent stale state from missed events (256-slot broadcast buffer in daemon)

### Phase 4: Inspection & Health Monitoring

**4.1 — Reservation Detail Page (`/reservations/[id]`)**
- [ ] `+page.server.ts` calls `Inspect()` with reservation_id
- [ ] Displays full `ReservationInfo`:
  - ID (copyable UUID)
  - Owner (parsed with type badge)
  - Ports: assigned (prominent), requested (if different, show "requested X, got Y"), target
  - State with badge
  - Timestamps: created, updated (relative + absolute on hover)
  - Lease: duration, expires_at with countdown
- [ ] For VM reservations (`vm:*`), show **Tunnel Health** panel:
  - Alive indicator (green pulse / red)
  - Uptime duration
  - Last health check time
  - Reconnect count (with warning if > 0)
- [ ] For host reservations (`host:*`), show "Direct host port hold — no tunnel" instead of health panel
  - Do NOT show fabricated TunnelHealth (daemon returns `alive: true, uptime: 0` for host reservations, which is misleading)
- [ ] Release button (with appropriate confirmation)
- [ ] Auto-refresh health data every 10 seconds via polling `Inspect()`
- [ ] Use `AbortController` per polling cycle; abort on navigation/unmount
- [ ] Pause polling when tab is hidden (`document.hidden` check)

### Research Insights: Detail Page Polling (from races reviewer)

```typescript
let abortController: AbortController | null = null;

const pollInterval = setInterval(async () => {
  if (document.hidden) return; // Don't poll hidden tabs
  abortController?.abort();
  abortController = new AbortController();
  try {
    const res = await fetch(`/api/inspect/${id}`, { signal: abortController.signal });
    // update health state
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') return;
    // handle real errors
  }
}, 10_000);

onDestroy(() => {
  clearInterval(pollInterval);
  abortController?.abort();
});
```

**4.2 — Tunnel Health in Dashboard**
- [ ] Small health indicator dot on each VM reservation row (green/red)
- [ ] Tooltip on hover showing uptime and reconnect count
- [ ] No health indicator for host reservations

**4.3 — Lease Countdown**
- [ ] Client-side countdown derived reactively from store (not captured value):
  ```typescript
  let now = $state(Date.now());
  const interval = setInterval(() => { now = Date.now(); }, 1000);
  const remaining = $derived(
    reservation.expiresAt
      ? Math.max(0, reservation.expiresAt - now)
      : Infinity
  );
  ```
- [ ] Shows "5m 23s remaining" → "Expired — awaiting cleanup" when countdown hits zero but state still active
- [ ] "Indefinite" label for reservations without a lease
- [ ] Self-corrects when `expires_at` changes underneath (SSE update, reconciliation)

### Phase 5: Filtering, Grouping, & Polish

**5.1 — Filtering**
- [ ] Filter bar above the table:
  - Owner type: All / Host / VM (quick-toggle buttons)
  - State: All / Active / Pending / Failed (multi-select or toggle chips)
  - Search: free-text filter on owner string
- [ ] Filters applied client-side on the loaded reservation list (dataset is small enough)
- [ ] URL query params reflect filter state (shareable/bookmarkable)

**5.2 — Sorting**
- [ ] Clickable column headers for sorting:
  - Assigned port (default, ascending)
  - Owner (alphabetical)
  - State (pending → active → failed → released)
  - Created (newest first)
  - Lease expiry (soonest first)
- [ ] Sort state persisted in URL query params

**5.3 — Grouping by VM**
- [ ] Optional toggle to group reservations by VM name
- [ ] Collapsible sections: "host" group, "vm:smith" group, "vm:jones" group
- [ ] Per-group summary: count, port range used

**5.4 — Released Reservations**
- [ ] Hidden by default
- [ ] "Show released" toggle in filter bar
- [ ] When enabled, calls `List(state_filter: RELEASED)` and appends to table
- [ ] Released rows visually muted (lower opacity, strikethrough port)

**5.5 — Daemon Connectivity**
- [ ] Persistent status indicator in the layout header
- [ ] When daemon is unreachable:
  - Banner: "Cannot connect to portd. Is the daemon running?"
  - Hint: `portd` command to start it
  - All mutation buttons disabled
  - Dashboard shows last-known state (if any) with "stale" warning
- [ ] Auto-recovery: poll `/api/reservations` (via dashboard load) every 5 seconds when disconnected
  - Set 2-second gRPC deadline on health checks to prevent pileup if daemon is hung
- [ ] On reconnection: wait for `EventSource.onopen` before declaring success, then full refresh + toast

**5.6 — Error Handling & Toasts**
- [ ] Toast notification system (shadcn-svelte Sonner or similar)
- [ ] gRPC error → user message mapping in `$lib/utils/errors.ts`:

  | gRPC Status | UI Message |
  |---|---|
  | `ALREADY_EXISTS` | "Port {port} is already reserved by {owner}" |
  | `RESOURCE_EXHAUSTED` | "No ports available in range 10000-60000" or "Owner limit (100) reached" |
  | `NOT_FOUND` | "Reservation not found" |
  | `FAILED_PRECONDITION` | "VM '{name}' is not configured in ssh.toml" |
  | `UNAVAILABLE` (channel connected) | "SSH connection to {vm} failed: {detail}" |
  | `UNAVAILABLE` (channel not connected) | "Cannot reach portd. Is the daemon running?" |
  | `INTERNAL` | "Internal error — check portd logs" |
  | `INVALID_ARGUMENT` | Show specific field validation error |

- [ ] Disambiguate `UNAVAILABLE` — check channel connectivity to distinguish "daemon down" from "SSH failure"
- [ ] Error messages use the daemon's detail string as-is within the wrapper (don't parse with regex)

**5.7 — Accessibility**
- [ ] State badges use icon + text + color (not color alone)
- [ ] Keyboard navigation for table rows and actions
- [ ] Proper ARIA labels on interactive elements
- [ ] Focus management after dialogs close

## Technical Considerations

### gRPC-to-SSE Bridge Design

The server maintains **one shared gRPC Watch stream** and fans out to connected SSE clients. Both the channel and the fan-out subscriber set live on `globalThis` to survive HMR.

```
gRPC Watch() ──→ globalThis fan-out ──→ SSE Client 1
                                    ──→ SSE Client 2
                                    ──→ SSE Client N (max 10)
```

If the gRPC stream dies (daemon restart), the server reconnects with exponential backoff and sends a `event: refresh` SSE event telling clients to do a full `List()` refresh.

### Reactive Store Design (from TypeScript + races reviewers)

```typescript
// $lib/stores/reservations.svelte.ts
class ReservationStore {
  #reservations = $state<Map<string, ReservationViewModel>>(new Map());
  #loading = $state(false);
  #error = $state<string | null>(null);

  get reservations() { return [...this.#reservations.values()]; }
  get loading() { return this.#loading; }
  get error() { return this.#error; }

  upsert(r: ReservationViewModel) { /* merge with updated_at check */ }
  mergeMany(rs: ReservationViewModel[]) { /* batch merge, don't replace */ }
  applyEvent(id: string, newState: string) { /* SSE event handler */ }
  markReleasing(id: string) { /* set UI transient state */ }
}

export const reservationStore = new ReservationStore();
```

**Key rules:**
- Never expose raw `$state` for direct assignment — all mutations through methods
- Use `Map<string, ReservationViewModel>` not array — O(1) lookup by ID
- `mergeMany` checks `updatedAt` per-reservation — never overwrites fresher SSE data with stale `List()`
- `applyEvent` respects UI transient state (don't overwrite `UI_RELEASING` optimistic state)

### Proto→View Model Mapper

```typescript
// $lib/mappers/reservation.ts
const STATE_MAP = { 1: "pending", 2: "active", 3: "failed", 4: "released" } as const satisfies Record<number, string>;

export function toViewModel(proto: ReservationInfo): ReservationViewModel {
  return {
    id: proto.id,
    owner: parseOwner(proto.owner), // returns ParsedOwner with UnknownOwner fallback
    assignedPort: proto.assignedPort,
    targetHost: proto.targetHost,
    targetPort: proto.targetPort,
    state: STATE_MAP[proto.state] ?? "pending",
    createdAt: timestampToDate(proto.createdAt),
    updatedAt: timestampToDate(proto.updatedAt),
    expiresAt: proto.expiresAt ? timestampToDate(proto.expiresAt) : null,
    leaseSeconds: proto.leaseSeconds ?? null,
  };
}
```

### Owner String Parsing

```
"host:web"        → { type: "host", service: "web" }
"vm:smith:api"    → { type: "vm", vm: "smith", service: "api" }
"vm:smith:db.main"→ { type: "vm", vm: "smith", service: "db.main" }
"anything:else"   → { type: "unknown", raw: "anything:else" }
```

### Port Display

The dashboard emphasizes `assigned_port` as the primary identifier (this is what users connect to). The `target_host:target_port` is shown as the forwarding destination. If `requested_port` differs from `assigned_port`, show a subtle "(requested 8080)" note.

### Timestamp Handling

Proto `Timestamp { seconds, nanos }` → JavaScript `Date` via manual conversion in the mapper layer. Display as relative time ("2m ago") with absolute time on hover using `Intl.RelativeTimeFormat`.

## System-Wide Impact

- **No changes to portd or portctl** — the UI is a pure client of the existing gRPC API
- **No database changes** — reads/writes go through the gRPC service
- **New directory `./ui`** — independent Node.js project, not part of the Cargo workspace
- **Proto file is shared** — `ui/proto-gen.sh` references `../proto/portd.proto`
- **New file `hooks.server.ts`** — adds Origin validation for security

## Dependencies & Risks

| Risk | Mitigation |
|---|---|
| Daemon not running | Clear "daemon offline" state with startup instructions |
| SSE events missed (Watch buffer overflow) | Periodic reconciliation via List() every 30s + `event: refresh` on lag |
| Proto file changes | proto-gen.sh re-run needed; add to dev workflow |
| nice-grpc UDS support | Verified: @grpc/grpc-js supports `unix:` URIs natively |
| Svelte 5 + shadcn-svelte compatibility | shadcn-svelte has Svelte 5 support since 2025 |
| Cross-origin attacks on localhost | Origin header validation in hooks.server.ts |
| SSE event beats POST response | Per-reservation UI state machine with transient states |
| Stale List() overwrites fresh SSE data | Merge-not-replace with `updatedAt` comparison |
| HMR orphans fan-out listeners | Both gRPC client and fan-out on `globalThis` |
| Daemon hung (not down) | 2-second gRPC deadline on health checks |

## Implementation Order

1. **Phase 1** — Scaffold SvelteKit project, proto codegen, gRPC client, dashboard with static data, security hooks
2. **Phase 2** — Reserve form, Release action, error handling, UI state machine
3. **Phase 3** — SSE Watch bridge, real-time updates, EventSource wrapper, reconciliation
4. **Phase 4** — Inspect detail page, tunnel health display, lease countdown
5. **Phase 5** — Filtering, sorting, grouping, released toggle, polish

Each phase produces a working, shippable increment.

## Design Decisions

1. **nice-grpc over raw @grpc/grpc-js** — AsyncIterable for Watch stream maps cleanly to SSE ReadableStream; much better TypeScript DX
2. **SSE over WebSocket** — Unidirectional (server→client) matches Watch RPC; SvelteKit has no native WebSocket support; SSE needs no upgrade negotiation
3. **Shared Watch stream** — One gRPC connection per server process, fan-out to N browser clients; efficient for a local dev tool
4. **Client-side filtering** — Dataset is small (tens to low hundreds of reservations); avoids unnecessary server round-trips
5. **Origin validation (not full auth)** — Blocks cross-origin attacks from malicious websites while keeping the tool zero-config for local use
6. **Structured owner input** — Prevents malformed owner strings; radio toggle for host/vm determines form fields
7. **No new RPCs required** — The existing 5 RPCs cover all UI needs; VM name is free-text (no ListVMs RPC needed for MVP)
8. **ts-proto `useDate=false`** — Avoid lossy Date conversion; manual conversion in mapper layer preserves nanosecond fidelity
9. **Class-based store over raw `$state`** — Enforces mutation through methods, prevents raw array replacement races
10. **Merge-not-replace reconciliation** — Prevents stale `List()` from overwriting fresh SSE data

## Sources & References

### Internal References
- Proto definition: `proto/portd.proto`
- gRPC implementation: `crates/daemon/src/grpc.rs`
- Broker logic: `crates/daemon/src/broker.rs`
- Error types: `crates/core/src/error.rs`
- Input validation: `crates/daemon/src/grpc.rs:21-58`
- Watch broadcast: `crates/daemon/src/broker.rs:97` (256-slot broadcast channel)
- Config defaults: `crates/daemon/src/config.rs:85-117`
- Socket permissions: `crates/daemon/src/main.rs:154` (0o600)
- Tunnel health check: `crates/daemon/src/tunnel.rs:253-359`
- Lease cleanup interval: `crates/daemon/src/lease.rs` (60 seconds)

### External References
- [SvelteKit docs](https://svelte.dev/docs/kit)
- [SvelteKit adapter-node](https://svelte.dev/docs/kit/adapter-node)
- [SvelteKit server-only modules](https://svelte.dev/docs/kit/server-only-modules)
- [nice-grpc](https://github.com/deeplay-io/nice-grpc)
- [ts-proto](https://github.com/stephenh/ts-proto)
- [ts-proto nice-grpc flags](https://github.com/stephenh/ts-proto/blob/main/README.markdown)
- [shadcn-svelte](https://www.shadcn-svelte.com)
- [shadcn-svelte data table](https://shadcn-svelte.com/docs/components/data-table)
- [shadcn-svelte dark mode](https://shadcn-svelte.com/docs/dark-mode/svelte)
- [@grpc/grpc-js UDS support](https://github.com/grpc/grpc-node)
- [mode-watcher](https://github.com/svecosystem/mode-watcher)
- [Svelte 5 $state runes](https://svelte.dev/docs/svelte/$state)
- [gRPC naming spec (unix: URI)](https://grpc.github.io/grpc/cpp/md_doc_naming.html)
