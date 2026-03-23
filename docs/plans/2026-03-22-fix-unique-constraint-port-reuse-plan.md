---
title: "fix: UNIQUE constraint blocks port reuse after release"
type: fix
status: completed
date: 2026-03-22
---

# fix: UNIQUE constraint blocks port reuse after release

## Enhancement Summary

**Deepened on:** 2026-03-23
**Research agents used:** 7 (best-practices, framework-docs, security-sentinel, data-migration-expert, performance-oracle, code-simplicity-reviewer, data-integrity-guardian)

### Key Improvements
1. Discovered critical race condition in `expire_leases()` that must be fixed alongside the bug
2. Replaced hand-rolled migration runner (Part 3) with `sqlx::migrate!()` — built-in, battle-tested
3. Added `BEGIN EXCLUSIVE` transaction wrapping and explicit column lists to the migration SQL
4. Clearly separated "must-do" (Part 1) from "defense-in-depth" (Part 2) based on simplicity analysis

### New Considerations Discovered
- `expire_leases()` SELECT-then-UPDATE is not atomic — interleaves with Broker operations
- `sqlx::migrate!()` already provides version tracking, checksums, and auto-transaction wrapping
- The `-- no-transaction` directive is needed for table-rebuild migrations that use PRAGMAs
- `max_connections(1)` is load-bearing for correctness — must be documented

---

## Overview

The `assigned_port` column has an unconditional `UNIQUE` constraint (`migrations/001_initial.sql:5`). When a reservation is released, the row is soft-deleted (`state = 'released'`) but remains in the table. Any subsequent INSERT for the same port fails with a UNIQUE constraint violation, even though the allocation logic correctly considers released ports as available.

## Problem Statement

**Timeline of failure:**

1. Port 8080 reserved — row inserted with `assigned_port = 8080`, state `active`.
2. Port 8080 released — row UPDATE'd to `state = 'released'`, row **remains**.
3. New reservation requests port 8080.
4. `allocate_port()` queries `WHERE state != 'released'`, does not see the old row, returns 8080 as available.
5. INSERT of new row with `assigned_port = 8080` **fails**: UNIQUE constraint violated.
6. Released row would not be purged for up to 24 hours (`released_ttl_secs = 86400`).

This blocks the most basic port lifecycle: reserve → release → re-reserve.

## Proposed Solution

### Part 1: DELETE-before-INSERT (the fix — ~5 lines)

In the `reserve()` transaction in `broker.rs`, before the INSERT, delete any released row occupying the target port:

```sql
DELETE FROM reservations WHERE assigned_port = ? AND state = 'released'
```

This runs inside the existing transaction, so it is atomic with the INSERT. It solves the bug immediately without requiring a schema migration.

**Why this is safe:** The `purge_released()` task in `lease.rs` already deletes released rows on a TTL. This just makes it happen eagerly for the specific port being reused, which is strictly better — the released row for that port has no further value once a new reservation claims it.

**Why this alone is sufficient:** The DELETE and INSERT are in the same transaction. There is no window where a conflicting row can exist. The UNIQUE constraint will never fire because the conflicting row is gone before the INSERT executes.

### Research Insights — Part 1

**All 7 review agents agree Part 1 is necessary and sufficient for the immediate bug fix.**

**Critical placement detail:** The DELETE must be inside the existing `pool.begin()` transaction at `broker.rs:119`, not before it. If the DELETE runs outside the transaction and the INSERT fails, the DELETE cannot be rolled back and reservation history is permanently lost.

**Performance:** At this scale (~10-100 reservations), an extra DELETE inside a SQLite transaction adds microseconds. Not a concern.

**Audit trail impact:** The DELETE bypasses the `released_ttl_secs` window for the specific port being reused. This is acceptable — if the port is being reused, the old released row has no further diagnostic value. Other released rows (for ports not being reused) are still retained for the full TTL.

### Part 1b: Fix expire_leases() race condition (critical prerequisite)

**Discovered during deepening — not in original plan.**

The `expire_leases()` function in `lease.rs:88-124` performs a SELECT then a separate UPDATE without a transaction. Between these two statements, the Broker can interleave operations:

1. LeaseCleanup SELECTs row R (port 8080) as expired.
2. Broker's `reserve()` DELETEs the released row for port 8080, then INSERTs a new reservation.
3. LeaseCleanup UPDATEs — but row R was deleted. The UPDATE is a silent no-op.
4. LeaseCleanup sends `ExpiredReservation { id: R, assigned_port: 8080 }` on the channel.
5. The expired-reservation handler calls `release_hold_listener(8080)` — destroying the **new** reservation's hold listener.

