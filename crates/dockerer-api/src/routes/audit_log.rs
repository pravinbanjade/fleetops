use crate::{auth::extract::AdminUser, ApiResult, AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use dockerer_store::AuditEntry;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub before: Option<i64>,
}

pub async fn list(
    State(s): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(s.store.list_audit(limit, q.before).await?))
}
