use port_authority_core::error::PortError;
use port_authority_core::types::ReservationState;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::net::TcpListener as StdTcpListener;
use tracing::{debug, info};
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

/// The port broker — manages reservations and port allocation.
/// All methods are &mut self; external synchronization is via tokio::sync::Mutex.
pub struct Broker {
    pool: SqlitePool,
    port_range: (u16, u16),
    /// Hold listeners for ports that are reserved but not yet tunneled.
    hold_listeners: HashMap<u16, StdTcpListener>,
}

impl Broker {
    pub fn new(pool: SqlitePool, port_range_start: u16, port_range_end: u16) -> Self {
        Self {
            pool,
            port_range: (port_range_start, port_range_end),
            hold_listeners: HashMap::new(),
        }
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

        // Fetch and return the full record
        self.get_reservation_by_id(&id).await
    }

    /// Update the state of a reservation.
    pub async fn update_state(
        &self,
        reservation_id: &str,
        new_state: ReservationState,
    ) -> Result<(), PortError> {
        sqlx::query("UPDATE reservations SET state = ? WHERE id = ?")
            .bind(new_state.as_sql())
            .bind(reservation_id)
            .execute(&self.pool)
            .await
            .map_err(|e| PortError::Database(e.to_string()))?;
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
                let pattern = format!("{}%", owner);
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE owner LIKE ? AND state = ? ORDER BY assigned_port",
                    SELECT_COLS
                ))
                .bind(pattern)
                .bind(state)
                .fetch_all(&self.pool)
                .await
            }
            (Some(owner), None) => {
                let pattern = format!("{}%", owner);
                sqlx::query(&format!(
                    "SELECT {} FROM reservations WHERE owner LIKE ? AND state != 'released' ORDER BY assigned_port",
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
}
