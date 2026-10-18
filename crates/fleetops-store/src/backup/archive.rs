//! What is inside the sealed file: a tar.gz of `manifest.json` and
//! `fleetops.db`, a consistent snapshot taken with `VACUUM INTO`.

use super::{
    format::{self, damaged, KdfParams},
    SealedColumn, EE_MIGRATION_BASE,
};
use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Connection, SqliteConnection};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, BufWriter, Read},
    path::{Path, PathBuf},
};
use utoipa::ToSchema;

pub const FORMAT: u32 = 1;
/// Short-lived rows a restore has no use for.
pub const EXCLUDED_TABLES: &[&str] = &["sessions", "ee_sso_states"];
const MANIFEST: &str = "manifest.json";
const DB: &str = "fleetops.db";
const MAX_MANIFEST: u64 = 1024 * 1024;
const MAX_TOTAL: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub fleetops_version: String,
    pub created_at: i64,
    pub edition: String,
    pub source: Option<String>,
    pub community_migration: i64,
    pub enterprise_migration: Option<i64>,
    pub counts: BTreeMap<String, i64>,
    /// The source server's key, base64; never shown.
    pub secret_key: String,
    pub sealed: Vec<SealedColumn>,
}

/// What an admin is shown before restoring: the manifest without secrets.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BackupSummary {
    pub fleetops_version: String,
    pub created_at: i64,
    /// `community` or `enterprise`.
    pub edition: String,
    pub source: Option<String>,
    /// Rows per table.
    pub counts: BTreeMap<String, i64>,
    /// The backup holds Enterprise tables.
    pub has_enterprise_data: bool,
}

impl Manifest {
    pub fn summary(&self) -> BackupSummary {
        BackupSummary {
            fleetops_version: self.fleetops_version.clone(),
            created_at: self.created_at,
            edition: self.edition.clone(),
            source: self.source.clone(),
            counts: self.counts.clone(),
            has_enterprise_data: self.enterprise_migration.is_some(),
        }
    }
}

pub struct BackupMeta {
    pub now: i64,
    pub edition: String,
    pub source: Option<String>,
    /// Every sealed column this server knows, Community and Enterprise.
    pub sealed: Vec<SealedColumn>,
}

#[derive(Debug)]
pub struct Opened {
    pub manifest: Manifest,
    pub db: PathBuf,
}

fn io(e: std::io::Error) -> Error {
    Error::Internal(format!("backup: {e}"))
}

fn rand_name(prefix: &str, ext: &str) -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{prefix}-{hex}.{ext}")
}

pub(crate) async fn connect(path: &Path) -> Result<SqliteConnection> {
    SqliteConnectOptions::new()
        .filename(path)
        .connect()
        .await
        .map_err(db_err)
}

pub(crate) async fn tables(c: &mut SqliteConnection) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table'
         AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(&mut *c)
    .await
    .map_err(db_err)
}

/// Removes the excluded rows and reads the counts and migration levels.
async fn prepare_snapshot(path: &Path) -> Result<(BTreeMap<String, i64>, i64, Option<i64>)> {
    let mut c = connect(path).await?;
    let names = tables(&mut c).await?;
    for t in EXCLUDED_TABLES {
        if names.iter().any(|n| n == t) {
            sqlx::query(&format!("DELETE FROM \"{t}\""))
                .execute(&mut c)
                .await
                .map_err(db_err)?;
        }
    }
    sqlx::query("VACUUM")
        .execute(&mut c)
        .await
        .map_err(db_err)?;
    let mut counts = BTreeMap::new();
    for t in names
        .iter()
        .filter(|n| !EXCLUDED_TABLES.contains(&n.as_str()))
    {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{t}\""))
            .fetch_one(&mut c)
            .await
            .map_err(db_err)?;
        counts.insert(t.clone(), n);
    }
    let ce: Option<i64> =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE version < ?")
            .bind(EE_MIGRATION_BASE)
            .fetch_one(&mut c)
            .await
            .map_err(db_err)?;
    let ee: Option<i64> =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE version >= ?")
            .bind(EE_MIGRATION_BASE)
            .fetch_one(&mut c)
            .await
            .map_err(db_err)?;
    c.close().await.map_err(db_err)?;
    Ok((counts, ce.unwrap_or(0), ee))
}

