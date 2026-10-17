use super::ctx;
use crate::openapi::{EnvIdParam, EnvParam};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::docker::{NetworkSpec, NetworkSummary, PruneReport};
use fleetops_core::perm::{Row, Verb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct AttachBody {
    pub container: String,
    /// Disconnect only: detach even if the container is running.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedNetwork {
    pub id: String,
}

#[utoipa::path(
    get,
    path = "/envs/{env}/networks",
    operation_id = "listNetworks",
    tag = "networks",
    summary = "List networks",
    params(EnvParam),
    responses((status = 200, description = "Networks", body = Vec<NetworkSummary>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<NetworkSummary>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::View)).await?;
    Ok(Json(c.ops.list_networks().await?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/networks",
    operation_id = "createNetwork",
    tag = "networks",
    summary = "Create a network",
    params(EnvParam),
    request_body = NetworkSpec,
    responses((status = 201, description = "Created", body = CreatedNetwork))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(spec): ApiJson<NetworkSpec>,
) -> ApiResult<(StatusCode, Json<CreatedNetwork>)> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::Create)).await?;
    let res = c.ops.create_network(&spec).await;
    c.audit(&s, "network.create", "network", &spec.name, &res)
        .await;
    Ok((StatusCode::CREATED, Json(CreatedNetwork { id: res? })))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/networks/{id}",
    operation_id = "inspectNetwork",
    tag = "networks",
    summary = "Docker's network inspect, verbatim",
    params(EnvIdParam),
    responses((status = 200, description = "Inspect output", body = Object))
)]
pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::View)).await?;
    Ok(Json(c.ops.inspect_network(&id).await?))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/networks/{id}",
    operation_id = "deleteNetwork",
    tag = "networks",
    summary = "Remove a network",
    params(EnvIdParam),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::Delete)).await?;
    let res = c.ops.remove_network(&id).await;
    c.audit(&s, "network.remove", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/networks/{id}/connect",
    operation_id = "connectNetwork",
    tag = "networks",
    summary = "Attach a container to a network",
    params(EnvIdParam),
    request_body = AttachBody,
    responses((status = 204, description = "Attached"))
)]
pub async fn connect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::Operate)).await?;
    let res = c.ops.connect_network(&id, &body.container).await;
    c.audit(&s, "network.connect", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/networks/{id}/disconnect",
    operation_id = "disconnectNetwork",
    tag = "networks",
    summary = "Detach a container from a network",
    params(EnvIdParam),
    request_body = AttachBody,
    responses((status = 204, description = "Detached"))
)]
pub async fn disconnect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::Operate)).await?;
    let res = c
        .ops
        .disconnect_network(&id, &body.container, body.force)
        .await;
    c.audit(&s, "network.disconnect", "network", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/networks/prune",
    operation_id = "pruneNetworks",
    tag = "networks",
    summary = "Remove unused networks",
    params(EnvParam),
    responses((status = 200, description = "What was removed", body = PruneReport))
)]
pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Networks, Verb::Delete)).await?;
    let res = c.ops.prune_networks().await;
    c.audit(&s, "network.prune", "network", "*", &res).await;
    Ok(Json(res?))
}
