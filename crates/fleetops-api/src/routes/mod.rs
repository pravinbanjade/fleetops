pub mod audit_log;
pub mod auth;
pub mod docker;
pub mod envs;
pub mod tasks;
pub mod users;

use crate::{error::ApiError, AppState};
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, patch, post},
    Json, Router,
};
use fleetops_core::{now_unix, Error};
use fleetops_store::NewAudit;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

/// Every API route, with its documentation. Routes are registered only
/// through `routes!`, which takes each path from the handler's
/// `#[utoipa::path]`, so the served API and the OpenAPI document agree.
pub fn api() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(crate::openapi::serve))
        .routes(routes!(auth::status))
        .routes(routes!(auth::setup))
        .routes(routes!(auth::login))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(auth::list_sessions))
        .routes(routes!(auth::revoke_session))
        .routes(routes!(auth::change_password))
        .routes(routes!(users::list, users::create))
        .routes(routes!(users::update, users::remove))
        .routes(routes!(audit_log::list))
        .route("/ws", get(crate::ws::handler))
        .route("/tasks/{id}", get(tasks::get))
        .merge(OpenApiRouter::from(env_routes()))
        .fallback(not_found)
}

fn env_routes() -> Router<AppState> {
    use docker::{
        compose, containers as c, image_tasks, images as i, networks as n, system, volumes as v,
    };
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
        .route("/envs/{env}/images", get(i::list))
        .route("/envs/{env}/images/prune", post(i::prune))
        .route("/envs/{env}/images/pull", post(image_tasks::pull))
        .route(
            "/envs/{env}/images/build",
            post(image_tasks::build).layer(DefaultBodyLimit::max(image_tasks::BUILD_UPLOAD_MAX)),
        )
        .route("/envs/{env}/images/{id}", get(i::inspect).delete(i::remove))
        .route("/envs/{env}/images/{id}/history", get(i::history))
        .route("/envs/{env}/images/{id}/tag", post(i::tag))
        .route("/envs/{env}/volumes", get(v::list).post(v::create))
        .route("/envs/{env}/volumes/prune", post(v::prune))
        .route(
            "/envs/{env}/volumes/{name}",
            get(v::inspect).delete(v::remove),
        )
        .route("/envs/{env}/networks", get(n::list).post(n::create))
        .route("/envs/{env}/networks/prune", post(n::prune))
        .route(
            "/envs/{env}/networks/{id}",
            get(n::inspect).delete(n::remove),
        )
        .route("/envs/{env}/networks/{id}/connect", post(n::connect))
        .route("/envs/{env}/networks/{id}/disconnect", post(n::disconnect))
        .route("/envs/{env}/compose-projects", get(compose::list))
        .route(
            "/envs/{env}/compose-projects/{project}/{action}",
            post(compose::action),
        )
}

#[derive(Serialize, ToSchema)]
pub struct Health {
    /// Always `ok`.
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/health",
    operation_id = "getHealth",
    tag = "system",
    summary = "Liveness check",
    security(()),
    responses((status = 200, description = "The server is up", body = Health))
)]
pub async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
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
