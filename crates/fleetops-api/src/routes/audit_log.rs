use crate::{auth::extract::AdminUser, extract::ApiQuery, ApiResult, AppState};
use axum::{extract::State, Json};
use fleetops_store::{AuditEntry, AuditFilter};
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// Entries to return, 1-500 (default 100).
    pub limit: Option<i64>,
    /// Only entries with an id below this, for paging.
    pub before: Option<i64>,
    /// Only this user's entries.
    pub user_id: Option<i64>,
    /// Only this action, or every action under a prefix ending in `.` (e.g. `container.`).
    pub action: Option<String>,
    /// Only entries about this environment.
    pub env_id: Option<i64>,
    /// Only entries at or after this time, in Unix seconds.
    pub since: Option<i64>,
    /// Only entries that did not succeed.
    pub failed: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/audit",
    operation_id = "listAudit",
    tag = "audit",
    summary = "Audit entries, newest first",
    params(AuditQuery),
    responses((status = 200, description = "Audit entries", body = Vec<AuditEntry>))
)]
pub async fn list(
    State(s): State<AppState>,
    _admin: AdminUser,
    ApiQuery(q): ApiQuery<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let f = AuditFilter {
        user_id: q.user_id,
        action: q.action.filter(|a| !a.is_empty()),
        environment_id: q.env_id,
        since: q.since,
        failed: q.failed.unwrap_or(false),
    };
    Ok(Json(
        s.store.list_audit_filtered(limit, q.before, &f).await?,
    ))
}
