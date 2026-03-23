use port_authority_core::error::PortError;
use port_authority_core::types::ReservationState;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::net::TcpListener as StdTcpListener;
use tokio::sync::broadcast;
use tracing::{debug, info};

/// An internal event emitted by the Broker when reservation state changes.
#[derive(Debug, Clone)]
pub struct BrokerEvent {
    pub reservation_id: String,
    pub owner: String,
    pub old_state: String,
    pub new_state: String,
    pub message: Option<String>,
}
use uuid::Uuid;

/// A reservation record as stored in the database.
#[derive(Debug, Clone)]
pub struct Reservation {
    pub id: String,
    pub owner: String,
    pub requested_port: Option<i64>,
    pub assigned_port: i64,
    pub target_host: String,
    pub target_port: i64,
    pub state: String,
    pub lease_seconds: Option<i64>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub reconnect_count: i64,
}

impl Reservation {
    fn from_row(row: sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            owner: row.try_get("owner")?,
            requested_port: row.try_get("requested_port")?,
            assigned_port: row.try_get("assigned_port")?,
            target_host: row.try_get("target_host")?,
            target_port: row.try_get("target_port")?,
            state: row.try_get("state")?,
            lease_seconds: row.try_get("lease_seconds")?,
            expires_at: row.try_get("expires_at")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            reconnect_count: row.try_get("reconnect_count")?,
        })
    }
}

const SELECT_COLS: &str = "id, owner, requested_port, assigned_port, target_host, target_port, \
     state, lease_seconds, expires_at, created_at, updated_at, reconnect_count";

/// Escape special LIKE pattern characters so they match literally.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The port broker — manages reservations and port allocation.
/// All methods are &mut self; external synchronization is via tokio::sync::Mutex.
pub struct Broker {
    pool: SqlitePool,
    port_range: (u16, u16),
    max_per_owner: u32,
    /// Hold listeners for ports that are reserved but not yet tunneled.
    hold_listeners: HashMap<u16, StdTcpListener>,
    /// Broadcast channel for reservation state change events.
    event_tx: broadcast::Sender<BrokerEvent>,
}

/// Extract the owner prefix used for per-owner reservation counting.
///
/// For VM owners like "vm:smith:web", returns "vm:smith" (groups all services on that VM).
/// For other owners like "host:web", returns the full string.
fn owner_count_prefix(owner: &str) -> String {
    if owner.starts_with("vm:") {
        let parts: Vec<&str> = owner.splitn(3, ':').collect();
        if parts.len() >= 2 {
            format!("{}:{}", parts[0], parts[1])
        } else {
            owner.to_string()
        }
    } else {
        owner.to_string()
    }
}

