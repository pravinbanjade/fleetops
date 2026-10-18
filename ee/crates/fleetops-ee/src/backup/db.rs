//! The backup tables (`1000004_backup.sql`).

use fleetops_core::{Error, Result};
use fleetops_store::Store;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use utoipa::ToSchema;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

/// How many backups each target keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Retention {
    /// The newest N.
    Count(u32),
    /// Those made in the last N days.
    Days(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Schedule {
    pub enabled: bool,
    /// 0–23, UTC.
    pub hour_utc: u8,
    pub retention: Retention,
    pub passphrase_set: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            hour_utc: 2,
            retention: Retention::Count(7),
            passphrase_set: false,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ScheduleRow {
    enabled: bool,
    hour_utc: i64,
    retention_kind: String,
    retention_value: i64,
    passphrase_set: bool,
}

pub async fn get_schedule(pool: &SqlitePool) -> Result<Schedule> {
    let row: Option<ScheduleRow> = sqlx::query_as(
        "SELECT enabled, hour_utc, retention_kind, retention_value,
                passphrase IS NOT NULL AS passphrase_set
         FROM ee_backup_schedule WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(match row {
        None => Schedule::default(),
        Some(r) => Schedule {
            enabled: r.enabled,
            hour_utc: r.hour_utc as u8,
            retention: match r.retention_kind.as_str() {
                "days" => Retention::Days(r.retention_value as u32),
                _ => Retention::Count(r.retention_value as u32),
            },
            passphrase_set: r.passphrase_set,
        },
    })
}

/// Saves everything but the passphrase.
pub async fn put_schedule(pool: &SqlitePool, s: &Schedule, now: i64) -> Result<()> {
    let (kind, value) = match s.retention {
        Retention::Count(n) => ("count", n),
        Retention::Days(n) => ("days", n),
    };
    sqlx::query(
        "INSERT INTO ee_backup_schedule (id, enabled, hour_utc, retention_kind, retention_value, updated_at)
         VALUES (1, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, hour_utc = excluded.hour_utc,
             retention_kind = excluded.retention_kind, retention_value = excluded.retention_value,
             updated_at = excluded.updated_at",
    )
    .bind(s.enabled)
    .bind(s.hour_utc as i64)
    .bind(kind)
    .bind(value as i64)
    .bind(now)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

const PASSPHRASE_AAD: &[u8] = b"backup:1";

pub async fn set_passphrase(store: &Store, passphrase: &str, now: i64) -> Result<()> {
    let sealed = store.secrets().seal(PASSPHRASE_AAD, passphrase.as_bytes());
    sqlx::query(
        "INSERT INTO ee_backup_schedule (id, passphrase, updated_at) VALUES (1, ?, ?)
         ON CONFLICT (id) DO UPDATE SET passphrase = excluded.passphrase, updated_at = excluded.updated_at",
    )
    .bind(sealed)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn passphrase(store: &Store) -> Result<Option<String>> {
    let sealed: Option<Option<String>> =
        sqlx::query_scalar("SELECT passphrase FROM ee_backup_schedule WHERE id = 1")
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    match sealed.flatten() {
        None => Ok(None),
        Some(s) => {
            let plain = store.secrets().open(PASSPHRASE_AAD, &s).map_err(|_| {
                Error::Invalid("the saved backup passphrase can't be read; set it again".into())
            })?;
            Ok(Some(String::from_utf8_lossy(&plain).into_owned()))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    S3,
    Local,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::S3 => "s3",
            TargetKind::Local => "local",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "s3" => Some(TargetKind::S3),
            "local" => Some(TargetKind::Local),
            _ => None,
        }
    }
}

/// Where backups go; secrets are kept apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetSettings {
    S3 {
        /// Blank means AWS.
        #[serde(default)]
        endpoint: Option<String>,
        region: String,
        bucket: String,
        #[serde(default)]
        prefix: String,
        /// Path-style URLs, for MinIO and similar.
        #[serde(default)]
        path_style: bool,
        access_key_id: String,
    },
    Local {
        /// An absolute directory outside the data directory.
        #[schema(value_type = String)]
        path: PathBuf,
    },
}

impl TargetSettings {
    pub fn kind(&self) -> TargetKind {
        match self {
            TargetSettings::S3 { .. } => TargetKind::S3,
            TargetSettings::Local { .. } => TargetKind::Local,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Secrets {
    pub secret_access_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: i64,
    pub settings: TargetSettings,
    pub secret_set: bool,
}

fn target_aad(id: i64) -> Vec<u8> {
    format!("backup-target:{id}").into_bytes()
}

#[derive(sqlx::FromRow)]
struct TargetRow {
    id: i64,
    settings: String,
    secret_set: bool,
}

fn into_target(r: TargetRow) -> Result<Target> {
    Ok(Target {
        id: r.id,
        settings: serde_json::from_str(&r.settings)
            .map_err(|e| Error::Internal(format!("backup target settings: {e}")))?,
        secret_set: r.secret_set,
    })
}

pub async fn list_targets(pool: &SqlitePool) -> Result<Vec<Target>> {
    sqlx::query_as::<_, TargetRow>(
        "SELECT id, settings, secrets IS NOT NULL AS secret_set FROM ee_backup_targets ORDER BY kind DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(into_target)
    .collect()
}

pub async fn get_target(pool: &SqlitePool, kind: TargetKind) -> Result<Option<Target>> {
    sqlx::query_as::<_, TargetRow>(
        "SELECT id, settings, secrets IS NOT NULL AS secret_set FROM ee_backup_targets WHERE kind = ?",
    )
    .bind(kind.as_str())
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .map(into_target)
    .transpose()
}

/// Creates or replaces the target of this kind. `secrets: None` keeps the
/// saved ones.
pub async fn upsert_target(
    store: &Store,
    settings: &TargetSettings,
    secrets: Option<&S3Secrets>,
    now: i64,
) -> Result<Target> {
    let json = serde_json::to_string(settings)
        .map_err(|e| Error::Internal(format!("backup target settings: {e}")))?;
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO ee_backup_targets (kind, settings, created_at, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (kind) DO UPDATE SET settings = excluded.settings, updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(settings.kind().as_str())
    .bind(json)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    if let Some(s) = secrets {
        let plain = serde_json::to_vec(s).map_err(|e| Error::Internal(e.to_string()))?;
        sqlx::query("UPDATE ee_backup_targets SET secrets = ? WHERE id = ?")
            .bind(store.secrets().seal(&target_aad(id), &plain))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    get_target(store.pool(), settings.kind())
        .await?
        .ok_or_else(|| Error::Internal("backup target vanished".into()))
}

/// Whether a target was removed.
pub async fn delete_target(pool: &SqlitePool, kind: TargetKind) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_backup_targets WHERE kind = ?")
        .bind(kind.as_str())
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

pub async fn target_secrets(store: &Store, id: i64) -> Result<Option<S3Secrets>> {
    let sealed: Option<Option<String>> =
        sqlx::query_scalar("SELECT secrets FROM ee_backup_targets WHERE id = ?")
            .bind(id)
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    match sealed.flatten() {
        None => Ok(None),
        Some(s) => {
            let plain = store.secrets().open(&target_aad(id), &s).map_err(|_| {
                Error::Invalid("the saved S3 secret can't be read; enter it again".into())
            })?;
            Ok(Some(serde_json::from_slice(&plain).map_err(|e| {
                Error::Internal(format!("backup target secrets: {e}"))
            })?))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Schedule,
    Manual,
    CatchUp,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Schedule => "schedule",
            Trigger::Manual => "manual",
            Trigger::CatchUp => "catch_up",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct Run {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `schedule`, `manual` or `catch_up`.
    pub trigger: String,
    /// `s3` or `local`; null when skipped.
    pub target_kind: Option<String>,
    /// `success`, `failure` or `skipped`.
    pub result: String,
    pub file_name: Option<String>,
    pub size: Option<i64>,
    pub error: Option<String>,
}

pub struct NewRun<'a> {
    pub started_at: i64,
    pub finished_at: i64,
    pub trigger: Trigger,
    pub target: Option<&'a Target>,
    pub result: &'a str,
    pub file_name: Option<&'a str>,
    pub size: Option<i64>,
    pub error: Option<&'a str>,
}

/// Most run rows kept.
pub const RUNS_KEPT: i64 = 200;

pub async fn record_run(pool: &SqlitePool, r: &NewRun<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO ee_backup_runs (started_at, finished_at, trigger, target_id, target_kind, result, file_name, size, error)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(r.started_at)
    .bind(r.finished_at)
    .bind(r.trigger.as_str())
    .bind(r.target.map(|t| t.id))
    .bind(r.target.map(|t| t.settings.kind().as_str()))
    .bind(r.result)
    .bind(r.file_name)
    .bind(r.size)
    .bind(r.error)
    .execute(pool)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM ee_backup_runs WHERE id NOT IN
             (SELECT id FROM ee_backup_runs ORDER BY started_at DESC, id DESC LIMIT ?)",
    )
    .bind(RUNS_KEPT)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn list_runs(pool: &SqlitePool, limit: i64) -> Result<Vec<Run>> {
    sqlx::query_as(
        "SELECT id, started_at, finished_at, trigger, target_kind, result, file_name, size, error
         FROM ee_backup_runs ORDER BY started_at DESC, id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

pub async fn last_success_at(pool: &SqlitePool) -> Result<Option<i64>> {
    sqlx::query_scalar("SELECT MAX(started_at) FROM ee_backup_runs WHERE result = 'success'")
        .fetch_one(pool)
        .await
        .map_err(db_err)
}

pub async fn last_attempt_at(pool: &SqlitePool) -> Result<Option<i64>> {
    sqlx::query_scalar("SELECT MAX(started_at) FROM ee_backup_runs")
        .fetch_one(pool)
        .await
        .map_err(db_err)
}
