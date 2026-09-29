use super::ctx;
use crate::{auth::extract::CurrentUser, authz::Action, extract::ApiPath, ApiResult, AppState};
use axum::{extract::State, Json};
use serde_json::Value;

pub async fn info(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.info().await?))
}

pub async fn disk_usage(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.disk_usage().await?))
}
