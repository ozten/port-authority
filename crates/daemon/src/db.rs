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
        .max_connections(1) // Single-writer for SQLite
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open database: {}", db_path.display()))?;

    // Run migrations
    run_migrations(&pool).await?;

    info!(path = %db_path.display(), "database initialized");
    Ok(pool)
}

async fn run_migrations(pool: &SqlitePool) -> anyhow::Result<()> {
    let migration_sql = include_str!("../../../migrations/001_initial.sql");
    sqlx::raw_sql(migration_sql)
        .execute(pool)
        .await
        .context("failed to run migrations")?;
    Ok(())
}
