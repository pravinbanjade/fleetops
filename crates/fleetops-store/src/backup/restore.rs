//! Restoring: a backup is opened and prepared in a staging directory, then
//! left at `<data>/restore-pending/` for the next start to swap in before
//! anything opens the database.

use super::{
    archive::{connect, tables},
    format::damaged,
    Manifest, SealedColumn,
};
use crate::{
    db_err,
    secrets::{SecretBox, SecretKey},
};
use fleetops_core::{Error, Result};
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Connection, SqliteConnection};
use std::path::{Path, PathBuf};

pub const PENDING_DIR: &str = "restore-pending";
pub const STAGING_DIR: &str = "restore-staging";
/// Scratch space for backups being made, uploaded or fetched.
pub const TMP_DIR: &str = "backup-tmp";
pub const PRE_RESTORE_DIR: &str = "pre-restore";
pub const PRE_RESTORE_KEEP: usize = 3;
/// Rejected pending databases kept for a look; older ones are removed.
pub const RESTORE_FAILED_KEEP: usize = 3;
const RESTORE_FAILED: &str = "restore-failed-";
const DB: &str = "fleetops.db";

pub struct ServerVersions {
    pub community_max: i64,
    /// `None` on a Community build, which can't judge Enterprise migrations.
    pub enterprise_max: Option<i64>,
    pub version: String,
}

fn io(e: std::io::Error) -> Error {
    Error::Internal(format!("restore: {e}"))
}

/// Refuses a backup made by a newer FleetOps than this one.
pub fn check_versions(m: &Manifest, v: &ServerVersions) -> Result<()> {
    let newer_ee = matches!(
        (m.enterprise_migration, v.enterprise_max),
        (Some(b), Some(s)) if b > s
    );
    if m.community_migration > v.community_max || newer_ee {
        let ver = &m.fleetops_version;
        return Err(Error::Invalid(format!(
            "this backup is from FleetOps {ver}; upgrade to at least {ver} to restore it"
        )));
    }
    Ok(())
}

/// A plain SQL identifier that names an existing column. The names come
/// from the backup's manifest, which whoever made the file controls.
fn ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The rows of a sealed column, after checking its names are plain and real.
pub(crate) async fn sealed_rows(
    c: &mut SqliteConnection,
    col: &SealedColumn,
) -> Result<Vec<(i64, String)>> {
    let (t, cname, id) = (&col.table, &col.column, &col.id_column);
    if ![t, cname, id].iter().all(|n| ident(n)) {
        return Err(damaged());
    }
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
        .bind(t)
        .fetch_all(&mut *c)
        .await
        .map_err(db_err)?;
    if !cols.contains(cname) || !cols.contains(id) {
        return Err(damaged());
    }
    sqlx::query_as(&format!(
        "SELECT \"{id}\", \"{cname}\" FROM \"{t}\" WHERE \"{cname}\" IS NOT NULL"
    ))
    .fetch_all(&mut *c)
    .await
    .map_err(db_err)
}

/// Opens every sealed value the manifest lists with the backup's key and
/// seals it again with `target`. All or nothing.
pub async fn reseal(db: &Path, m: &Manifest, target: &SecretKey) -> Result<u64> {
    let bad = || {
        Error::Invalid("the backup's secrets could not be opened; the file may be damaged".into())
    };
    let from = SecretBox::new(&SecretKey::from_base64(&m.secret_key).map_err(|_| bad())?);
    let to = SecretBox::new(target);
    let mut c = connect(db).await?;
    let present = tables(&mut c).await?;
    let mut tx = c.begin().await.map_err(db_err)?;
    let mut n = 0;
    for col in m.sealed.iter().filter(|s| present.contains(&s.table)) {
        let rows = sealed_rows(&mut tx, col).await?;
        let (t, cname, id) = (&col.table, &col.column, &col.id_column);
        for (row, sealed) in rows {
            let aad = col.aad_for(row);
            let plain = from.open(&aad, &sealed).map_err(|_| bad())?;
            sqlx::query(&format!(
                "UPDATE \"{t}\" SET \"{cname}\" = ? WHERE \"{id}\" = ?"
            ))
            .bind(to.seal(&aad, &plain))
            .bind(row)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            n += 1;
        }
    }
    tx.commit().await.map_err(db_err)?;
    c.close().await.map_err(db_err)?;
    Ok(n)
}

