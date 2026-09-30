use super::{ctx, ForceQuery};
use crate::openapi::{EnvNameParam, EnvParam};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath, ApiQuery},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::docker::{PruneReport, VolumeSpec, VolumeSummary};
use serde_json::Value;

#[utoipa::path(
    get,
    path = "/envs/{env}/volumes",
    operation_id = "listVolumes",
    tag = "volumes",
    summary = "List volumes",
    params(EnvParam),
    responses((status = 200, description = "Volumes", body = Vec<VolumeSummary>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<VolumeSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_volumes().await?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/volumes",
    operation_id = "createVolume",
    tag = "volumes",
    summary = "Create a volume",
    params(EnvParam),
    request_body = VolumeSpec,
    responses((status = 201, description = "Created", body = VolumeSummary))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(spec): ApiJson<VolumeSpec>,
) -> ApiResult<(StatusCode, Json<VolumeSummary>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.create_volume(&spec).await;
    c.audit(&s, "volume.create", "volume", &spec.name, &res)
        .await;
    Ok((StatusCode::CREATED, Json(res?)))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/volumes/{name}",
    operation_id = "inspectVolume",
    tag = "volumes",
    summary = "Docker's volume inspect, verbatim",
    params(EnvNameParam),
    responses((status = 200, description = "Inspect output", body = Object))
)]
pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_volume(&name).await?))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/volumes/{name}",
    operation_id = "deleteVolume",
    tag = "volumes",
    summary = "Remove a volume",
    params(EnvNameParam, ForceQuery),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_volume(&name, q.force).await;
    c.audit(&s, "volume.remove", "volume", &name, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/volumes/prune",
    operation_id = "pruneVolumes",
    tag = "volumes",
    summary = "Remove unused volumes",
    params(EnvParam),
    responses((status = 200, description = "What was removed", body = PruneReport))
)]
pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_volumes().await;
    c.audit(&s, "volume.prune", "volume", "*", &res).await;
    Ok(Json(res?))
}
