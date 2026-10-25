//! A swarm's cluster, read through a manager's agent (swarm spec §4):
//! nodes, services, tasks, secrets, configs and networks, each behind its
//! Swarm permission row (spec §6). Cluster calls need the permission on the
//! whole environment: stack-scoped grants come with stack scoping.

use crate::openapi::EnvParam;
use crate::{
    auth::extract::CurrentUser,
    authz::{deny_audited, Need},
    extract::ApiPath,
    routes::kube::wrong_kind,
    ApiResult, AppState,
};
use fleetops_core::access::Grants;
use axum::{
    extract::{Query, State},
    Json,
};
use fleetops_core::env::Environment;
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::swarm::{
    ConfigSummary, NodeDetail, NodeSummary, SecretSummary, ServiceDetail, ServiceSummary,
    SwarmNetwork, SwarmOps, TaskSummary,
};
use fleetops_core::{Error, Result};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::IntoParams;

/// Resolves a swarm environment and checks `need`; a permission must be
/// held on the whole environment.
async fn checked(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    need: Need,
) -> Result<(Arc<dyn Environment>, Grants)> {
    let (env, grants) = crate::authz::env_access(s, cu, name, need).await?;
    if !env.kind().is_swarm() {
        return Err(wrong_kind(name, env.kind()));
    }
    if let Need::Perm(p) = need {
        deny_audited(s, cu, env.id(), &p.to_string(), grants.check_env(p)).await?;
    }
    Ok((env, grants))
}

/// A swarm environment the caller may use, reachable or not: for live
/// feeds, which say themselves when no manager answers and recover.
pub(crate) async fn swarm_env_for(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    need: Need,
) -> Result<(Arc<dyn Environment>, Grants)> {
    checked(s, cu, name, need).await
}

fn swarm_ops(env: &Arc<dyn Environment>) -> Result<Arc<dyn SwarmOps>> {
    env.swarm()
        .ok_or_else(|| Error::EnvUnavailable("no manager agent is connected".into()))
}

/// The swarm's cluster calls, through its serving manager.
pub(crate) async fn swarm_for(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    need: Need,
) -> Result<(Arc<dyn Environment>, Arc<dyn SwarmOps>)> {
    let (env, _) = checked(s, cu, name, need).await?;
    let ops = swarm_ops(&env)?;
    Ok((env, ops))
}

/// Everything a swarm write needs: the caller, the environment, its
/// cluster calls and the caller's grants.
pub struct SwarmCtx {
    pub user: CurrentUser,
    pub env: Arc<dyn Environment>,
    pub ops: Arc<dyn SwarmOps>,
    pub grants: Grants,
}

pub(crate) async fn swarm_ctx(
    s: &AppState,
    user: CurrentUser,
    name: &str,
    p: Permission,
) -> Result<SwarmCtx> {
    let (env, grants) = checked(s, &user, name, Need::Perm(p)).await?;
    let ops = swarm_ops(&env)?;
    Ok(SwarmCtx {
        user,
        env,
        ops,
        grants,
    })
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(dead_code)]
pub struct EnvIdPath {
    /// Environment name.
    env: String,
    /// Swarm object id.
    id: String,
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TaskFilter {
    /// Only this service's tasks (id).
    pub service: Option<String>,
    /// Only tasks on this node (id).
    pub node: Option<String>,
}

#[utoipa::path(get, path = "/envs/{env}/swarm/nodes", operation_id = "listSwarmNodes", tag = "swarm",
    summary = "The swarm's nodes", params(EnvParam),
    responses((status = 200, description = "Every node", body = Vec<NodeSummary>)))]
pub async fn nodes(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<NodeSummary>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmNodes, Verb::View)).await?;
    Ok(Json(ops.nodes().await?))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/nodes/{id}", operation_id = "getSwarmNode", tag = "swarm",
    summary = "One node", params(EnvIdPath),
    responses((status = 200, description = "The node", body = NodeDetail)))]
pub async fn node(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<NodeDetail>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmNodes, Verb::View)).await?;
    let n = ops
        .node_detail(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such node '{id}'")))?;
    Ok(Json(n))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/services", operation_id = "listSwarmServices", tag = "swarm",
    summary = "The swarm's services", params(EnvParam),
    responses((status = 200, description = "Every service", body = Vec<ServiceSummary>)))]
pub async fn services(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ServiceSummary>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmServices, Verb::View)).await?;
    Ok(Json(ops.services().await?))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/services/{id}", operation_id = "getSwarmService", tag = "swarm",
    summary = "One service, with its spec", params(EnvIdPath),
    responses((status = 200, description = "The service", body = ServiceDetail)))]
pub async fn service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<ServiceDetail>> {
    let (env, grants) = checked(&s, &cu, &env, Need::of(Row::SwarmServices, Verb::View)).await?;
    let ops = swarm_ops(&env)?;
    let mut svc = ops
        .service(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such service '{id}'")))?;
    // As for containers: env values are secret material without Reveal.
    let reveal = Permission::new(Row::Containers, Verb::Reveal);
    if !grants.allows(reveal, None) {
        if let Some(vars) = svc
            .spec
            .pointer_mut("/TaskTemplate/ContainerSpec/Env")
            .and_then(serde_json::Value::as_array_mut)
        {
            let mut text: Vec<String> = vars
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect();
            fleetops_core::docker::mask_env(&mut text);
            *vars = text.into_iter().map(serde_json::Value::String).collect();
        }
    }
    Ok(Json(svc))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/tasks", operation_id = "listSwarmTasks", tag = "swarm",
    summary = "The swarm's tasks", params(EnvParam, TaskFilter),
    responses((status = 200, description = "Matching tasks", body = Vec<TaskSummary>)))]
pub async fn tasks(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    Query(f): Query<TaskFilter>,
) -> ApiResult<Json<Vec<TaskSummary>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmServices, Verb::View)).await?;
    let tasks = ops
        .tasks()
        .await?
        .into_iter()
        .filter(|t| f.service.as_ref().is_none_or(|s| *s == t.service_id))
        .filter(|t| {
            f.node
                .as_ref()
                .is_none_or(|n| t.node_id.as_ref() == Some(n))
        })
        .collect();
    Ok(Json(tasks))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/secrets", operation_id = "listSwarmSecrets", tag = "swarm",
    summary = "The swarm's secrets (metadata only)", params(EnvParam),
    responses((status = 200, description = "Every secret", body = Vec<SecretSummary>)))]
pub async fn secrets(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<SecretSummary>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmSecrets, Verb::View)).await?;
    Ok(Json(ops.secrets().await?))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/configs", operation_id = "listSwarmConfigs", tag = "swarm",
    summary = "The swarm's configs", params(EnvParam),
    responses((status = 200, description = "Every config", body = Vec<ConfigSummary>)))]
pub async fn configs(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ConfigSummary>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::SwarmConfigs, Verb::View)).await?;
    Ok(Json(ops.configs().await?))
}

#[utoipa::path(get, path = "/envs/{env}/swarm/networks", operation_id = "listSwarmNetworks", tag = "swarm",
    summary = "The swarm's swarm-scoped networks", params(EnvParam),
    responses((status = 200, description = "Every swarm network", body = Vec<SwarmNetwork>)))]
pub async fn networks(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<SwarmNetwork>>> {
    let (_, ops) = swarm_for(&s, &cu, &env, Need::of(Row::Networks, Verb::View)).await?;
    Ok(Json(ops.networks().await?))
}
