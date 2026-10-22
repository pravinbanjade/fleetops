pub mod access;
pub mod audit_log;
pub mod auth;
pub mod backup;
pub mod docker;
pub mod edition;
pub mod envs;
pub(crate) mod helm;
pub(crate) mod kube;
pub(crate) mod kube_generic;
pub mod stack_hooks;
pub mod tasks;
pub mod users;

use crate::{error::ApiError, AppState};
use axum::{extract::DefaultBodyLimit, Json};
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
        .routes(routes!(edition::get))
        .routes(routes!(crate::openapi::serve))
        .routes(routes!(auth::status))
        .routes(routes!(auth::setup))
        .routes(
            routes!(backup::setup_stage)
                .layer(DefaultBodyLimit::max(crate::backup::BACKUP_UPLOAD_MAX)),
        )
        .routes(routes!(backup::setup_restore))
        .routes(routes!(backup::download))
        .routes(
            routes!(backup::stage).layer(DefaultBodyLimit::max(crate::backup::BACKUP_UPLOAD_MAX)),
        )
        .routes(routes!(backup::restore))
        .routes(routes!(backup::discard))
        .routes(routes!(auth::login))
        .routes(routes!(auth::providers))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(auth::list_sessions))
        .routes(routes!(auth::revoke_session))
        .routes(routes!(auth::change_password))
        .routes(routes!(users::list, users::create))
        .routes(routes!(users::update, users::remove))
        .routes(routes!(access::list_user))
        .routes(routes!(access::set, access::revoke))
        .routes(routes!(audit_log::list))
        .routes(routes!(crate::ws::handler))
        .routes(routes!(crate::agents::connect::handler))
        .routes(routes!(tasks::get))
        .routes(routes!(stack_hooks::hook).layer(DefaultBodyLimit::max(stack_hooks::HOOK_BODY_MAX)))
        .merge(env_routes())
        .fallback(not_found)
}

fn env_routes() -> OpenApiRouter<AppState> {
    use docker::{
        compose, containers as c, image_tasks, images as i, networks as n, stack_files, stacks,
        stacks_git, system, volumes as v,
    };
    OpenApiRouter::new()
        .routes(routes!(envs::list, envs::create))
        .routes(routes!(envs::update, envs::remove))
        .routes(routes!(envs::status))
        .routes(routes!(access::list_env))
        .routes(routes!(envs::rotate_agent))
        .routes(routes!(envs::kube_contexts))
        .routes(routes!(system::info))
        .routes(routes!(system::disk_usage))
        .routes(routes!(system::recent_events))
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
        .routes(routes!(stacks::list, stacks::create))
        .routes(routes!(stacks::validate))
        .routes(routes!(stacks::get, stacks::remove))
        .routes(routes!(stacks::save_draft))
        .routes(routes!(stacks::plan_stack))
        .routes(routes!(stacks::deploy))
        .routes(routes!(stacks::revisions))
        .routes(routes!(stacks::revision))
        .routes(routes!(stacks::action))
        .routes(routes!(
            stacks_git::get,
            stacks_git::set,
            stacks_git::detach
        ))
        .routes(routes!(stacks_git::sync))
        .routes(routes!(stacks_git::resume))
        .routes(routes!(stacks_git::rotate_secret))
        .routes(routes!(stacks_git::reset_host_key))
        .routes(routes!(stack_files::list))
        .routes(routes!(stack_files::set, stack_files::remove))
        .routes(routes!(kube::apply))
        .routes(routes!(kube::templates))
        .routes(routes!(helm::list))
        .routes(routes!(helm::get))
        .routes(routes!(helm::history))
        .routes(routes!(helm::rollback))
        .routes(routes!(helm::uninstall))
        .routes(routes!(helm::capabilities))
        .routes(routes!(helm::show))
        .routes(routes!(helm::install))
        .routes(routes!(helm::upgrade))
        .routes(routes!(kube::usage))
        .routes(routes!(kube::counts))
        .routes(routes!(kube::list))
        .routes(routes!(kube::get, kube::delete, kube::replace))
        .routes(routes!(kube::editable))
        .routes(routes!(kube::events))
        .routes(routes!(kube::reveal_secret))
        .routes(routes!(kube::workload_pods))
        .routes(routes!(kube::scale))
        .routes(routes!(kube::restart))
        .routes(routes!(kube::suspend))
        .routes(routes!(kube::resume))
        .routes(routes!(kube::cordon))
        .routes(routes!(kube::uncordon))
        .routes(routes!(kube::trigger))
        .routes(routes!(kube_generic::discovery))
        .routes(routes!(kube_generic::list))
        .routes(routes!(
            kube_generic::get,
            kube_generic::delete,
            kube_generic::replace
        ))
        .routes(routes!(kube_generic::editable))
        .routes(routes!(kube_generic::events))
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
    audit_detail(s, user_id, action, target_type, target_id, result, None).await
}

/// `audit` with a detail object. Public for Enterprise routes.
pub async fn audit_detail(
    s: &AppState,
    user_id: Option<i64>,
    action: &str,
    target_type: &str,
    target_id: &str,
    result: &str,
    detail: Option<serde_json::Value>,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id,
        environment_id: None,
        action,
        target_type: Some(target_type),
        target_id: Some(target_id),
        result,
        detail,
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}
