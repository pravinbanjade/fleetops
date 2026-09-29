pub mod audit_log;
pub mod auth;
pub mod docker;
pub mod envs;
pub mod users;

use crate::{error::ApiError, AppState};
use axum::{
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use dockerer_core::{now_unix, Error};
use dockerer_store::NewAudit;
use serde_json::{json, Value};

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/auth/status", get(auth::status))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/{id}", delete(auth::revoke_session))
        .route("/auth/password", put(auth::change_password))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", patch(users::update).delete(users::remove))
        .route("/audit", get(audit_log::list))
        .merge(env_routes())
        .fallback(not_found)
}

fn env_routes() -> Router<AppState> {
    use docker::{containers as c, system};
    Router::new()
        .route("/envs", get(envs::list).post(envs::create))
        .route("/envs/{env}", patch(envs::update).delete(envs::remove))
        .route("/envs/{env}/status", get(envs::status))
        .route("/envs/{env}/info", get(system::info))
        .route("/envs/{env}/df", get(system::disk_usage))
        .route("/envs/{env}/containers", get(c::list).post(c::create))
        .route("/envs/{env}/containers/prune", post(c::prune))
        .route(
            "/envs/{env}/containers/{id}",
            get(c::inspect).delete(c::remove),
        )
        .route("/envs/{env}/containers/{id}/spec", get(c::spec))
        .route("/envs/{env}/containers/{id}/rename", post(c::rename))
        .route(
            "/envs/{env}/containers/{id}/recreate",
            post(c::recreate_container),
        )
        .route("/envs/{env}/containers/{id}/duplicate", post(c::duplicate))
        .route("/envs/{env}/containers/{id}/files", get(c::list_files))
        .route(
            "/envs/{env}/containers/{id}/files/download",
            get(c::download_file),
        )
        .route("/envs/{env}/containers/{id}/{action}", post(c::action))
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn not_found() -> ApiError {
    ApiError(Error::NotFound("no such API route".into()))
}

/// Records an audit row. Failures are logged, never surfaced to the caller.
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
