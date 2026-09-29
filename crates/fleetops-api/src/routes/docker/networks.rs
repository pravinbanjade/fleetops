use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::docker::{NetworkSpec, NetworkSummary, PruneReport};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub struct AttachBody {
    pub container: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct CreatedNetwork {
    pub id: String,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<NetworkSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_networks().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(spec): ApiJson<NetworkSpec>,
) -> ApiResult<(StatusCode, Json<CreatedNetwork>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.create_network(&spec).await;
    c.audit(&s, "network.create", "network", &spec.name, &res)
        .await;
    Ok((StatusCode::CREATED, Json(CreatedNetwork { id: res? })))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_network(&id).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_network(&id).await;
    c.audit(&s, "network.remove", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn connect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.connect_network(&id, &body.container).await;
    c.audit(&s, "network.connect", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn disconnect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c
        .ops
        .disconnect_network(&id, &body.container, body.force)
        .await;
    c.audit(&s, "network.disconnect", "network", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_networks().await;
    c.audit(&s, "network.prune", "network", "*", &res).await;
    Ok(Json(res?))
}
