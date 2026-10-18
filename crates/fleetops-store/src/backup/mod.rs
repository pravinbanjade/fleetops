//! Backup and restore (backup spec): one passphrase-sealed file holding a
//! snapshot of the database and what is needed to open its secrets.

use serde::{Deserialize, Serialize};

/// Enterprise migrations start at this version, so a database's highest
/// Community and Enterprise migrations can be told apart.
pub const EE_MIGRATION_BASE: i64 = 1_000_000;

/// A column holding values sealed with the server key. Backups list these
/// as data, so a server without the Enterprise code still re-seals
/// Enterprise secrets on restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedColumn {
    pub table: String,
    pub column: String,
    pub id_column: String,
    /// The AAD, with `{id}` standing for the row id.
    pub aad: String,
}

impl SealedColumn {
    pub fn new(table: &str, column: &str, id_column: &str, aad: &str) -> Self {
        Self {
            table: table.into(),
            column: column.into(),
            id_column: id_column.into(),
            aad: aad.into(),
        }
    }

    pub fn aad_for(&self, id: i64) -> Vec<u8> {
        self.aad.replace("{id}", &id.to_string()).into_bytes()
    }
}

/// The Community sealed columns. Agent credentials are hashes, not sealed.
pub fn community_sealed() -> Vec<SealedColumn> {
    vec![SealedColumn::new(
        "env_credentials",
        "ciphertext",
        "env_id",
        "{id}",
    )]
}

/// The newest Community migration this build knows.
pub fn community_migration_max() -> i64 {
    sqlx::migrate!("./migrations")
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}
