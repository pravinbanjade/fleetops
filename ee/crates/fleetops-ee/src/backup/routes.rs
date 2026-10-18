//! `/ee/backup` (backup spec, Enterprise): the schedule, targets, runs, and
//! restoring a recent backup straight from a target. Admins only. Without a
//! license, anything that would make backups run is refused; turning them
//! off always works.

use super::{
    db::{self, Retention, Run, S3Secrets, Schedule, TargetKind, TargetSettings},
    run::Runner,
    schedule::next_slot,
    targets::{sink, validate_local, RemoteFile},
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Extension, Json,
};
use fleetops_api::{
    audit_detail,
    auth::extract::AdminUser,
    authz::require_feature,
    backup::{data_dir, tmp_file, StagedBackup},
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use fleetops_core::{edition::Feature, now_unix, Error, Result};
use fleetops_store::backup::check_passphrase;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

/// How many recent backups the console offers to restore.
pub const RECENT: usize = 5;

#[derive(Deserialize, ToSchema)]
pub struct ScheduleBody {
    pub enabled: bool,
    /// 0–23, UTC.
    pub hour_utc: u8,
    pub retention: Retention,
}

#[derive(Deserialize, ToSchema)]
pub struct PassphraseBody {
    pub passphrase: String,
}

#[derive(Serialize, ToSchema)]
pub struct TargetView {
    pub kind: TargetKind,
    pub settings: TargetSettings,
    /// Whether the S3 secret access key is saved; it is never returned.
    pub secret_set: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct TargetBody {
    pub settings: TargetSettings,
    /// S3 only; absent keeps the saved one.
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct KindParam {
    /// `s3` or `local`.
    #[allow(dead_code)]
    kind: String,
}

#[derive(Deserialize, IntoParams)]
pub struct RunsQuery {
    /// At most 50; default 20.
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct BackupStatus {
    pub licensed: bool,
    pub enabled: bool,
    /// Enabled, but not running for lack of a license.
    pub paused: bool,
    pub last_run: Option<Run>,
    /// The most recent run failed for at least one target.
    pub last_failed: bool,
    pub next_run_at: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct RecentBackup {
    pub target_kind: TargetKind,
    #[serde(flatten)]
    pub file: RemoteFile,
}

#[derive(Serialize, ToSchema)]
pub struct TargetError {
    pub target_kind: TargetKind,
    pub error: String,
}

#[derive(Serialize, ToSchema)]
pub struct RecentBackups {
    pub backups: Vec<RecentBackup>,
    /// Targets that could not be listed.
    pub errors: Vec<TargetError>,
}

#[derive(Deserialize, ToSchema)]
pub struct RestoreRecentBody {
    pub target_kind: TargetKind,
    pub name: String,
    /// Needed only when the backup was made with an earlier passphrase.
    #[serde(default)]
    pub passphrase: Option<String>,
}

fn kind(k: &str) -> Result<TargetKind> {
    TargetKind::parse(k).ok_or_else(|| Error::NotFound(format!("no backup target kind {k}")))
}

async fn audit(s: &AppState, by: i64, action: &str, target: &str, detail: serde_json::Value) {
    audit_detail(
        s,
        Some(by),
        action,
        "backup",
        target,
        "success",
        Some(detail),
    )
    .await
}

#[utoipa::path(get, path = "/ee/backup/schedule", operation_id = "getBackupSchedule", tag = "enterprise",
    responses((status = 200, description = "The schedule", body = Schedule)))]
pub async fn get_schedule(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<Schedule>> {
    Ok(Json(db::get_schedule(s.store.pool()).await?))
}

#[utoipa::path(put, path = "/ee/backup/schedule", operation_id = "putBackupSchedule", tag = "enterprise",
    request_body = ScheduleBody,
    responses((status = 200, description = "Saved", body = Schedule)))]
pub async fn put_schedule(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<ScheduleBody>,
) -> ApiResult<Json<Schedule>> {
    let pool = s.store.pool();
    let cur = db::get_schedule(pool).await?;
    let only_disables = !b.enabled && b.hour_utc == cur.hour_utc && b.retention == cur.retention;
    if !only_disables {
        require_feature(&s, Feature::Backup)?;
    }
    if b.hour_utc > 23 {
        return Err(Error::Invalid("hour_utc must be 0–23".into()).into());
    }
    if matches!(b.retention, Retention::Count(0) | Retention::Days(0)) {
        return Err(Error::Invalid("keep at least 1".into()).into());
    }
    if b.enabled {
        if !cur.passphrase_set {
            return Err(Error::Invalid("set a backup passphrase before enabling".into()).into());
        }
        if db::list_targets(pool).await?.is_empty() {
            return Err(Error::Invalid("add a backup target before enabling".into()).into());
        }
    }
    let next = Schedule {
        enabled: b.enabled,
        hour_utc: b.hour_utc,
        retention: b.retention,
        passphrase_set: cur.passphrase_set,
    };
    db::put_schedule(pool, &next, now_unix()).await?;
    audit(
        &s,
        cu.user.id,
        "backup.schedule.update",
        "schedule",
        json!({
            "enabled": next.enabled, "hour_utc": next.hour_utc, "retention": next.retention,
        }),
    )
    .await;
    Ok(Json(next))
}

#[utoipa::path(put, path = "/ee/backup/schedule/passphrase", operation_id = "putBackupPassphrase", tag = "enterprise",
    request_body = PassphraseBody,
    responses((status = 204, description = "Saved")))]
