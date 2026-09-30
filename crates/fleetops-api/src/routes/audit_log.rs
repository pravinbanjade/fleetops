use crate::{auth::extract::AdminUser, ApiResult, AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use fleetops_store::AuditEntry;
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditQuery {
    /// Entries to return, 1-500 (default 100).
    pub limit: Option<i64>,
    /// Only entries with an id below this, for paging.
    pub before: Option<i64>,
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
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(s.store.list_audit(limit, q.before).await?))
}
