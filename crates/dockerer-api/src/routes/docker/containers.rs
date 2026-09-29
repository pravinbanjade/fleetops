use super::{ctx, ForceQuery};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dockerer_core::docker::{
    ContainerAction, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use dockerer_core::recreate::recreate;
use dockerer_core::{now_unix, Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest file the browser will download through the API.
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(flatten)]
    pub spec: ContainerSpec,
    /// Start the container after creating it (default true).
    #[serde(default = "yes")]
    pub start: bool,
}

#[derive(Serialize)]
pub struct Created {
    pub id: String,
}

#[derive(Deserialize)]
pub struct NameBody {
    pub name: String,
}

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct RecreateResult {
    pub id: String,
    pub old_removed: bool,
    pub backup_name: String,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ContainerSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_containers().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    body.spec.validate()?;
    let target = body
        .spec
        .name
        .clone()
        .unwrap_or_else(|| body.spec.image.clone());
    let res = c.ops.create_container(&body.spec).await;
    c.audit(&s, "container.create", "container", &target, &res)
        .await;
    let id = res?;
    if body.start {
        let started = c.ops.container_action(&id, ContainerAction::Start).await;
        c.audit(&s, "container.start", "container", &id, &started)
            .await;
        started?;
    }
    Ok((StatusCode::CREATED, Json(Created { id })))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_container(&id).await?))
}

pub async fn spec(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<ContainerSpec>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.container_spec(&id).await?))
}

pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id, action)): ApiPath<(String, String, String)>,
) -> ApiResult<StatusCode> {
    let action: ContainerAction = action.parse()?;
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.container_action(&id, action).await;
    c.audit(
        &s,
        &format!("container.{}", action.as_str()),
        "container",
        &id,
        &res,
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rename(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.rename_container(&id, &body.name).await;
    c.audit(&s, "container.rename", "container", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_container(&id, q.force, q.volumes).await;
    c.audit(&s, "container.remove", "container", &id, &res)
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
    let res = c.ops.prune_containers().await;
    c.audit(&s, "container.prune", "container", "*", &res).await;
    Ok(Json(res?))
}

pub async fn recreate_container(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(spec): ApiJson<ContainerSpec>,
) -> ApiResult<Json<RecreateResult>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = recreate(&*c.ops, &id, &spec, now_unix()).await;
    c.audit(&s, "container.recreate", "container", &id, &res)
        .await;
    let out = res?;
    Ok(Json(RecreateResult {
        id: out.new_id,
        old_removed: out.old_removed,
        backup_name: out.backup_name,
    }))
}

pub async fn duplicate(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let mut spec = c.ops.container_spec(&id).await?;
    spec.name = Some(body.name);
    spec.validate()?;
    let res = async {
        let new_id = c.ops.create_container(&spec).await?;
        c.ops
            .container_action(&new_id, ContainerAction::Start)
            .await?;
        Ok::<_, Error>(new_id)
    }
    .await;
    c.audit(&s, "container.duplicate", "container", &id, &res)
        .await;
    Ok((StatusCode::CREATED, Json(Created { id: res? })))
}

pub async fn list_files(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<DirListing>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_dir(&id, &q.path).await?))
}

pub async fn download_file(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Response> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    let data = c.ops.read_file(&id, &q.path, MAX_DOWNLOAD_BYTES).await?;
    let file_name: String = q
        .path
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\')
        .collect();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        if file_name.is_empty() {
            "file"
        } else {
            &file_name
        }
    );
    let mut res = data.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}
