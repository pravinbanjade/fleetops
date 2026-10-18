//! Backup and restore plumbing shared by the routes and the Enterprise
//! crate: the extension points, and the backups uploaded and prepared but
//! not yet restored.

use crate::AppState;
use async_trait::async_trait;
use fleetops_core::{Error, Result};
use fleetops_store::{
    backup::{self, BackupSummary, Manifest, SealedColumn, ServerVersions},
    Store,
};
use rand::RngCore;
use serde::Serialize;
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use utoipa::ToSchema;

/// What a backup and a restore need from the Enterprise tables; nothing in
/// Community.
#[async_trait]
pub trait BackupHooks: Send + Sync {
    /// The Enterprise sealed columns.
    fn sealed(&self) -> Vec<SealedColumn>;
    /// The newest Enterprise migration this build knows.
    fn enterprise_migration_max(&self) -> Option<i64>;
    /// Brings a restored database's Enterprise tables up to date.
    async fn migrate(&self, pool: &SqlitePool) -> Result<()>;
}

pub struct NoBackupHooks;

#[async_trait]
impl BackupHooks for NoBackupHooks {
    fn sealed(&self) -> Vec<SealedColumn> {
        vec![]
    }
    fn enterprise_migration_max(&self) -> Option<i64> {
        None
    }
    async fn migrate(&self, _: &SqlitePool) -> Result<()> {
        Ok(())
    }
}

/// Starts the server again so a pending restore is swapped in.
pub trait Restart: Send + Sync {
    fn restart(&self);
}

pub struct NoRestart;

impl Restart for NoRestart {
    fn restart(&self) {
        tracing::warn!("restart FleetOps to finish the restore");
    }
}

/// How long an uploaded backup waits for its restore.
pub const STAGE_TTL: Duration = Duration::from_secs(15 * 60);
/// Largest backup upload.
pub const BACKUP_UPLOAD_MAX: usize = 512 * 1024 * 1024;
pub use fleetops_store::backup::restore::TMP_DIR;
/// An unpacked database may be at most this many times its upload, or
/// [`MIN_DB_LIMIT`]: a small file that inflates to gigabytes is refused.
pub const DB_RATIO: u64 = 16;
pub const MIN_DB_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Serialize, ToSchema)]
pub struct StagedBackup {
    /// Pass to the restore call.
    pub id: String,
    pub summary: BackupSummary,
}

struct Entry {
    dir: PathBuf,
    db: PathBuf,
    manifest: Manifest,
    setup: bool,
    expires: Instant,
}

pub struct StagedBackups {
    entries: Mutex<HashMap<String, Entry>>,
    applying: AtomicBool,
    /// First-run uploads are unauthenticated: one at a time.
    setup_slot: Arc<tokio::sync::Semaphore>,
}

impl Default for StagedBackups {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            applying: AtomicBool::default(),
            setup_slot: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

pub fn data_dir(s: &AppState) -> Result<&Path> {
    s.config
        .data_dir
        .as_deref()
        .ok_or_else(|| Error::Internal("no data directory".into()))
}

/// A fresh file path in `<data>/backup-tmp/`.
pub fn tmp_file(s: &AppState, ext: &str) -> Result<PathBuf> {
    let dir = data_dir(s)?.join(TMP_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Internal(format!("backup: {e}")))?;
    Ok(dir.join(format!("{}.{ext}", random_id())))
}

fn random_id() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Every sealed column this server knows.
pub fn sealed_columns(s: &AppState) -> Vec<SealedColumn> {
    let mut v = backup::community_sealed();
    v.extend(s.backup_hooks.sealed());
    v
}

impl StagedBackups {
    /// Held while a first-run upload is read and opened.
    pub fn setup_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.setup_slot.clone().try_acquire_owned().map_err(|_| {
            Error::Conflict("another backup is being opened; try again when it finishes".into())
        })
    }

