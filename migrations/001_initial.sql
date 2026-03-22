CREATE TABLE IF NOT EXISTS reservations (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    requested_port INTEGER,
    assigned_port INTEGER NOT NULL UNIQUE,
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

-- Auto-update updated_at on any UPDATE
CREATE TRIGGER IF NOT EXISTS update_reservation_timestamp
    AFTER UPDATE ON reservations
    BEGIN
        UPDATE reservations SET updated_at = datetime('now')
        WHERE id = NEW.id;
    END;

CREATE INDEX IF NOT EXISTS idx_reservations_owner ON reservations(owner);
CREATE INDEX IF NOT EXISTS idx_reservations_state ON reservations(state);
-- Note: assigned_port UNIQUE constraint already creates an implicit index

-- Composite indexes for actual query patterns
CREATE INDEX IF NOT EXISTS idx_reservations_owner_target
    ON reservations(owner, target_host, target_port);
CREATE INDEX IF NOT EXISTS idx_reservations_expiry
    ON reservations(state, expires_at)
    WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_reservations_released_cleanup
    ON reservations(state, updated_at)
    WHERE state = 'released';
