use super::{ctx, ForceQuery};
use crate::openapi::{EnvIdParam, EnvParam};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath, ApiQuery},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::docker::{ImageSummary, PruneReport};
use fleetops_core::perm::{Row, Verb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, ToSchema)]
pub struct TagBody {
    pub repo: String,
    #[serde(default = "latest")]
    pub tag: String,
}

fn latest() -> String {
    "latest".into()
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PruneQuery {
    /// Also remove unused tagged images, not just dangling ones.
    #[serde(default)]
    pub all: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ImageRemoved {
    /// Image ids deleted and tags removed, as Docker reports them.
    pub removed: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/envs/{env}/images",
    operation_id = "listImages",
    tag = "images",
    summary = "List images",
    params(EnvParam),
    responses((status = 200, description = "Images", body = Vec<ImageSummary>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ImageSummary>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::View)).await?;
    Ok(Json(c.ops.list_images().await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/images/{id}",
    operation_id = "inspectImage",
    tag = "images",
    summary = "Docker's image inspect, verbatim",
    params(EnvIdParam),
    responses((status = 200, description = "Inspect output", body = Object))
)]
pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::View)).await?;
    Ok(Json(c.ops.inspect_image(&id).await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/images/{id}/history",
    operation_id = "getImageHistory",
    tag = "images",
    summary = "Docker's image history, verbatim",
    params(EnvIdParam),
    responses((status = 200, description = "Layers, newest first", body = Vec<Object>))
)]
pub async fn history(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::View)).await?;
    Ok(Json(c.ops.image_history(&id).await?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/images/{id}/tag",
    operation_id = "tagImage",
    tag = "images",
    summary = "Add a tag to an image",
    params(EnvIdParam),
    request_body = TagBody,
    responses((status = 204, description = "Tagged"))
)]
pub async fn tag(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<TagBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::Create)).await?;
    let res = c.ops.tag_image(&id, &body.repo, &body.tag).await;
    c.audit(&s, "image.tag", "image", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/images/{id}",
    operation_id = "deleteImage",
    tag = "images",
    summary = "Remove an image",
    params(EnvIdParam, ForceQuery),
    responses((status = 200, description = "Removed and untagged ids", body = ImageRemoved))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<ForceQuery>,
) -> ApiResult<Json<ImageRemoved>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::Delete)).await?;
    let res = c.ops.remove_image(&id, q.force).await;
    c.audit(&s, "image.remove", "image", &id, &res).await;
    Ok(Json(ImageRemoved { removed: res? }))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/images/prune",
    operation_id = "pruneImages",
    tag = "images",
    summary = "Remove dangling images, or all unused ones with `all`",
    params(EnvParam, PruneQuery),
    responses((status = 200, description = "What was removed", body = PruneReport))
)]
pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiQuery(q): ApiQuery<PruneQuery>,
) -> ApiResult<Json<PruneReport>> {
    let all = q.all;
    let c = ctx(&s, cu, &env, Need::of(Row::Images, Verb::Delete)).await?;
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
