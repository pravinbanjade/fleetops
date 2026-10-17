//! The Enterprise API, under `/ee` (editions spec §4.4).

use crate::{
    export,
    license::{verify, Status},
    state::{LicenseInfo, LicenseState, Loaded, Source},
};
use axum::{
    body::Body,
    extract::State,
    http::header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    response::{IntoResponse, Response},
    Extension, Json,
};
use fleetops_api::{
    auth::extract::AdminUser, authz::require_feature, extract::ApiQuery, ApiResult, AppState,
};
use fleetops_core::{edition::Feature, now_unix, Error};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use utoipa::IntoParams;

type Lic = Extension<Arc<LicenseState>>;

fn day(ts: i64) -> String {
    jiff::Timestamp::from_second(ts)
        .map(|t| t.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

async fn info(s: &AppState, lic: &LicenseState) -> ApiResult<Json<LicenseInfo>> {
    let users = s.store.count_users().await?;
    Ok(Json(lic.info(
        now_unix(),
        users,
        s.envs.count_nodes().await,
    )))
}

#[utoipa::path(
    get,
    path = "/ee/license",
    operation_id = "getLicense",
    tag = "enterprise",
    summary = "The Enterprise license in effect",
    responses((status = 200, description = "License", body = LicenseInfo))
)]
pub async fn get_license(
    State(s): State<AppState>,
    Extension(lic): Lic,
    _admin: AdminUser,
) -> ApiResult<Json<LicenseInfo>> {
    info(&s, &lic).await
}

#[utoipa::path(
    put,
    path = "/ee/license",
    operation_id = "putLicense",
    tag = "enterprise",
    summary = "Install a license (replaces any uploaded one)",
    request_body(content = String, content_type = "text/plain"),
    responses((status = 200, description = "License", body = LicenseInfo))
)]
pub async fn put_license(
    State(s): State<AppState>,
    Extension(lic): Lic,
    AdminUser(admin): AdminUser,
    body: String,
) -> ApiResult<Json<LicenseInfo>> {
    let now = now_unix();
    let parsed = verify(&body, lic.keys()).map_err(|e| Error::Invalid(format!("license: {e}")))?;
    if parsed.status_at(now) != Status::Valid {
        let expired = day(parsed.payload.expires_at);
        return Err(Error::Invalid(format!("license: expired on {expired}")).into());
    }
    crate::db::put(s.store.pool(), body.trim(), now, admin.user.id).await?;
    let detail = json!({
        "customer": parsed.payload.customer,
        "expires_at": parsed.payload.expires_at,
        "max_nodes": parsed.payload.max_nodes,
    });
    let id = parsed.payload.id.clone();
    lic.set(Loaded::Ok(parsed), Some(Source::Uploaded));
    fleetops_api::audit_detail(
        &s,
        Some(admin.user.id),
        "license.update",
        "license",
        &id,
        "success",
        Some(detail),
    )
    .await;
    info(&s, &lic).await
}

#[utoipa::path(
    delete,
    path = "/ee/license",
    operation_id = "deleteLicense",
    tag = "enterprise",
    summary = "Remove the uploaded license (falls back to --license-file)",
    responses((status = 200, description = "License now in effect", body = LicenseInfo))
)]
pub async fn delete_license(
    State(s): State<AppState>,
    Extension(lic): Lic,
    AdminUser(admin): AdminUser,
) -> ApiResult<Json<LicenseInfo>> {
    crate::db::delete(s.store.pool()).await?;
    lic.load(&s.store).await?;
    fleetops_api::audit_detail(
        &s,
        Some(admin.user.id),
        "license.remove",
        "license",
        "uploaded",
        "success",
        None,
    )
    .await;
    info(&s, &lic).await
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// `csv` (default) or `ndjson`.
    #[param(inline)]
    pub format: Option<export::Format>,
    /// Unix seconds, inclusive.
    pub from: Option<i64>,
    /// Unix seconds, exclusive.
    pub to: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/ee/audit/export",
    operation_id = "exportAudit",
    tag = "enterprise",
    summary = "Download the audit log (needs the audit_export feature)",
    params(ExportQuery),
    responses((status = 200, description = "Audit rows, oldest first",
        content((String = "text/csv"), (String = "application/x-ndjson"))))
)]
pub async fn export_audit(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiQuery(q): ApiQuery<ExportQuery>,
) -> ApiResult<Response> {
    require_feature(&s, Feature::AuditExport)?;
    let format = q.format.unwrap_or_default();
    fleetops_api::audit_detail(
        &s,
        Some(admin.user.id),
        "audit.export",
        "audit",
        format.ext(),
        "success",
        Some(json!({"from": q.from, "to": q.to})),
    )
    .await;
    let name = format!("fleetops-audit-{}.{}", now_unix(), format.ext());
    Ok((
        [
            (CONTENT_TYPE, format.mime().to_string()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        Body::from_stream(export::rows(s.store.clone(), q.from, q.to, format)),
    )
        .into_response())
}
