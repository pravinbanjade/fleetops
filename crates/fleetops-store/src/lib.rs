//! SQLite persistence for fleetops. All timestamps are unix seconds passed
//! in by the caller so that behaviour is deterministic under test.

mod access;
mod agents;
mod audit;
pub mod backup;
mod credentials;
mod environments;
pub mod secrets;
mod sessions;
mod settings;
mod stack_files;
mod stacks;
mod stacks_git;
mod swarm;
mod users;

pub use access::BindingRecord;
pub use agents::AgentRecord;
pub use audit::{AuditEntry, NewAudit};
pub use environments::EnvironmentRecord;
pub use sessions::{NewSession, SessionRecord};
pub use settings::AUDIT_RETENTION_KEY;
pub use stack_files::{RevisionFile, StoredFile};
pub use stacks::{Builds, BuiltImage, Draft, RevisionRecord, StackRecord, STACK_REVISIONS_KEPT};
pub use swarm::{KeyMatch, SwarmAgentRecord, SwarmCluster};
pub use stacks_git::{GitRecord, GitSettings, KeyChange, TokenChange};

use fleetops_core::{Error, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::{path::Path, str::FromStr, time::Duration};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    /// Seals credentials; a random per-process key until `with_secret_key`.
    secrets: std::sync::Arc<secrets::SecretBox>,
    /// The key behind `secrets`, which a backup carries.
    key: std::sync::Arc<secrets::SecretKey>,
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
        // An Enterprise build adds its own migrations (versions 1000000+) to
        // the same table; a Community binary must still start on that
        // database, so applied versions it does not know are ignored.
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.set_ignore_missing(true);
        migrator
            .run(&pool)
            .await
            .map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
        let key = secrets::SecretKey::random();
        Ok(Self {
            pool,
            secrets: std::sync::Arc::new(secrets::SecretBox::new(&key)),
            key: std::sync::Arc::new(key),
        })
    }

    /// The connection pool, for the Enterprise crate's own tables.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The box that seals stored secrets.
    pub fn secrets(&self) -> &secrets::SecretBox {
        &self.secrets
    }

    /// Seals and opens credentials with `key` instead of the random
    /// per-process key every store starts with.
    pub fn with_secret_key(mut self, key: &secrets::SecretKey) -> Self {
        self.secrets = std::sync::Arc::new(secrets::SecretBox::new(key));
        self.key = std::sync::Arc::new(key.clone());
        self
    }

    /// The key that seals this store's secrets.
    pub fn secret_key(&self) -> &secrets::SecretKey {
        &self.key
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
