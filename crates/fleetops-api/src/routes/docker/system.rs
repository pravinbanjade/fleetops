use super::ctx;
use crate::openapi::EnvParam;
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiPath, ApiQuery},
    ApiResult, AppState,
};
use axum::{extract::State, Json};
use fleetops_core::{now_unix, streams::DockerEvent};
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;

/// How far back `GET /envs/{env}/events` looks at most, in seconds.
pub const RECENT_EVENTS_MAX_AGE: i64 = 3600;
/// Its default look-back when `since` is not given.
pub const RECENT_EVENTS_DEFAULT_AGE: i64 = 900;

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
    let c = ctx(&s, cu, &env, Need::AnyGrant).await?;
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
    let c = ctx(&s, cu, &env, Need::AnyGrant).await?;
    Ok(Json(c.ops.disk_usage().await?))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsQuery {
    /// Unix seconds. Defaults to 15 minutes ago; anything older than an hour
    /// is treated as an hour ago.
    pub since: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/envs/{env}/events",
    operation_id = "listRecentEvents",
    tag = "environments",
    summary = "The Docker events still held since a time, oldest first",
    params(EnvParam, EventsQuery),
    responses((status = 200, description = "Recent events", body = [DockerEvent]))
)]
pub async fn recent_events(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiQuery(q): ApiQuery<EventsQuery>,
) -> ApiResult<Json<Vec<DockerEvent>>> {
    let c = ctx(&s, cu, &env, Need::AnyGrant).await?;
    let now = now_unix();
    let since = q
        .since
        .unwrap_or(now - RECENT_EVENTS_DEFAULT_AGE)
        .max(now - RECENT_EVENTS_MAX_AGE);
    Ok(Json(c.ops.recent_events(since).await?))
}
