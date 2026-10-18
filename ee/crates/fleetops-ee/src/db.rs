//! The Enterprise tables, in the same SQLite database as the Community ones.

use fleetops_core::{Error, Result};
use sqlx::SqlitePool;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

/// Applies the Enterprise migrations; Community versions in the same table
/// are ignored, as the Community migrator ignores these.
pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_ignore_missing(true);
    migrator
        .run(pool)
        .await
        .map_err(|e| Error::Internal(format!("enterprise migration failed: {e}")))
}

/// The newest Enterprise migration this build knows.
pub fn migration_max() -> i64 {
    sqlx::migrate!("./migrations")
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}

pub async fn get(pool: &SqlitePool) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT raw FROM ee_license WHERE id = 1")
        .fetch_optional(pool)
        .await
        .map_err(db_err)
}

pub async fn put(pool: &SqlitePool, raw: &str, now: i64, by: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO ee_license (id, raw, uploaded_at, uploaded_by) VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET raw = excluded.raw,
             uploaded_at = excluded.uploaded_at, uploaded_by = excluded.uploaded_by",
    )
    .bind(raw)
    .bind(now)
    .bind(by)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Whether a stored license was removed.
pub async fn delete(pool: &SqlitePool) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_license WHERE id = 1")
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}