impl Broker {
    pub fn new(pool: SqlitePool, port_range_start: u16, port_range_end: u16, max_per_owner: u32) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            pool,
            port_range: (port_range_start, port_range_end),
            max_per_owner,
            hold_listeners: HashMap::new(),
            event_tx,
        }
    }

    /// Subscribe to reservation state change events.
    pub fn subscribe(&self) -> broadcast::Receiver<BrokerEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a broker event. Failures (no receivers) are silently ignored.
    fn emit_event(&self, event: BrokerEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Get a clone of the event sender (for external emission, e.g. lease expiry).
    pub fn event_sender(&self) -> broadcast::Sender<BrokerEvent> {
        self.event_tx.clone()
    }

    /// Reserve a port. Returns the reservation record.
    ///
    /// For host-side reservations (owner starts with "host:"), no tunnel is needed —
    /// the reservation goes directly to "active" state.
    ///
    /// For VM reservations, the reservation starts in "pending" state.
    pub async fn reserve(
        &mut self,
        owner: &str,
        preferred_port: Option<u16>,
        target_host: &str,
        target_port: u16,
        lease_seconds: Option<u32>,
        exact_only: bool,
    ) -> Result<Reservation, PortError> {
        // Check for duplicate: same owner + same target
        let existing = sqlx::query(&format!(
            "SELECT {} FROM reservations WHERE owner = ? AND target_host = ? AND target_port = ? AND state != 'released'",
            SELECT_COLS
        ))
        .bind(owner)
        .bind(target_host)
        .bind(target_port as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PortError::Database(e.to_string()))?;

        if let Some(row) = existing {
            let r = Reservation::from_row(row).map_err(|e| PortError::Database(e.to_string()))?;
            debug!(id = %r.id, "returning existing reservation (idempotent)");
            return Ok(r);
        }

        // Enforce per-owner reservation limit
        let prefix = owner_count_prefix(owner);
        let like_pattern = format!("{}%", escape_like(&prefix));
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reservations WHERE owner LIKE ? ESCAPE '\\' AND state != 'released'",
        )
        .bind(&like_pattern)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| PortError::Database(e.to_string()))?;

        if count.0 as u32 >= self.max_per_owner {
            return Err(PortError::OwnerLimitExceeded(prefix, self.max_per_owner));
        }

        // Allocate a port
        let port = self.allocate_port(preferred_port, exact_only).await?;

        // Bind-and-hold: bind the port immediately to prevent TOCTOU races
        let listener = StdTcpListener::bind(("127.0.0.1", port)).map_err(|_| {
            PortError::PortUnavailable(port, "bind failed (port in use by another process)".into())
        })?;

        // Determine initial state
        let is_host = owner.starts_with("host:");
        let initial_state = if is_host {
            ReservationState::Active
        } else {
            ReservationState::Pending
        };

        let id = Uuid::new_v4().to_string();
        let state_str = initial_state.as_sql();

        // Insert in a transaction — rollback if anything fails
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;

        // Clear any released row holding this port to avoid UNIQUE constraint violation.
        // Released rows are dead data — purge_released() would eventually delete them anyway.
        sqlx::query("DELETE FROM reservations WHERE assigned_port = ? AND state = 'released'")
            .bind(port as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;

        if let Some(lease_secs) = lease_seconds.filter(|&s| s > 0) {
            sqlx::query(
                "INSERT INTO reservations (id, owner, requested_port, assigned_port, target_host, target_port, state, lease_seconds, expires_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now', '+' || ? || ' seconds'))",
            )
            .bind(&id)
            .bind(owner)
            .bind(preferred_port.map(|p| p as i64))
            .bind(port as i64)
            .bind(target_host)
            .bind(target_port as i64)
            .bind(state_str)
            .bind(lease_secs as i64)
            .bind(lease_secs as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;
        } else {
            sqlx::query(
                "INSERT INTO reservations (id, owner, requested_port, assigned_port, target_host, target_port, state, lease_seconds) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(owner)
            .bind(preferred_port.map(|p| p as i64))
            .bind(port as i64)
            .bind(target_host)
            .bind(target_port as i64)
            .bind(state_str)
            .bind(lease_seconds.map(|s| s as i64))
            .execute(&mut *tx)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;

        // Store the hold listener
        self.hold_listeners.insert(port, listener);

        info!(id = %id, port = port, owner = owner, state = %state_str, "port reserved");

        self.emit_event(BrokerEvent {
            reservation_id: id.clone(),
            owner: owner.to_string(),
            old_state: "unspecified".to_string(),
            new_state: state_str.to_string(),
            message: Some("new reservation".to_string()),
        });

        // Fetch and return the full record
        self.get_reservation_by_id(&id).await
    }

    /// Update the state of a reservation.
    pub async fn update_state(
        &self,
        reservation_id: &str,
        new_state: ReservationState,
    ) -> Result<(), PortError> {
        // Fetch old state and owner for event emission
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT state, owner FROM reservations WHERE id = ?")
                .bind(reservation_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| PortError::Database(e.to_string()))?;

        let (old_state, owner) =
            row.ok_or_else(|| PortError::ReservationNotFound(reservation_id.to_string()))?;

        sqlx::query("UPDATE reservations SET state = ? WHERE id = ?")
            .bind(new_state.as_sql())
            .bind(reservation_id)
            .execute(&self.pool)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;

        self.emit_event(BrokerEvent {
            reservation_id: reservation_id.to_string(),
            owner,
            old_state,
            new_state: new_state.as_sql().to_string(),
            message: None,
        });

        Ok(())
    }

    /// Release a reservation by ID.
    pub async fn release_by_id(&mut self, reservation_id: &str) -> Result<(), PortError> {
        let reservation = self.get_reservation_by_id(reservation_id).await?;
        self.do_release(&reservation).await
    }

    /// Release a reservation by port.
    pub async fn release_by_port(&mut self, port: u16) -> Result<(), PortError> {
        let reservation = self.get_reservation_by_port(port).await?;
        self.do_release(&reservation).await
    }

    async fn do_release(&mut self, reservation: &Reservation) -> Result<(), PortError> {
        // Released → released is a no-op (idempotent)
        if reservation.state == ReservationState::Released.as_sql() {
            return Ok(());
        }

        let port = reservation.assigned_port as u16;

        // Update state to released
        sqlx::query("UPDATE reservations SET state = 'released' WHERE id = ?")
            .bind(&reservation.id)
            .execute(&self.pool)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;

        // Free the hold listener
        self.hold_listeners.remove(&port);

        info!(id = %reservation.id, port = port, "reservation released");

        self.emit_event(BrokerEvent {
            reservation_id: reservation.id.clone(),
            owner: reservation.owner.clone(),
            old_state: reservation.state.clone(),
            new_state: "released".to_string(),
            message: None,
        });

        Ok(())
    }

    /// List reservations with optional filters.
    pub async fn list(
        &self,
        owner_filter: Option<&str>,
        state_filter: Option<&str>,
    ) -> Result<Vec<Reservation>, PortError> {
        let rows = match (owner_filter, state_filter) {
            (Some(owner), Some(state)) => {
                let pattern = format!("{}%", escape_like(owner));
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE owner LIKE ? ESCAPE '\\' AND state = ? ORDER BY assigned_port",
                    SELECT_COLS
                ))
                .bind(pattern)
                .bind(state)
                .fetch_all(&self.pool)
                .await
            }
            (Some(owner), None) => {
                let pattern = format!("{}%", escape_like(owner));
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE owner LIKE ? ESCAPE '\\' AND state != 'released' ORDER BY assigned_port",
                    SELECT_COLS
                ))
                .bind(pattern)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some(state)) => {
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE state = ? ORDER BY assigned_port",
                    SELECT_COLS
                ))
                .bind(state)
                .fetch_all(&self.pool)
                .await
            }
            (None, None) => {
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE state != 'released' ORDER BY assigned_port",
                    SELECT_COLS
                ))
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| PortError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| Reservation::from_row(row).map_err(|e| PortError::Database(e.to_string())))
            .collect()
    }

    /// Get a single reservation by ID.
    pub async fn get_reservation_by_id(&self, id: &str) -> Result<Reservation, PortError> {
        let row = sqlx::query(&format!(
            "SELECT {} FROM reservations WHERE id = ?",
            SELECT_COLS
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PortError::Database(e.to_string()))?
        .ok_or_else(|| PortError::ReservationNotFound(id.to_string()))?;

        Reservation::from_row(row).map_err(|e| PortError::Database(e.to_string()))
    }

    /// Release a hold listener for a given port (used when tunnel takes over the port).
    pub fn release_hold_listener(&mut self, port: u16) {
        self.hold_listeners.remove(&port);
    }

    /// Extract the VM name from an owner string like "vm:smith:web".
    /// Returns None if the owner is not a VM reservation.
    pub fn vm_name_from_owner(owner: &str) -> Option<&str> {
        if owner.starts_with("vm:") {
            owner.split(':').nth(1)
        } else {
            None
        }
    }

    /// Get a single reservation by assigned port.
    pub async fn get_reservation_by_port(&self, port: u16) -> Result<Reservation, PortError> {
        let row = sqlx::query(&format!(
            "SELECT {} FROM reservations WHERE assigned_port = ? AND state != 'released'",
            SELECT_COLS
        ))
        .bind(port as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PortError::Database(e.to_string()))?
        .ok_or_else(|| PortError::ReservationNotFound(format!("port {}", port)))?;

        Reservation::from_row(row).map_err(|e| PortError::Database(e.to_string()))
    }

    /// Allocate a port based on preference and availability.
    async fn allocate_port(
        &self,
        preferred: Option<u16>,
        exact_only: bool,
    ) -> Result<u16, PortError> {
        let (start, end) = self.port_range;

        // Get all currently assigned ports
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT assigned_port FROM reservations WHERE state != 'released'")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| PortError::Database(e.to_string()))?;

        let assigned_set: std::collections::HashSet<u16> =
            rows.iter().map(|r| r.0 as u16).collect();

        if let Some(preferred_port) = preferred {
            if !assigned_set.contains(&preferred_port) && self.can_bind(preferred_port) {
                return Ok(preferred_port);
            }

            if exact_only {
                if assigned_set.contains(&preferred_port) {
                    let owner: Option<(String,)> = sqlx::query_as(
                        "SELECT owner FROM reservations WHERE assigned_port = ? AND state != 'released'",
                    )
                    .bind(preferred_port as i64)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| PortError::Database(e.to_string()))?;

                    let owner_name = owner.map(|o| o.0).unwrap_or_else(|| "unknown".to_string());
                    return Err(PortError::PortUnavailable(preferred_port, owner_name));
                }
                return Err(PortError::ExactPortUnavailable(preferred_port));
            }

            // Fallback: scan upward from preferred, then wrap
            for port in preferred_port..=end {
                if !assigned_set.contains(&port) && self.can_bind(port) {
                    return Ok(port);
                }
            }
            for port in start..preferred_port {
                if !assigned_set.contains(&port) && self.can_bind(port) {
                    return Ok(port);
                }
            }
        } else {
            // No preference: allocate lowest available in range
            for port in start..=end {
                if !assigned_set.contains(&port) && self.can_bind(port) {
                    return Ok(port);
                }
            }
        }

        Err(PortError::PortRangeExhausted(start, end))
    }

    /// Check if a port can be bound (not in use by another process).
    fn can_bind(&self, port: u16) -> bool {
        StdTcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    #[cfg(test)]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    async fn setup_broker() -> (Broker, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(&db_url)
            .unwrap()
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .unwrap();

        let broker = Broker::new(pool, 10000, 60000, 100);
        (broker, dir)
    }

    #[tokio::test]
    async fn test_reserve_and_release_basic() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("host:web", None, "127.0.0.1", 8080, None, false)
            .await
            .unwrap();

        assert_eq!(r.state, "active");
        assert!((10000..=60000).contains(&(r.assigned_port as u16)));

        broker.release_by_id(&r.id).await.unwrap();

        let released = broker.get_reservation_by_id(&r.id).await.unwrap();
        assert_eq!(released.state, "released");
    }

    #[tokio::test]
    async fn test_reserve_release_reserve_same_port() {
        let (mut broker, _dir) = setup_broker().await;

        let r1 = broker
            .reserve("host:web", Some(12345), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();
        assert_eq!(r1.assigned_port, 12345);

        broker.release_by_id(&r1.id).await.unwrap();

        // Reserve the same preferred port again — this previously failed with a
        // UNIQUE constraint violation on assigned_port before the bug fix.
        let r2 = broker
            .reserve("host:web2", Some(12345), "127.0.0.1", 9090, None, true)
            .await
            .unwrap();
        assert_eq!(r2.assigned_port, 12345);
        assert_ne!(r2.id, r1.id);
    }

    #[tokio::test]
    async fn test_idempotent_reserve() {
        let (mut broker, _dir) = setup_broker().await;

        let r1 = broker
            .reserve("host:web", None, "127.0.0.1", 8080, None, false)
            .await
            .unwrap();
        let r2 = broker
            .reserve("host:web", None, "127.0.0.1", 8080, None, false)
            .await
            .unwrap();

        assert_eq!(r1.id, r2.id);
    }

    #[tokio::test]
    async fn test_preferred_port_fallback() {
        let (mut broker, _dir) = setup_broker().await;

        let r1 = broker
            .reserve("host:a", Some(12346), "127.0.0.1", 8080, None, false)
            .await
            .unwrap();
        assert_eq!(r1.assigned_port, 12346);

        let r2 = broker
            .reserve("host:b", Some(12346), "127.0.0.1", 9090, None, false)
            .await
            .unwrap();
        assert_ne!(r2.assigned_port, 12346);
    }

    #[tokio::test]
    async fn test_exact_port_unavailable() {
        let (mut broker, _dir) = setup_broker().await;

        broker
            .reserve("host:a", Some(12347), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();

        let result = broker
            .reserve("host:b", Some(12347), "127.0.0.1", 9090, None, true)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            PortError::PortUnavailable(port, _) => assert_eq!(port, 12347),
            other => panic!("expected PortUnavailable, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_list_filters() {
        let (mut broker, _dir) = setup_broker().await;

        let r1 = broker
            .reserve("host:a", None, "127.0.0.1", 8001, None, false)
            .await
            .unwrap();
        broker
            .reserve("host:b", None, "127.0.0.1", 8002, None, false)
            .await
            .unwrap();
        broker
            .reserve("vm:c:web", None, "127.0.0.1", 8003, None, false)
            .await
            .unwrap();

        // No filter — all 3
        let all = broker.list(None, None).await.unwrap();
        assert_eq!(all.len(), 3);

        // owner_filter="host" — 2 results
        let hosts = broker.list(Some("host"), None).await.unwrap();
        assert_eq!(hosts.len(), 2);

        // owner_filter="vm" — 1 result
        let vms = broker.list(Some("vm"), None).await.unwrap();
        assert_eq!(vms.len(), 1);

        // Release one, default list excludes released
        broker.release_by_id(&r1.id).await.unwrap();
        let after_release = broker.list(None, None).await.unwrap();
        assert_eq!(after_release.len(), 2);
    }

    #[tokio::test]
    async fn test_vm_reservation_starts_pending() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("vm:test:web", None, "127.0.0.1", 8080, None, false)
            .await
            .unwrap();

        assert_eq!(r.state, "pending");
    }

    #[tokio::test]
    async fn test_update_state() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("vm:test:web", None, "127.0.0.1", 8080, None, false)
            .await
            .unwrap();
        assert_eq!(r.state, "pending");

        broker
            .update_state(&r.id, ReservationState::Active)
            .await
            .unwrap();

        let updated = broker.get_reservation_by_id(&r.id).await.unwrap();
        assert_eq!(updated.state, "active");
    }

    #[tokio::test]
    async fn test_lease_expiration_cleanup() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("host:exp", Some(12348), "127.0.0.1", 8080, Some(1), true)
            .await
            .unwrap();
        assert_eq!(r.state, "active");
        assert!(r.expires_at.is_some());

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let pool = broker.pool();
        let rows = sqlx::query_as::<_, (String, i64)>(
            "UPDATE reservations SET state = 'released' \
             WHERE state IN ('active', 'pending') \
             AND expires_at IS NOT NULL \
             AND expires_at <= datetime('now') \
             RETURNING id, assigned_port",
        )
        .fetch_all(pool)
        .await
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, r.id);
        assert_eq!(rows[0].1, 12348);

        let updated = broker.get_reservation_by_id(&r.id).await.unwrap();
        assert_eq!(updated.state, "released");
    }

    #[tokio::test]
    async fn test_reserve_after_lease_expiry() {
        let (mut broker, _dir) = setup_broker().await;
        let pool = broker.pool().clone();

        let r1 = broker
            .reserve("host:exp2", Some(12350), "127.0.0.1", 8080, Some(1), true)
            .await
            .unwrap();
        assert_eq!(r1.assigned_port, 12350);

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let rows = sqlx::query_as::<_, (String, i64)>(
            "UPDATE reservations SET state = 'released' \
             WHERE state IN ('active', 'pending') \
             AND expires_at IS NOT NULL \
             AND expires_at <= datetime('now') \
             RETURNING id, assigned_port",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);

        // Release the hold listener so the port can be re-bound
        broker.release_hold_listener(12350);

        let r2 = broker
            .reserve("host:exp2b", Some(12350), "127.0.0.1", 9090, None, true)
            .await
            .unwrap();
        assert_eq!(r2.assigned_port, 12350);
        assert_ne!(r2.id, r1.id);
    }

    #[tokio::test]
    async fn test_purge_released_reservations() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("host:purge", Some(12351), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();
        broker.release_by_id(&r.id).await.unwrap();

        let pool = broker.pool();

        // Drop the auto-update trigger so we can backdate updated_at
        sqlx::query("DROP TRIGGER IF EXISTS update_reservation_timestamp")
            .execute(pool)
            .await
            .unwrap();

        // Backdate the updated_at so the reservation appears old
        sqlx::query("UPDATE reservations SET updated_at = datetime('now', '-2 days') WHERE id = ?")
            .bind(&r.id)
            .execute(pool)
            .await
            .unwrap();

        // Purge released reservations older than 1 day (86400 seconds)
        let result = sqlx::query(
            "DELETE FROM reservations WHERE state = 'released' \
             AND updated_at <= datetime('now', '-86400 seconds')",
        )
        .execute(pool)
        .await
        .unwrap();
        assert_eq!(result.rows_affected(), 1);

        // The reservation should no longer exist
        let gone = broker.get_reservation_by_id(&r.id).await;
        assert!(gone.is_err());
    }

    #[tokio::test]
    async fn test_event_broadcast() {
        let (mut broker, _dir) = setup_broker().await;
        let mut rx = broker.subscribe();

        let r = broker
            .reserve("host:evt", Some(12352), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.reservation_id, r.id);
        assert_eq!(event.new_state, "active");
        assert_eq!(event.old_state, "unspecified");

        broker.release_by_id(&r.id).await.unwrap();

        let event2 = rx.try_recv().unwrap();
        assert_eq!(event2.reservation_id, r.id);
        assert_eq!(event2.new_state, "released");
        assert_eq!(event2.old_state, "active");
    }

    #[tokio::test]
    async fn test_event_broadcast_on_update_state() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("vm:evt2:web", Some(12353), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();
        assert_eq!(r.state, "pending");

        let mut rx = broker.subscribe();

        broker
            .update_state(&r.id, ReservationState::Active)
            .await
            .unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.reservation_id, r.id);
        assert_eq!(event.old_state, "pending");
        assert_eq!(event.new_state, "active");
    }

    #[tokio::test]
    async fn test_release_idempotent() {
        let (mut broker, _dir) = setup_broker().await;

        let r = broker
            .reserve("host:idem", Some(12354), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();

        broker.release_by_id(&r.id).await.unwrap();
        // Second release should be a no-op, not an error
        broker.release_by_id(&r.id).await.unwrap();

        let released = broker.get_reservation_by_id(&r.id).await.unwrap();
        assert_eq!(released.state, "released");
    }

    #[tokio::test]
    async fn test_delete_clears_released_on_reuse() {
        let (mut broker, _dir) = setup_broker().await;
        let pool = broker.pool().clone();

        let r1 = broker
            .reserve("host:del1", Some(12349), "127.0.0.1", 8080, None, true)
            .await
            .unwrap();
        assert_eq!(r1.assigned_port, 12349);

        broker.release_by_id(&r1.id).await.unwrap();

        // Verify the released row still exists
        let released_row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM reservations WHERE id = ? AND state = 'released'")
                .bind(&r1.id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert!(released_row.is_some());

        // Reserve the same port with a different owner
        let r2 = broker
            .reserve("host:del2", Some(12349), "127.0.0.1", 9090, None, true)
            .await
            .unwrap();
        assert_eq!(r2.assigned_port, 12349);
        assert_ne!(r2.id, r1.id);

        // The old released row should be gone (deleted by reserve's transaction)
        let old_row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM reservations WHERE id = ?")
                .bind(&r1.id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert!(old_row.is_none());
    }
}