pub async fn put_passphrase(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<PassphraseBody>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Backup)?;
    check_passphrase(&b.passphrase)?;
    db::set_passphrase(&s.store, &b.passphrase, now_unix()).await?;
    audit(
        &s,
        cu.user.id,
        "backup.passphrase.set",
        "schedule",
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

fn view(t: db::Target) -> TargetView {
    TargetView {
        kind: t.settings.kind(),
        settings: t.settings,
        secret_set: t.secret_set,
    }
}

#[utoipa::path(get, path = "/ee/backup/targets", operation_id = "listBackupTargets", tag = "enterprise",
    responses((status = 200, description = "Targets", body = [TargetView])))]
pub async fn list_targets(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<TargetView>>> {
    Ok(Json(
        db::list_targets(s.store.pool())
            .await?
            .into_iter()
            .map(view)
            .collect(),
    ))
}

#[utoipa::path(put, path = "/ee/backup/targets/{kind}", operation_id = "putBackupTarget", tag = "enterprise",
    params(KindParam), request_body = TargetBody,
    responses((status = 200, description = "Saved", body = TargetView)))]
pub async fn put_target(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(k): ApiPath<String>,
    ApiJson(b): ApiJson<TargetBody>,
) -> ApiResult<Json<TargetView>> {
    require_feature(&s, Feature::Backup)?;
    let k = kind(&k)?;
    if b.settings.kind() != k {
        return Err(Error::Invalid("the settings are for another kind of target".into()).into());
    }
    let secrets = match &b.settings {
        TargetSettings::Local { path } => {
            validate_local(path, data_dir(&s)?)?;
            None
        }
        TargetSettings::S3 {
            bucket,
            region,
            access_key_id,
            ..
        } => {
            if bucket.trim().is_empty()
                || region.trim().is_empty()
                || access_key_id.trim().is_empty()
            {
                return Err(
                    Error::Invalid("bucket, region and access_key_id are required".into()).into(),
                );
            }
            let existing = db::get_target(s.store.pool(), k).await?;
            match b.secret_access_key.filter(|v| !v.is_empty()) {
                Some(v) => Some(S3Secrets {
                    secret_access_key: v,
                }),
                None if existing.is_some_and(|t| t.secret_set) => None,
                None => return Err(Error::Invalid("secret_access_key is required".into()).into()),
            }
        }
    };
    let t = db::upsert_target(&s.store, &b.settings, secrets.as_ref(), now_unix()).await?;
    audit(
        &s,
        cu.user.id,
        "backup.target.update",
        k.as_str(),
        json!({ "settings": t.settings }),
    )
    .await;
    Ok(Json(view(t)))
}

#[utoipa::path(delete, path = "/ee/backup/targets/{kind}", operation_id = "deleteBackupTarget", tag = "enterprise",
    params(KindParam), responses((status = 204, description = "Removed")))]
pub async fn delete_target(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(k): ApiPath<String>,
) -> ApiResult<StatusCode> {
    let k = kind(&k)?;
    if !db::delete_target(s.store.pool(), k).await? {
        return Err(Error::NotFound("no such backup target".into()).into());
    }
    // A schedule with nowhere to write would only fail.
    if db::list_targets(s.store.pool()).await?.is_empty() {
        let mut sched = db::get_schedule(s.store.pool()).await?;
        if sched.enabled {
            sched.enabled = false;
            db::put_schedule(s.store.pool(), &sched, now_unix()).await?;
        }
    }
    audit(
        &s,
        cu.user.id,
        "backup.target.delete",
        k.as_str(),
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn target_sink(s: &AppState, k: TargetKind) -> Result<Box<dyn super::targets::Sink>> {
    let t = db::get_target(s.store.pool(), k)
        .await?
        .ok_or_else(|| Error::NotFound("no such backup target".into()))?;
    let secrets = db::target_secrets(&s.store, t.id).await?;
    sink(&t, secrets, data_dir(s)?)
}

#[utoipa::path(post, path = "/ee/backup/targets/{kind}/test", operation_id = "testBackupTarget", tag = "enterprise",
    params(KindParam), responses((status = 204, description = "Writable")))]
