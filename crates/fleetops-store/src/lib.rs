//! SQLite persistence for fleetops. All timestamps are unix seconds passed
//! in by the caller so that behaviour is deterministic under test.

mod agents;
mod audit;
mod credentials;
mod environments;
pub mod secrets;
mod sessions;
mod settings;
mod users;

pub use agents::AgentRecord;
pub use audit::{AuditEntry, NewAudit};
pub use environments::EnvironmentRecord;
pub use sessions::{NewSession, SessionRecord};
pub use settings::AUDIT_RETENTION_KEY;

use fleetops_core::{Error, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::{path::Path, str::FromStr, time::Duration};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    /// Seals credentials; a random per-process key until `with_secret_key`.
    secrets: std::sync::Arc<secrets::SecretBox>,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::init(pool).await
    }

    /// Single-connection in-memory database for tests. The connection must
    /// never be recycled or the database disappears.
    pub async fn in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(db_err)?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::init(pool).await
    }

    async fn init(pool: SqlitePool) -> Result<Self> {
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
        Ok(Self {
            pool,
            secrets: std::sync::Arc::new(secrets::SecretBox::new(&secrets::SecretKey::random())),
        })
    }

    /// Seals and opens credentials with `key` instead of the random
    /// per-process key every store starts with.
    pub fn with_secret_key(mut self, key: &secrets::SecretKey) -> Self {
        self.secrets = std::sync::Arc::new(secrets::SecretBox::new(key));
        self
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::RowNotFound => Error::NotFound("record not found".into()),
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict("already exists".into())
        }
        _ => Error::Internal(format!("database error: {e}")),
    }
}
