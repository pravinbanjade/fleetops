//! What a Community backup needs to know about the Enterprise tables.

use async_trait::async_trait;
use fleetops_api::backup::BackupHooks;
use fleetops_core::Result;
use fleetops_store::backup::SealedColumn;
use sqlx::SqlitePool;

pub struct EeBackupHooks;

/// Every Enterprise column sealed with the server key.
pub fn sealed() -> Vec<SealedColumn> {
    vec![
        SealedColumn::new("ee_sso_connections", "secrets", "id", "sso:{id}"),
        SealedColumn::new("ee_backup_schedule", "passphrase", "id", "backup:{id}"),
        SealedColumn::new("ee_backup_targets", "secrets", "id", "backup-target:{id}"),
    ]
}

#[async_trait]
impl BackupHooks for EeBackupHooks {
    fn sealed(&self) -> Vec<SealedColumn> {
        sealed()
    }
    fn enterprise_migration_max(&self) -> Option<i64> {
        Some(crate::db::migration_max())
    }
    async fn migrate(&self, pool: &SqlitePool) -> Result<()> {
        crate::db::migrate(pool).await
    }
}
