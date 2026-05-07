//! `SQLite` pool with embedded migrations.
//!
//! Migrations live in `backend/migrations/` and are baked into the binary at
//! compile time via `sqlx::migrate!()`. The pool itself uses
//! `sqlx::sqlite::SqlitePool` over the configured `database_url`.

use anyhow::{Context as _, Result};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr as _;
use tracing::Level;
use tracing::event;

/// Embedded migration set under `backend/migrations/`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Opens (or creates) the `SQLite` database, applies all pending migrations,
/// and returns the live pool.
///
/// # Errors
///
/// Returns an error if the URL is malformed, the file cannot be created, or
/// a migration fails.
pub async fn init(database_url: &str) -> Result<SqlitePool> {
    // `mode=rwc` (the default in `Config::DEFAULT_DATABASE_URL`) lets SQLX
    // create the DB file on first run; production overrides set the path
    // inside the mounted PVC.
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid ANVIL_DATABASE_URL={database_url:?}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open SQLite at {database_url}"))?;

    MIGRATOR
        .run(&pool)
        .await
        .context("running embedded migrations")?;

    event!(
        name: "anvil.db.ready",
        Level::INFO,
        db.url = database_url,
        "database ready",
    );

    Ok(pool)
}
