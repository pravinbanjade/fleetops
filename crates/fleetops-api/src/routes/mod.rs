pub mod audit_log;
pub mod auth;
pub mod docker;
pub mod envs;
pub mod tasks;
pub mod users;

use crate::{error::ApiError, AppState};
use axum::{extract::DefaultBodyLimit, routing::get, Json};
use fleetops_core::{now_unix, Error};
use fleetops_store::NewAudit;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, router::UtoipaMethodRouterExt, routes};

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
        .routes(routes!(tasks::get))
        .merge(env_routes())
        .fallback(not_found)
}

fn env_routes() -> OpenApiRouter<AppState> {
    use docker::{
        compose, containers as c, image_tasks, images as i, networks as n, system, volumes as v,
    };
    OpenApiRouter::new()
        .routes(routes!(envs::list, envs::create))
        .routes(routes!(envs::update, envs::remove))
        .routes(routes!(envs::status))
        .routes(routes!(system::info))
        .routes(routes!(system::disk_usage))
        .routes(routes!(c::list, c::create))
        .routes(routes!(c::prune))
        .routes(routes!(c::inspect, c::remove))
        .routes(routes!(c::spec))
        .routes(routes!(c::rename))
        .routes(routes!(c::recreate_container))
        .routes(routes!(c::duplicate))
        .routes(routes!(c::list_files))
        .routes(routes!(c::download_file))
        .routes(routes!(c::action))
        .routes(routes!(i::list))
        .routes(routes!(i::prune))
        .routes(routes!(image_tasks::pull))
        .routes(
            routes!(image_tasks::build).layer(DefaultBodyLimit::max(image_tasks::BUILD_UPLOAD_MAX)),
        )
        .routes(routes!(i::inspect, i::remove))
        .routes(routes!(i::history))
        .routes(routes!(i::tag))
        .routes(routes!(v::list, v::create))
        .routes(routes!(v::prune))
        .routes(routes!(v::inspect, v::remove))
        .routes(routes!(n::list, n::create))
        .routes(routes!(n::prune))
        .routes(routes!(n::inspect, n::remove))
        .routes(routes!(n::connect))
        .routes(routes!(n::disconnect))
        .routes(routes!(compose::list))
        .routes(routes!(compose::action))
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
