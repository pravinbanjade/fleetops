use super::{ctx, ForceQuery};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::docker::{PruneReport, VolumeSpec, VolumeSummary};
use serde_json::Value;

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<VolumeSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_volumes().await?))
}

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

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_volume(&name).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_volume(&name, q.force).await;
    c.audit(&s, "volume.remove", "volume", &name, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

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