    /// Decrypts `file` into `<data>/restore-staging/<id>/` and prepares it:
    /// version check, re-seal with this server's key, migrations.
    pub async fn stage(
        &self,
        s: &AppState,
        file: &Path,
        passphrase: &str,
        setup: bool,
    ) -> Result<StagedBackup> {
        self.sweep().await;
        let id = random_id();
        let dir = data_dir(s)?.join(backup::restore::STAGING_DIR).join(&id);
        match prepare(s, file, passphrase, &dir).await {
            Ok((manifest, db)) => {
                let summary = manifest.summary();
                let mut map = self.entries.lock().await;
                if setup {
                    // A new first-run upload replaces the last one.
                    map.retain(|_, e| {
                        if e.setup {
                            let _ = std::fs::remove_dir_all(&e.dir);
                        }
                        !e.setup
                    });
                }
                map.insert(
                    id.clone(),
                    Entry {
                        dir,
                        db,
                        manifest,
                        setup,
                        expires: Instant::now() + STAGE_TTL,
                    },
                );
                Ok(StagedBackup { id, summary })
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// Removes and returns a staged backup made the same way (`setup`).
    async fn take(&self, id: &str, setup: bool) -> Result<Entry> {
        let mut map = self.entries.lock().await;
        match map.get(id) {
            Some(e) if e.setup == setup && e.expires > Instant::now() => {
                Ok(map.remove(id).expect("present"))
            }
            _ => Err(Error::NotFound(
                "that backup is no longer staged; upload it again".into(),
            )),
        }
    }

    pub async fn discard(&self, id: &str) {
        if let Some(e) = self.entries.lock().await.remove(id) {
            let _ = std::fs::remove_dir_all(e.dir);
        }
    }

    async fn sweep(&self) {
        let now = Instant::now();
        self.entries.lock().await.retain(|_, e| {
            let keep = e.expires > now;
            if !keep {
                let _ = std::fs::remove_dir_all(&e.dir);
            }
            keep
        });
    }

    /// Leaves the staged backup `id` for the next start to swap in, then
    /// restarts. `by` is who asked, for the audit row in the restored data.
    pub async fn apply(&self, s: &AppState, id: &str, setup: bool, by: Option<&str>) -> Result<()> {
        if self.applying.swap(true, Ordering::SeqCst) {
            return Err(Error::Conflict("a restore is already in progress".into()));
        }
        let r = self.apply_inner(s, id, setup, by).await;
        if r.is_err() {
            self.applying.store(false, Ordering::SeqCst);
        }
        r
    }

    async fn apply_inner(
        &self,
        s: &AppState,
        id: &str,
        setup: bool,
        by: Option<&str>,
    ) -> Result<()> {
        let e = self.take(id, setup).await?;
        let r = async {
            let store = Store::open(&e.db).await?;
            store
                .insert_audit(&fleetops_store::NewAudit {
                    ts: fleetops_core::now_unix(),
                    // The restored users are not this server's users.
                    user_id: None,
                    environment_id: None,
                    action: "backup.restore",
                    target_type: Some("backup"),
                    target_id: None,
                    result: "success",
                    detail: Some(serde_json::json!({
                        "by": by,
                        "source_version": e.manifest.fleetops_version,
                        "created_at": e.manifest.created_at,
                    })),
                })
                .await?;
            store.pool().close().await;
            let data = data_dir(s)?;
            let marked = backup::mark_pending(&e.db, data).await;
            if marked.is_err() {
                let _ = std::fs::remove_dir_all(data.join(backup::restore::PENDING_DIR));
            }
            marked
        }
        .await;
        let _ = std::fs::remove_dir_all(&e.dir);
        r?;
        let restart = s.restart.clone();
        tokio::spawn(async move {
            // Let the response reach the browser first.
            tokio::time::sleep(Duration::from_millis(500)).await;
            restart.restart();
        });
        Ok(())
    }
}

async fn prepare(
    s: &AppState,
    file: &Path,
    passphrase: &str,
    dir: &Path,
) -> Result<(Manifest, PathBuf)> {
    let (file, pw, out) = (
        file.to_path_buf(),
        passphrase.to_string(),
        dir.to_path_buf(),
    );
    let opened = tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&file).map_err(|e| Error::Internal(format!("backup: {e}")))?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let max_db = len.saturating_mul(DB_RATIO).max(MIN_DB_LIMIT);
        backup::open_backup_limited(std::io::BufReader::new(f), &pw, &out, max_db)
    })
    .await
    .map_err(|e| Error::Internal(format!("backup: {e}")))??;
    backup::check_versions(
        &opened.manifest,
        &ServerVersions {
            community_max: backup::community_migration_max(),
            enterprise_max: s.backup_hooks.enterprise_migration_max(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )?;
    backup::reseal(&opened.db, &opened.manifest, s.store.secret_key()).await?;
    let store = Store::open(&opened.db).await?;
    s.backup_hooks.migrate(store.pool()).await?;
    store.pool().close().await;
    Ok((opened.manifest, opened.db))
}