/// Folds any WAL into the file, then moves the prepared database to
/// `<data>/restore-pending/fleetops.db`.
pub async fn mark_pending(db: &Path, data: &Path) -> Result<()> {
    let mut c = connect(db).await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut c)
        .await
        .map_err(db_err)?;
    sqlx::query("PRAGMA journal_mode = DELETE")
        .execute(&mut c)
        .await
        .map_err(db_err)?;
    c.close().await.map_err(db_err)?;
    let dir = data.join(PENDING_DIR);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let tmp = dir.join("fleetops.db.tmp");
    if std::fs::rename(db, &tmp).is_err() {
        std::fs::copy(db, &tmp).map_err(io)?;
    }
    std::fs::rename(&tmp, dir.join(DB)).map_err(io)
}

async fn healthy(db: &Path) -> bool {
    let check = async {
        let mut c = SqliteConnectOptions::new()
            .filename(db)
            .read_only(true)
            .connect()
            .await
            .ok()?;
        let r: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&mut c)
            .await
            .ok()?;
        let _ = c.close().await;
        Some(r == "ok")
    };
    check.await.unwrap_or(false)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fresh_dir(parent: &Path, base: &str) -> PathBuf {
    let mut p = parent.join(base);
    let mut i = 1;
    while p.exists() {
        p = parent.join(format!("{base}-{i}"));
        i += 1;
    }
    p
}

/// Keeps the newest `keep` directories in `dir` whose names start with `prefix`.
fn prune(dir: &Path, prefix: &str, keep: usize) -> Result<()> {
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map_err(io)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix) && e.path().is_dir())
        .map(|e| {
            let t = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, e.path())
        })
        .collect();
    entries.sort();
    let extra = entries.len().saturating_sub(keep);
    for (_, p) in entries.into_iter().take(extra) {
        std::fs::remove_dir_all(p).map_err(io)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pending {
    /// Nothing was waiting.
    None,
    /// Restored; the previous data is in this directory.
    Applied(PathBuf),
    /// The pending database failed its check and was moved here; the
    /// current data was kept.
    Rejected(PathBuf),
}

/// Run at startup, before the store opens. Swaps a pending restore in,
/// keeping the current data in `<data>/pre-restore/<ts>/`.
pub async fn apply_pending(data: &Path) -> Result<Pending> {
    // Left behind by a request that was cut off, or a run the restart ended.
    for d in [STAGING_DIR, TMP_DIR] {
        let p = data.join(d);
        if p.exists() {
            std::fs::remove_dir_all(&p).map_err(io)?;
        }
    }
    let pending_dir = data.join(PENDING_DIR);
    let pending = pending_dir.join(DB);
    if !pending.exists() {
        if pending_dir.exists() {
            std::fs::remove_dir_all(&pending_dir).map_err(io)?;
        }
        return Ok(Pending::None);
    }
    let ts = now_secs();
    if !healthy(&pending).await {
        let failed = fresh_dir(data, &format!("{RESTORE_FAILED}{ts}"));
        std::fs::rename(&pending_dir, &failed).map_err(io)?;
        prune(data, RESTORE_FAILED, RESTORE_FAILED_KEEP)?;
        return Ok(Pending::Rejected(failed));
    }
    let root = data.join(PRE_RESTORE_DIR);
    std::fs::create_dir_all(&root).map_err(io)?;
    let safety = fresh_dir(&root, &ts.to_string());
    std::fs::create_dir_all(&safety).map_err(io)?;
    for name in [DB, "fleetops.db-wal", "fleetops.db-shm"] {
        let p = data.join(name);
        if p.exists() {
            std::fs::rename(&p, safety.join(name)).map_err(io)?;
        }
    }
    let key = data.join("secret.key");
    if key.exists() {
        std::fs::copy(&key, safety.join("secret.key")).map_err(io)?;
    }
    std::fs::rename(&pending, data.join(DB)).map_err(io)?;
    std::fs::remove_dir_all(&pending_dir).map_err(io)?;
    prune(&root, "", PRE_RESTORE_KEEP)?;
    Ok(Pending::Applied(safety))
}
