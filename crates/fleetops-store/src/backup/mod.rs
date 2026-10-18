//! Backup and restore (backup spec): one passphrase-sealed file holding a
//! snapshot of the database and what is needed to open its secrets.

pub mod archive;
pub mod format;
pub mod restore;

pub use archive::{open_backup, BackupMeta, BackupSummary, Manifest, Opened, EXCLUDED_TABLES};
pub use format::{check_passphrase, KdfParams};
pub use restore::{apply_pending, check_versions, mark_pending, reseal, Pending, ServerVersions};

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

/// `fleetops-backup-<UTC yyyymmdd-hhmmss>.fob`.
pub fn backup_name(now: i64) -> String {
    let t = jiff::Timestamp::from_second(now).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    format!("fleetops-backup-{}.fob", t.strftime("%Y%m%d-%H%M%S"))
}

/// The time in a name [`backup_name`] made; `None` for any other name.
pub fn parse_name(name: &str) -> Option<i64> {
    let stamp = name
        .strip_prefix("fleetops-backup-")?
        .strip_suffix(".fob")?;
    let b = stamp.as_bytes();
    if b.len() != 15
        || b[8] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
    {
        return None;
    }
    let dt = jiff::civil::DateTime::strptime("%Y%m%d-%H%M%S", stamp).ok()?;
    Some(
        dt.to_zoned(jiff::tz::TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_second(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_round_trip() {
        let n = backup_name(1_700_000_000);
        assert_eq!(n, "fleetops-backup-20231114-221320.fob");
        assert_eq!(parse_name(&n), Some(1_700_000_000));
        for bad in [
            "fleetops-backup-2023.fob",
            "my.fob",
            "fleetops-backup-20231114-221320.fob.part",
            "fleetops-backup-2023111a-221320.fob",
        ] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }
}
