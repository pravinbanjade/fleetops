use super::ctx;
use crate::openapi::EnvParam;
use crate::{auth::extract::CurrentUser, authz::Action, extract::ApiPath, ApiResult, AppState};
use axum::{extract::State, Json};
use serde_json::Value;

#[utoipa::path(
    get,
    path = "/envs/{env}/info",
    operation_id = "getEngineInfo",
    tag = "environments",
    summary = "Docker's `/info`, verbatim",
    params(EnvParam),
    responses((status = 200, description = "Engine information", body = Object))
)]
pub async fn info(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.info().await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/df",
    operation_id = "getDiskUsage",
    tag = "environments",
    summary = "Docker's `/system/df`, verbatim",
    params(EnvParam),
    responses((status = 200, description = "Disk usage", body = Object))
)]
pub async fn disk_usage(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.disk_usage().await?))
}
