-- Partial unique index: defense-in-depth for port reuse bug fix.
-- Replaces the column-level UNIQUE on assigned_port with a partial
-- unique index that only enforces uniqueness on non-released rows.
--
-- Uses SQLite table-rebuild pattern because SQLite cannot DROP an
-- inline column-level UNIQUE constraint.

-- Dedup: keep one released row per port (arbitrary; all are dead data)
DELETE FROM reservations
WHERE state = 'released'
AND rowid NOT IN (
    SELECT MAX(rowid) FROM reservations
    WHERE state = 'released'
    GROUP BY assigned_port
);

-- Rebuild table without column-level UNIQUE on assigned_port
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

-- Recreate all original indexes
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

-- The fix: partial unique index replaces the column-level UNIQUE
CREATE UNIQUE INDEX idx_assigned_port_active
    ON reservations(assigned_port)
    WHERE state != 'released';
