use anyhow::Context;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;
use tracing::info;

/// Initialize the SQLite database pool with WAL mode and pragmas.
pub async fn init_pool(db_path: &Path) -> anyhow::Result<SqlitePool> {
    // Ensure parent directory exists
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
        // Single-writer — REQUIRED for correctness, not just performance.
        // Broker and LeaseCleanup share this pool without application-level
        // locking across their operations. Increasing max_connections breaks
        // transaction isolation assumptions between these two actors.
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open database: {}", db_path.display()))?;

    // Run migrations — sqlx tracks applied versions in _sqlx_migrations,
    // checksums each file, and wraps each migration in a transaction.
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    info!(path = %db_path.display(), "database initialized");
    Ok(pool)
}
