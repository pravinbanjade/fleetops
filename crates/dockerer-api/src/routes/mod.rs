use crate::{error::ApiError, AppState};
use axum::{routing::get, Json, Router};
use dockerer_core::{now_unix, Error};
use dockerer_store::NewAudit;
use serde_json::{json, Value};

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .fallback(not_found)
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn not_found() -> ApiError {
    ApiError(Error::NotFound("no such API route".into()))
}

/// Records an audit row. Failures are logged, never surfaced to the caller.
#[allow(dead_code)] // first used in Task 7
pub(crate) async fn audit(
    s: &AppState,
    user_id: Option<i64>,
    action: &str,
    target_type: &str,
    target_id: &str,
    result: &str,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id,
        environment_id: None,
        action,
        target_type: Some(target_type),
        target_id: Some(target_id),
        result,
        detail: None,
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}
