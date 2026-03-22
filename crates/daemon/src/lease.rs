use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::config::LeaseConfig;

#[derive(Debug, Clone)]
pub struct ExpiredReservation {
    pub id: String,
    pub assigned_port: u16,
}

pub struct LeaseCleanup {
    pool: SqlitePool,
    config: LeaseConfig,
}

impl LeaseCleanup {
    pub fn new(pool: SqlitePool, config: LeaseConfig) -> Self {
        Self { pool, config }
    }

    pub async fn run(
        self,
        expired_tx: mpsc::UnboundedSender<Vec<ExpiredReservation>>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.cleanup_interval_secs));

        info!(
            interval_secs = self.config.cleanup_interval_secs,
            released_ttl_secs = self.config.released_ttl_secs,
            "lease cleanup task started"
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("lease cleanup task shutting down");
                        return;
                    }
                }
            }

            if *shutdown.borrow() {
                info!("lease cleanup task shutting down");
                return;
            }

            // Step 1: Find reservations with expired leases
            let expired = match self.expire_leases().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "failed to expire leases");
                    continue;
                }
            };

            if !expired.is_empty() {
                info!(count = expired.len(), "expired lease reservations moved to released");
                if let Err(e) = expired_tx.send(expired) {
                    warn!(error = %e, "failed to send expired reservations to channel");
                }
            } else {
                debug!("no expired leases found");
            }

            // Step 2: Purge old released reservations
            match self.purge_released().await {
                Ok(count) if count > 0 => {
                    info!(count, "purged old released reservations");
                }
                Ok(_) => {
                    debug!("no released reservations to purge");
                }
                Err(e) => {
                    warn!(error = %e, "failed to purge released reservations");
                }
            }
        }
    }

    async fn expire_leases(&self) -> Result<Vec<ExpiredReservation>, sqlx::Error> {
        // SELECT first, then UPDATE — SQLite may not support RETURNING on older versions.
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT id, assigned_port FROM reservations \
             WHERE state IN ('active', 'pending') \
             AND expires_at IS NOT NULL \
             AND expires_at <= datetime('now')",
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(Vec::new());
        }

        sqlx::query(
            "UPDATE reservations SET state = 'released' \
             WHERE state IN ('active', 'pending') \
             AND expires_at IS NOT NULL \
             AND expires_at <= datetime('now')",
        )
        .execute(&self.pool)
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

    async fn purge_released(&self) -> Result<u64, sqlx::Error> {
        let ttl = self.config.released_ttl_secs as i64;

        let result = sqlx::query(
            "DELETE FROM reservations \
             WHERE state = 'released' \
             AND updated_at <= datetime('now', '-' || CAST(? AS TEXT) || ' seconds')",
        )
        .bind(ttl)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}
