use super::{ctx, ForceQuery};
use crate::openapi::{Bytes, ContainerActionName, EnvIdParam, EnvParam};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath, ApiQuery},
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fleetops_core::docker::{
    ContainerAction, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::recreate::recreate;
use fleetops_core::{now_unix, Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

/// Largest file the browser will download through the API.
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

fn yes() -> bool {
    true
}

#[derive(Deserialize, ToSchema)]
pub struct CreateBody {
    #[serde(flatten)]
    pub spec: ContainerSpec,
    /// Start the container after creating it (default true).
    #[serde(default = "yes")]
    pub start: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Created {
    pub id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct NameBody {
    pub name: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PathQuery {
    /// Absolute path inside the container.
    pub path: String,
}

#[derive(Serialize, ToSchema)]
pub struct RecreateResult {
    pub id: String,
    pub old_removed: bool,
    pub backup_name: String,
}

#[utoipa::path(
    get,
    path = "/envs/{env}/containers",
    operation_id = "listContainers",
    tag = "containers",
    summary = "List containers, running or not",
    params(EnvParam),
    responses((status = 200, description = "Containers", body = Vec<ContainerSummary>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ContainerSummary>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::View)).await?;
    Ok(Json(c.ops.list_containers().await?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/containers",
    operation_id = "createContainer",
    tag = "containers",
    summary = "Create a container, and start it unless `start` is false",
    params(EnvParam),
    request_body = CreateBody,
    responses((status = 201, description = "Created", body = Created))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Create)).await?;
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

#[utoipa::path(
    get,
    path = "/envs/{env}/containers/{id}",
    operation_id = "inspectContainer",
    tag = "containers",
    summary = "Docker's container inspect, verbatim",
    params(EnvIdParam),
    responses((status = 200, description = "Inspect output", body = Object))
)]
pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::View)).await?;
    Ok(Json(c.ops.inspect_container(&id).await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/containers/{id}/spec",
    operation_id = "getContainerSpec",
    tag = "containers",
    summary = "The container as an editable spec, for recreate and duplicate",
    params(EnvIdParam),
    responses((status = 200, description = "Spec", body = ContainerSpec))
)]
pub async fn spec(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<ContainerSpec>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::View)).await?;
    Ok(Json(c.ops.container_spec(&id).await?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/containers/{id}/{action}",
    operation_id = "containerAction",
    tag = "containers",
    summary = "Start, stop, restart, kill, pause or unpause a container",
    params(EnvIdParam, ("action" = ContainerActionName, Path)),
    responses((status = 204, description = "Done"))
)]
pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id, action)): ApiPath<(String, String, String)>,
) -> ApiResult<StatusCode> {
    let action: ContainerAction = action.parse()?;
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Operate)).await?;
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

#[utoipa::path(
    post,
    path = "/envs/{env}/containers/{id}/rename",
    operation_id = "renameContainer",
    tag = "containers",
    summary = "Rename a container",
    params(EnvIdParam),
    request_body = NameBody,
    responses((status = 204, description = "Renamed"))
)]
pub async fn rename(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Operate)).await?;
    let res = c.ops.rename_container(&id, &body.name).await;
    c.audit(&s, "container.rename", "container", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/containers/{id}",
    operation_id = "deleteContainer",
    tag = "containers",
    summary = "Remove a container",
    params(EnvIdParam, ForceQuery),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Delete)).await?;
    let res = c.ops.remove_container(&id, q.force, q.volumes).await;
    c.audit(&s, "container.remove", "container", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/containers/prune",
    operation_id = "pruneContainers",
    tag = "containers",
    summary = "Remove stopped containers",
    params(EnvParam),
    responses((status = 200, description = "What was removed", body = PruneReport))
)]
pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Delete)).await?;
    let res = c.ops.prune_containers().await;
    c.audit(&s, "container.prune", "container", "*", &res).await;
    Ok(Json(res?))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/containers/{id}/recreate",
    operation_id = "recreateContainer",
    tag = "containers",
    summary = "Replace a container with one built from `spec`, keeping the old one if that fails",
    params(EnvIdParam),
    request_body = ContainerSpec,
    responses((status = 200, description = "Recreated", body = RecreateResult))
)]
pub async fn recreate_container(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(spec): ApiJson<ContainerSpec>,
) -> ApiResult<Json<RecreateResult>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Create)).await?;
    let delete = Permission::new(Row::Containers, Verb::Delete);
    crate::authz::deny_audited(
        &s,
        &c.user,
        c.env.id(),
        &delete.to_string(),
        c.grants.check_env(delete),
    )
    .await?;
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

#[utoipa::path(
    post,
    path = "/envs/{env}/containers/{id}/duplicate",
    operation_id = "duplicateContainer",
    tag = "containers",
    summary = "Create and start a copy under a new name",
    params(EnvIdParam),
    request_body = NameBody,
    responses((status = 201, description = "Created", body = Created))
)]
pub async fn duplicate(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Create)).await?;
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

#[utoipa::path(
    get,
    path = "/envs/{env}/containers/{id}/files",
    operation_id = "listContainerFiles",
    tag = "containers",
    summary = "List a directory inside a container",
    params(EnvIdParam, PathQuery),
    responses((status = 200, description = "Directory listing", body = DirListing))
)]
pub async fn list_files(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<PathQuery>,
) -> ApiResult<Json<DirListing>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Reveal)).await?;
    Ok(Json(c.ops.list_dir(&id, &q.path).await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/containers/{id}/files/download",
    operation_id = "downloadContainerFile",
    tag = "containers",
    summary = "Download one file from a container (up to 100 MiB)",
    params(EnvIdParam, PathQuery),
    responses((status = 200, description = "File contents", body = inline(Bytes), content_type = "application/octet-stream"))
)]
pub async fn download_file(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<PathQuery>,
) -> ApiResult<Response> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Reveal)).await?;
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