**Fix:** Wrap the SELECT + UPDATE in a `BEGIN IMMEDIATE` transaction, or use `UPDATE ... RETURNING` (supported in SQLite 3.35+, and we're on 3.46.0):

```rust
// In lease.rs — replace separate SELECT + UPDATE with atomic operation:
let rows = sqlx::query_as::<_, (String, i64)>(
    "UPDATE reservations SET state = 'released' \
     WHERE state IN ('active', 'pending') \
     AND expires_at IS NOT NULL \
     AND expires_at <= datetime('now') \
     RETURNING id, assigned_port"
)
.fetch_all(&self.pool)
.await?;
```

This is a single statement — atomic by definition, no interleaving possible.

### Part 2: Partial unique index migration (optional defense-in-depth)

Replace the column-level UNIQUE with a partial unique index that only enforces uniqueness on non-released rows. This protects against any future code path that inserts without the DELETE guard.

**Migration 002** must use the SQLite table-rebuild pattern because SQLite cannot `ALTER TABLE DROP CONSTRAINT`:

1. `CREATE TABLE reservations_new (...)` — same schema, no UNIQUE on assigned_port
2. `INSERT INTO reservations_new (explicit columns) SELECT explicit columns FROM reservations`
3. `DROP TABLE reservations`
4. `ALTER TABLE reservations_new RENAME TO reservations`
5. Recreate trigger, all indexes, and add the new partial unique index
6. `CREATE UNIQUE INDEX idx_assigned_port_active ON reservations(assigned_port) WHERE state != 'released'`

### Research Insights — Part 2

**Simplicity analysis:** The simplicity reviewer argues Part 2 is YAGNI — Part 1 already guarantees no constraint violations, and the table-rebuild migration is the riskiest part of the plan. For a dev tool with ~100 rows, schema-level defense-in-depth may not justify the complexity.

**Counter-argument:** The security and best-practices reviewers note that the partial index catches bugs regardless of application logic. If any future code path does an INSERT without the DELETE guard, the index prevents corruption silently.

**If you proceed with Part 2, these issues must be addressed:**

1. **Transaction wrapping:** The migration SQL must use `BEGIN EXCLUSIVE` (not just `BEGIN`) to acquire a write lock immediately, preventing concurrent access during the rebuild.

2. **Use explicit column lists**, not `SELECT *`:
   ```sql
   INSERT INTO reservations_new
       (id, owner, requested_port, assigned_port, target_host, target_port,
        state, lease_seconds, expires_at, created_at, updated_at, reconnect_count)
   SELECT id, owner, requested_port, assigned_port, target_host, target_port,
          state, lease_seconds, expires_at, created_at, updated_at, reconnect_count
   FROM reservations;
   ```
   `SELECT *` is fragile — if column order ever diverges between old and new table definitions, data silently goes to the wrong columns.

3. **PRAGMA handling:** `PRAGMA foreign_keys=OFF` must be outside the transaction (it's a no-op inside `BEGIN`). Use the `-- no-transaction` directive with `sqlx::migrate!()`.

4. **Dedup before partial index creation:** If duplicate `assigned_port` values exist among non-released rows (shouldn't happen, but prevents migration failure):
   ```sql
   DELETE FROM reservations
   WHERE state = 'released'
   AND rowid NOT IN (
       SELECT MAX(rowid) FROM reservations
       WHERE state = 'released'
       GROUP BY assigned_port
   );
   ```

5. **Pre-migration backup:** Copy the SQLite file before destructive DDL:
   ```rust
   let backup = format!("{}.pre-002.bak", db_path.display());
   std::fs::copy(db_path, &backup)?;
   ```

### Part 3: Switch to sqlx::migrate!() (replaces hand-rolled runner)

**Updated based on framework research.** The original plan proposed a hand-rolled migration runner. `sqlx::migrate!()` already provides everything we need:

- Version tracking via `_sqlx_migrations` table
- SHA-384 checksum validation (detects edited migrations)
- Auto-transaction wrapping per migration
- `-- no-transaction` directive for table-rebuild migrations
- Migrations embedded in the binary at compile time

**Changes required:**

1. Add `"migrate"` feature to sqlx in workspace `Cargo.toml`:
   ```toml
   sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }
   ```

2. Replace `run_migrations()` in `db.rs`:
   ```rust
   pub async fn init_pool(db_path: &Path) -> anyhow::Result<SqlitePool> {
       // ... pool setup ...

       sqlx::migrate!("../../migrations")
           .run(&pool)
           .await
           .context("failed to run migrations")?;

       info!(path = %db_path.display(), "database initialized");
       Ok(pool)
   }
   ```

3. Rename migration files to sqlx convention:
   ```
   migrations/
     20240101000000_initial.sql         (renamed from 001_initial.sql)
     20260323000000_partial_unique.sql  (new, if Part 2 is implemented)
   ```

**Caveat:** Once `sqlx::migrate!()` records a migration as applied, its content is immutable. Any edit to `001_initial.sql` after first run causes a `VersionMismatch` error. This is a feature — it prevents schema drift.

## Technical Considerations

- **SQLite DDL is transactional** — the table-rebuild migration is safe inside a transaction.
- **Single-writer pool** (`max_connections(1)` in `db.rs:24`) serializes all DB access, preventing races between the DELETE-INSERT in `reserve()` and the `expire_leases()` in `lease.rs`. A code comment should document this as load-bearing for correctness.
- **Multiple released rows for the same port** can accumulate between purge cycles. The DELETE-before-INSERT in Part 1 prevents this for reused ports. The purge job handles the rest.
- **Existing databases** may have released rows with duplicate `assigned_port` values. Migration 002 should dedup before creating the partial unique index.
- **SQLite version:** The project bundles SQLite 3.46.0 via `libsqlite3-sys 0.30.1`. Partial indexes (3.8.0+) and `UPDATE ... RETURNING` (3.35.0+) are both supported.

### Research Insights — Technical

**Performance:** No concerns at this scale. The extra DELETE adds microseconds. The table rebuild for ~100 rows completes in single-digit milliseconds. The partial unique index is strictly better for write performance than a full index (inserts for released rows skip it).

**Partial index query planner gotcha:** SQLite only uses a partial index if the query's WHERE clause exactly matches a term in the index WHERE clause. `state != 'released'` matches, but `state IN ('active', 'pending')` does **not**. Keep query patterns consistent.

**Recursive trigger safety:** The `update_reservation_timestamp` trigger fires on UPDATE and executes an UPDATE on the same table. SQLite's default `PRAGMA recursive_triggers = OFF` prevents infinite recursion. Document this assumption.

## Acceptance Criteria

- [x] `reserve → release → reserve` for the same port succeeds (the core bug)
- [x] `reserve → lease-expire → reserve` for the same port succeeds
- [x] Idempotent reserve (same owner+target) still returns existing reservation
- [x] Released rows for reused ports are cleaned up eagerly on re-reserve
- [x] `expire_leases()` uses atomic `UPDATE ... RETURNING` (no race window)
- [x] `cargo check` passes with zero warnings
- [x] Comment in `db.rs` documents that `max_connections(1)` is required for correctness
- [ ] (If Part 2) Migration applies cleanly on both fresh and existing databases
- [x] `sqlx::migrate!()` replaces hand-rolled runner

## MVP

### broker.rs — DELETE-before-INSERT in reserve transaction

```rust
// Inside the existing transaction (after line 119: self.pool.begin()),
// BEFORE the INSERT:
sqlx::query(
    "DELETE FROM reservations WHERE assigned_port = ? AND state = 'released'"
)
.bind(port as i64)
.execute(&mut *tx)
.await
.map_err(|e| PortError::Database(e.to_string()))?;
```

### lease.rs — atomic expire_leases()

```rust
async fn expire_leases(&self) -> Result<Vec<ExpiredReservation>, sqlx::Error> {
    // Single atomic statement — no race window between SELECT and UPDATE
    let rows = sqlx::query_as::<_, (String, i64)>(
        "UPDATE reservations SET state = 'released' \
         WHERE state IN ('active', 'pending') \
         AND expires_at IS NOT NULL \
         AND expires_at <= datetime('now') \
         RETURNING id, assigned_port"
    )
    .fetch_all(&self.pool)
    .await?;

    let expired: Vec<ExpiredReservation> = rows
        .into_iter()
        .map(|(id, port)| {
            info!(id = %id, assigned_port = port, "lease expired, reservation released");
            ExpiredReservation {
                id,
                assigned_port: port as u16,
            }
        })
        .collect();

    Ok(expired)
}
```

### migrations/20260323000000_partial_unique.sql (if Part 2 is implemented)

```sql
-- no-transaction
PRAGMA foreign_keys=OFF;

BEGIN EXCLUSIVE;

-- Dedup released rows before rebuilding
-- Keep one released row per port (arbitrary choice; all are dead data)
DELETE FROM reservations
WHERE state = 'released'
AND rowid NOT IN (
    SELECT MAX(rowid) FROM reservations
    WHERE state = 'released'
    GROUP BY assigned_port
);

-- Rebuild table without column-level UNIQUE
CREATE TABLE reservations_new (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    requested_port INTEGER,
    assigned_port INTEGER NOT NULL,
    target_host TEXT NOT NULL,
    target_port INTEGER NOT NULL
        CHECK(target_port BETWEEN 1 AND 65535),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK(state IN ('pending', 'active', 'failed', 'released')),
    lease_seconds INTEGER,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    reconnect_count INTEGER NOT NULL DEFAULT 0
);

INSERT INTO reservations_new
    (id, owner, requested_port, assigned_port, target_host, target_port,
     state, lease_seconds, expires_at, created_at, updated_at, reconnect_count)
SELECT id, owner, requested_port, assigned_port, target_host, target_port,
       state, lease_seconds, expires_at, created_at, updated_at, reconnect_count
FROM reservations;

DROP TABLE reservations;
ALTER TABLE reservations_new RENAME TO reservations;

-- Recreate trigger
CREATE TRIGGER update_reservation_timestamp
    AFTER UPDATE ON reservations
    BEGIN
        UPDATE reservations SET updated_at = datetime('now')
        WHERE id = NEW.id;
    END;

-- Recreate indexes
CREATE INDEX idx_reservations_owner ON reservations(owner);
CREATE INDEX idx_reservations_state ON reservations(state);
CREATE INDEX idx_reservations_owner_target
    ON reservations(owner, target_host, target_port);
CREATE INDEX idx_reservations_expiry
    ON reservations(state, expires_at)
    WHERE expires_at IS NOT NULL;
CREATE INDEX idx_reservations_released_cleanup
    ON reservations(state, updated_at)
    WHERE state = 'released';

-- The fix: partial unique index
CREATE UNIQUE INDEX idx_assigned_port_active
    ON reservations(assigned_port)
    WHERE state != 'released';

COMMIT;

PRAGMA foreign_keys=ON;
```

### db.rs — switch to sqlx::migrate!()

```rust
pub async fn init_pool(db_path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create DB directory: {}", parent.display()))?;
    }

    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let options = SqliteConnectOptions::from_str(&db_url)?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(1) // Single-writer — REQUIRED for correctness.
                            // Broker and LeaseCleanup share this pool without
                            // application-level locking. Increasing this breaks
                            // transaction isolation assumptions.
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open database: {}", db_path.display()))?;

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    info!(path = %db_path.display(), "database initialized");
    Ok(pool)
}
```

## Pre-existing Issues Noted by Security Review

These are outside the scope of this bug fix but worth tracking:

- **SSH host key verification disabled** (`tunnel.rs:60-67`) — always accepts any host key. Acceptable for local dev but should be configurable.
- **Socket permissions 0660** (`main.rs:137`) — allows group access with no gRPC authentication. Consider `0600`.
- **LIKE pattern not escaped** (`broker.rs:230`) — `owner_filter` value used directly in LIKE pattern. A client can send `%` to match all owners.
- **No input validation** on `owner` or `target_host` string lengths.

## Sources

- `migrations/001_initial.sql:5` — the problematic `assigned_port INTEGER NOT NULL UNIQUE`
- `crates/daemon/src/broker.rs:119` — transaction boundary where DELETE must be placed
- `crates/daemon/src/broker.rs:321-380` — `allocate_port()` with `WHERE state != 'released'`
- `crates/daemon/src/broker.rs:72-171` — `reserve()` INSERT logic
- `crates/daemon/src/broker.rs:200-220` — `do_release()` soft-delete
- `crates/daemon/src/lease.rs:88-124` — `expire_leases()` non-atomic SELECT+UPDATE (the race)
- `crates/daemon/src/lease.rs:126-139` — `purge_released()` TTL cleanup
- `crates/daemon/src/db.rs:24` — `max_connections(1)` load-bearing for correctness
- `crates/daemon/src/db.rs:36-43` — hardcoded single-migration runner to replace

### External References

- [SQLite ALTER TABLE (official)](https://www.sqlite.org/lang_altertable.html)
- [SQLite Partial Indexes (official)](https://www.sqlite.org/partialindex.html)
- [sqlx::migrate! macro docs](https://docs.rs/sqlx/latest/sqlx/macro.migrate.html)
- [sqlx issue #2085 — foreign_keys pragma in migrations](https://github.com/launchbadge/sqlx/issues/2085)