fn pack(manifest: &Manifest, db: &Path, tgz: &Path) -> Result<()> {
    let gz = flate2::write::GzEncoder::new(
        BufWriter::new(File::create(tgz).map_err(io)?),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(gz);
    let json = serde_json::to_vec(manifest).map_err(|e| Error::Internal(e.to_string()))?;
    let mut h = tar::Header::new_gnu();
    h.set_size(json.len() as u64);
    h.set_mode(0o600);
    h.set_cksum();
    tar.append_data(&mut h, MANIFEST, json.as_slice())
        .map_err(io)?;
    let mut f = File::open(db).map_err(io)?;
    let mut h = tar::Header::new_gnu();
    h.set_size(f.metadata().map_err(io)?.len());
    h.set_mode(0o600);
    h.set_cksum();
    tar.append_data(&mut h, DB, &mut f).map_err(io)?;
    tar.into_inner().map_err(io)?.finish().map_err(io)?;
    Ok(())
}

impl Store {
    /// Writes an encrypted backup to the file `out`; `scratch` holds the
    /// snapshot and the unsealed archive while it is made.
    pub async fn write_backup(
        &self,
        meta: BackupMeta,
        passphrase: &str,
        params: KdfParams,
        scratch: &Path,
        out: &Path,
    ) -> Result<Manifest> {
        format::check_passphrase(passphrase)?;
        std::fs::create_dir_all(scratch).map_err(io)?;
        let snap = scratch.join(rand_name("snap", "db"));
        let tgz = scratch.join(rand_name("pack", "tgz"));
        let result = self
            .write_backup_inner(meta, passphrase, params, &snap, &tgz, out)
            .await;
        let _ = std::fs::remove_file(&snap);
        let _ = std::fs::remove_file(&tgz);
        result
    }

    async fn write_backup_inner(
        &self,
        meta: BackupMeta,
        passphrase: &str,
        params: KdfParams,
        snap: &Path,
        tgz: &Path,
        out: &Path,
    ) -> Result<Manifest> {
        sqlx::query("VACUUM INTO ?")
            .bind(snap.to_string_lossy().into_owned())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        let (counts, ce, ee) = prepare_snapshot(snap).await?;
        let manifest = Manifest {
            format: FORMAT,
            fleetops_version: env!("CARGO_PKG_VERSION").into(),
            created_at: meta.now,
            edition: meta.edition,
            source: meta.source,
            community_migration: ce,
            enterprise_migration: ee,
            counts,
            secret_key: self.key.to_base64(),
            sealed: meta.sealed,
        };
        let (m, snap, tgz, out, pw) = (
            manifest.clone(),
            snap.to_path_buf(),
            tgz.to_path_buf(),
            out.to_path_buf(),
            passphrase.to_string(),
        );
        tokio::task::spawn_blocking(move || -> Result<()> {
            pack(&m, &snap, &tgz)?;
            let input = BufReader::new(File::open(&tgz).map_err(io)?);
            let output = BufWriter::new(File::create(&out).map_err(io)?);
            format::encrypt(input, output, &pw, params)
        })
        .await
        .map_err(|e| Error::Internal(format!("backup: {e}")))??;
        Ok(manifest)
    }
}

/// Decrypts and unpacks a backup into `dir` (created; it must be empty).
pub fn open_backup(input: impl Read, passphrase: &str, dir: &Path) -> Result<Opened> {
    std::fs::create_dir_all(dir).map_err(io)?;
    if std::fs::read_dir(dir).map_err(io)?.next().is_some() {
        return Err(Error::Internal(
            "backup staging directory is not empty".into(),
        ));
    }
    let tgz = dir.join("payload.tgz");
    let result = (|| {
        format::decrypt(
            input,
            BufWriter::new(File::create(&tgz).map_err(io)?),
            passphrase,
        )?;
        unpack(&tgz, dir)
    })();
    let _ = std::fs::remove_file(&tgz);
    result
}

fn unpack(tgz: &Path, dir: &Path) -> Result<Opened> {
    let gz = flate2::read::GzDecoder::new(BufReader::new(File::open(tgz).map_err(io)?));
    let mut archive = tar::Archive::new(gz);
    let mut manifest: Option<Manifest> = None;
    let mut db: Option<PathBuf> = None;
    let mut total = 0u64;
    for entry in archive.entries().map_err(|_| damaged())? {
        let entry = entry.map_err(|_| damaged())?;
        if !entry.header().entry_type().is_file() {
            return Err(damaged());
        }
        let path = entry.path().map_err(|_| damaged())?.into_owned();
        let name = path.to_str().ok_or_else(damaged)?;
        match (name, manifest.is_some(), db.is_some()) {
            (MANIFEST, false, false) => {
                let mut buf = Vec::new();
                entry
                    .take(MAX_MANIFEST + 1)
                    .read_to_end(&mut buf)
                    .map_err(|_| damaged())?;
                if buf.len() as u64 > MAX_MANIFEST {
                    return Err(damaged());
                }
                total += buf.len() as u64;
                let m: Manifest = serde_json::from_slice(&buf).map_err(|_| damaged())?;
                if m.format != FORMAT {
                    return Err(Error::Invalid(
                        "this backup format is not supported by this FleetOps".into(),
                    ));
                }
                manifest = Some(m);
            }
            (DB, true, false) => {
                let out = dir.join(DB);
                let mut f = BufWriter::new(File::create(&out).map_err(io)?);
                let limit = MAX_TOTAL - total;
                let n = std::io::copy(&mut entry.take(limit + 1), &mut f).map_err(|_| damaged())?;
                if n > limit {
                    return Err(damaged());
                }
                db = Some(out);
            }
            _ => return Err(damaged()),
        }
    }
    match (manifest, db) {
        (Some(manifest), Some(db)) => Ok(Opened { manifest, db }),
        _ => Err(damaged()),
    }
}
