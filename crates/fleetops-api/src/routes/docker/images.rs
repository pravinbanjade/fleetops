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
use fleetops_core::docker::{ImageSummary, PruneReport};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct TagBody {
    pub repo: String,
    #[serde(default = "latest")]
    pub tag: String,
}

fn latest() -> String {
    "latest".into()
}

#[derive(Deserialize, Default)]
pub struct PruneQuery {
    /// Also remove unused tagged images, not just dangling ones.
    #[serde(default)]
    pub all: bool,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ImageSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_images().await?))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_image(&id).await?))
}

pub async fn history(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.image_history(&id).await?))
}

pub async fn tag(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<TagBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.tag_image(&id, &body.repo, &body.tag).await;
    c.audit(&s, "image.tag", "image", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_image(&id, q.force).await;
    c.audit(&s, "image.remove", "image", &id, &res).await;
    Ok(Json(json!({ "removed": res? })))
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    Query(q): Query<PruneQuery>,
) -> ApiResult<Json<PruneReport>> {
    let all = q.all;
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_images(all).await;
    c.audit(
        &s,
        "image.prune",
        "image",
        if all { "all" } else { "dangling" },
        &res,
    )
    .await;
    Ok(Json(res?))
}