pub async fn test_target(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(k): ApiPath<String>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Backup)?;
    target_sink(&s, kind(&k)?).await?.test().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/ee/backup/run", operation_id = "runBackup", tag = "enterprise",
    responses((status = 202, description = "Started; follow it in the runs")))]
pub async fn run_now(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    Extension(runner): Extension<Arc<Runner>>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Backup)?;
    runner.start(&s, cu.user.id)?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(get, path = "/ee/backup/runs", operation_id = "listBackupRuns", tag = "enterprise",
    params(RunsQuery), responses((status = 200, description = "Newest first", body = [Run])))]
pub async fn list_runs(
    State(s): State<AppState>,
    _a: AdminUser,
    Query(q): Query<RunsQuery>,
) -> ApiResult<Json<Vec<Run>>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    Ok(Json(db::list_runs(s.store.pool(), limit).await?))
}

#[utoipa::path(get, path = "/ee/backup/status", operation_id = "getBackupStatus", tag = "enterprise",
    responses((status = 200, description = "For the page and the banner", body = BackupStatus)))]
pub async fn status(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<BackupStatus>> {
    let pool = s.store.pool();
    let sched = db::get_schedule(pool).await?;
    let licensed = s.entitlements.has(Feature::Backup);
    let runs = db::list_runs(pool, 10).await?;
    let attempt = runs
        .iter()
        .find(|r| r.result != "skipped")
        .map(|r| r.started_at);
    let last_failed = attempt.is_some_and(|t| {
        runs.iter()
            .any(|r| r.started_at == t && r.result == "failure")
    });
    Ok(Json(BackupStatus {
        licensed,
        enabled: sched.enabled,
        paused: sched.enabled && !licensed,
        last_run: runs.into_iter().next(),
        last_failed,
        next_run_at: (sched.enabled && licensed).then(|| next_slot(now_unix(), sched.hour_utc)),
    }))
}

#[utoipa::path(get, path = "/ee/backup/recent", operation_id = "listRecentBackups", tag = "enterprise",
    responses((status = 200, description = "The newest backups across targets", body = RecentBackups)))]
pub async fn recent(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<RecentBackups>> {
    require_feature(&s, Feature::Backup)?;
    let mut backups = Vec::new();
    let mut errors = Vec::new();
    for t in db::list_targets(s.store.pool()).await? {
        let k = t.settings.kind();
        let listed = async {
            let secrets = db::target_secrets(&s.store, t.id).await?;
            sink(&t, secrets, data_dir(&s)?)?.list().await
        }
        .await;
        match listed {
            Ok(files) => backups.extend(files.into_iter().map(|file| RecentBackup {
                target_kind: k,
                file,
            })),
            Err(e) => errors.push(TargetError {
                target_kind: k,
                error: e.to_string(),
            }),
        }
    }
    backups.sort_by_key(|b| std::cmp::Reverse(b.file.created_at));
    backups.truncate(RECENT);
    Ok(Json(RecentBackups { backups, errors }))
}

#[utoipa::path(post, path = "/ee/backup/recent/restore", operation_id = "stageRecentBackup", tag = "enterprise",
    request_body = RestoreRecentBody,
    responses((status = 200, description = "Staged; confirm with POST /backup/stage/{id}/restore", body = StagedBackup)))]
pub async fn restore_recent(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiJson(b): ApiJson<RestoreRecentBody>,
) -> ApiResult<Json<StagedBackup>> {
    require_feature(&s, Feature::Backup)?;
    let file = tmp_file(&s, "fob")?;
    let r = async {
        target_sink(&s, b.target_kind)
            .await?
            .get(&b.name, &file)
            .await?;
        match b.passphrase.filter(|p| !p.is_empty()) {
            Some(p) => s.staged.stage(&s, &file, &p, false).await,
            None => {
                let stored = db::passphrase(&s.store).await?.ok_or_else(|| {
                    Error::Invalid("enter the passphrase this backup was made with".into())
                })?;
                s.staged
                    .stage(&s, &file, &stored, false)
                    .await
                    .map_err(|e| {
                        if e.to_string().contains("wrong passphrase or damaged file") {
                            Error::Invalid(
                            "this backup was made with a different passphrase; enter it to restore"
                                .into(),
                        )
                        } else {
                            e
                        }
                    })
            }
        }
    }
    .await;
    let _ = std::fs::remove_file(&file);
    Ok(Json(r?))
}
