//! `/backup` and the first-run restore (backup spec): download a backup,
//! upload one to see what it holds, then restore it.

use crate::{
    auth::extract::AdminUser,
    backup::{data_dir, sealed_columns, tmp_file, StagedBackup, TMP_DIR},
    extract::{ApiJson, ApiMultipart, ApiPath},
    openapi::BackupForm,
    routes::{audit, auth::setup_gate},
    ApiResult, AppState,
};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::backup::{self, BackupMeta, KdfParams};
use serde::Deserialize;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct BackupBody {
    /// At least 12 characters; needed to restore the file.
    pub passphrase: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmRestore {
    /// Must be `RESTORE`.
    pub confirm: String,
}

/// The `id` of a staged backup.
#[derive(Deserialize, utoipa::IntoParams)]
#[allow(dead_code)]
pub struct StageParam {
    id: String,
}

fn edition(s: &AppState) -> String {
    serde_json::to_value(s.entitlements.edition())
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "community".into())
}

/// Writes a backup of this server to a fresh file under `<data>/backup-tmp/`.
/// Public for the Enterprise scheduler.
pub async fn write_to_tmp(s: &AppState, passphrase: &str, now: i64) -> Result<PathBuf> {
    let out = tmp_file(s, "fob")?;
    let meta = BackupMeta {
        now,
        edition: edition(s),
        source: s.config.public_url.clone(),
        sealed: sealed_columns(s),
    };
    let scratch = data_dir(s)?.join(TMP_DIR);
    if let Err(e) = s
        .store
        .write_backup(meta, passphrase, KdfParams::DEFAULT, &scratch, &out)
        .await
    {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    Ok(out)
}

#[utoipa::path(
    post,
    path = "/backup",
    operation_id = "downloadBackup",
    tag = "backup",
    summary = "Download a passphrase-locked backup of this server",
    request_body = BackupBody,
    responses((status = 200, description = "The backup file", body = inline(Vec<u8>), content_type = "application/octet-stream"))
)]
pub async fn download(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(body): ApiJson<BackupBody>,
) -> ApiResult<Response> {
    backup::check_passphrase(&body.passphrase)?;
    let now = now_unix();
    let path = write_to_tmp(&s, &body.passphrase, now).await?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| Error::Internal(format!("backup: {e}")))?;
    let len = file.metadata().await.map(|m| m.len()).ok();
    // The open handle keeps the data readable; nothing stays on disk.
    let _ = std::fs::remove_file(&path);
    audit(
        &s,
        Some(cu.user.id),
        "backup.download",
        "backup",
        "",
        "success",
    )
    .await;
    let mut res = Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Some(len) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        backup::backup_name(now)
    )) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

fn bad(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("bad upload: {e}"))
}

/// Reads the `file` and `passphrase` fields, writing the file to disk.
async fn read_upload(
    s: &AppState,
    mut form: axum::extract::Multipart,
) -> Result<(PathBuf, String)> {
    let path = tmp_file(s, "upload")?;
    let r = async {
        let (mut got_file, mut passphrase) = (false, None);
        while let Some(mut field) = form.next_field().await.map_err(bad)? {
            match field.name().unwrap_or_default() {
                "file" if !got_file => {
                    let mut f = tokio::fs::File::create(&path)
                        .await
                        .map_err(|e| Error::Internal(format!("backup: {e}")))?;
                    while let Some(chunk) = field.chunk().await.map_err(bad)? {
                        f.write_all(&chunk)
                            .await
                            .map_err(|e| Error::Internal(format!("backup: {e}")))?;
                    }
                    f.flush()
                        .await
                        .map_err(|e| Error::Internal(format!("backup: {e}")))?;
                    got_file = true;
                }
                "passphrase" if passphrase.is_none() => {
                    passphrase = Some(field.text().await.map_err(bad)?);
                }
                other => return Err(bad(format!("unexpected field `{other}`"))),
            }
        }
        match (got_file, passphrase) {
            (true, Some(p)) => Ok(p),
            _ => Err(Error::Invalid("send `file` and `passphrase`".into())),
        }
    }
    .await;
    match r {
        Ok(p) => Ok((path, p)),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

async fn stage_upload(
    s: &AppState,
    form: axum::extract::Multipart,
    setup: bool,
) -> Result<StagedBackup> {
    let (path, passphrase) = read_upload(s, form).await?;
    let r = s.staged.stage(s, &path, &passphrase, setup).await;
    let _ = std::fs::remove_file(&path);
    r
}

#[utoipa::path(
    post,
    path = "/backup/stage",
    operation_id = "stageBackup",
    tag = "backup",
    summary = "Upload a backup and see what it holds (up to 512 MiB)",
    request_body(content = BackupForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "Ready to restore", body = StagedBackup))
)]
pub async fn stage(
    State(s): State<AppState>,
    _admin: AdminUser,
    ApiMultipart(form): ApiMultipart,
) -> ApiResult<Json<StagedBackup>> {
    Ok(Json(stage_upload(&s, form, false).await?))
}

#[utoipa::path(
    post,
    path = "/backup/stage/{id}/restore",
    operation_id = "restoreBackup",
    tag = "backup",
    summary = "Replace all data with a staged backup; the server restarts",
    params(StageParam),
    request_body = ConfirmRestore,
    responses((status = 202, description = "Restoring; everyone is signed out"))
)]
pub async fn restore(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<ConfirmRestore>,
) -> ApiResult<StatusCode> {
    if body.confirm != "RESTORE" {
        return Err(Error::Invalid("type RESTORE to confirm".into()).into());
    }
    s.staged
        .apply(&s, &id, false, Some(&cu.user.username))
        .await?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    delete,
    path = "/backup/stage/{id}",
    operation_id = "discardBackup",
    tag = "backup",
    summary = "Forget a staged backup",
    params(StageParam),
    responses((status = 204, description = "Discarded"))
)]
pub async fn discard(
    State(s): State<AppState>,
    _admin: AdminUser,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    s.staged.discard(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/auth/setup/backup",
    operation_id = "stageSetupBackup",
    tag = "backup",
    summary = "First run: upload a backup instead of creating the admin",
    security(()),
    request_body(content = BackupForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "Ready to restore", body = StagedBackup))
)]
pub async fn setup_stage(
    State(s): State<AppState>,
    ApiMultipart(form): ApiMultipart,
) -> ApiResult<Json<StagedBackup>> {
    setup_gate(&s).await?;
    Ok(Json(stage_upload(&s, form, true).await?))
}

#[utoipa::path(
    post,
    path = "/auth/setup/backup/{id}/restore",
    operation_id = "restoreSetupBackup",
    tag = "backup",
    summary = "First run: restore a staged backup; the server restarts",
    security(()),
    params(StageParam),
    responses((status = 202, description = "Restoring"))
)]
pub async fn setup_restore(
    State(s): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    setup_gate(&s).await?;
    s.staged.apply(&s, &id, true, None).await?;
    Ok(StatusCode::ACCEPTED)
}
