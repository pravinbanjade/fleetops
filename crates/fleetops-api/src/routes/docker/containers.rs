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
use fleetops_core::compose::files::{digest, labelled_files, secret_paths};
use fleetops_core::compose::FileKind;
use fleetops_core::docker::{mask_env, unmask_env};
use fleetops_core::docker::{
    ContainerAction, ContainerSpec, ContainerSummary, DirListing, DockerOps, PruneReport,
};
use fleetops_core::kube::MASK;
use fleetops_core::perm::{Permission, Row, Verb};

const REVEAL: Permission = Permission::new(Row::Containers, Verb::Reveal);
use fleetops_core::recreate::recreate;
use fleetops_core::{now_unix, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
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
    let mut list = c.ops.list_containers().await?;
    list.retain(|x| c.sees(&x.labels));
    Ok(Json(list))
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
    c.whole(&s, Permission::new(Row::Containers, Verb::Create))
        .await?;
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::View), &id)
        .await?;
    let mut v = c.ops.inspect_container(&id).await?;
    // Reveal held on the container's stack shows its values too.
    let stack = v["Config"]["Labels"][fleetops_core::swarm::STACK_LABEL]
        .as_str()
        .map(str::to_string);
    if !c.grants.allows(REVEAL, stack.as_deref()) {
        if let Some(env) = v.pointer_mut("/Config/Env").and_then(Value::as_array_mut) {
            let mut text: Vec<String> = env
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect();
            mask_env(&mut text);
            *env = text.into_iter().map(Value::String).collect();
        }
    }
    Ok(Json(v))
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::View), &id)
        .await?;
    let mut spec = c.ops.container_spec(&id).await?;
    if !c.grants.allows(REVEAL, super::stack_of(&spec.labels)) {
        mask_env(&mut spec.env);
    }
    Ok(Json(spec))
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::Operate), &id)
        .await?;
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::Operate), &id)
        .await?;
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::Delete), &id)
        .await?;
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
    c.whole(&s, Permission::new(Row::Containers, Verb::Delete))
        .await?;
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
    ApiJson(mut spec): ApiJson<ContainerSpec>,
) -> ApiResult<Json<RecreateResult>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Containers, Verb::Create)).await?;
    c.check_container(&s, Permission::new(Row::Containers, Verb::Create), &id)
        .await?;
    let delete = Permission::new(Row::Containers, Verb::Delete);
    crate::authz::deny_audited(
        &s,
        &c.user,
        c.env.id(),
        &delete.to_string(),
        c.grants.check_env(delete),
    )
    .await?;
    refuse_stack_files(c.ops.as_ref(), &id).await?;
    // A spec read without Reveal carries masked values: keep the live ones.
    if spec.env.iter().any(|e| e.ends_with(MASK)) {
        let live = c.ops.container_spec(&id).await?;
        unmask_env(&mut spec.env, &live.env)?;
    }
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
    c.whole(&s, Permission::new(Row::Containers, Verb::Create))
        .await?;
    refuse_stack_files(c.ops.as_ref(), &id).await?;
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::Reveal), &id)
        .await?;
    let secrets = refuse_secret(c.ops.as_ref(), &id, &q.path).await?;
    let mut listing = c.ops.list_dir(&id, &q.path).await?;
    if !secrets.is_empty() {
        let dir = normalise(&q.path);
        listing.entries.retain(|e| {
            let full = normalise(&format!("{dir}/{}", e.name));
            !secrets.contains(&full)
        });
    }
    Ok(Json(listing))
}

async fn labels_of(ops: &dyn DockerOps, id: &str) -> Result<BTreeMap<String, String>> {
    let inspect = ops.inspect_container(id).await?;
    Ok(serde_json::from_value(inspect["Config"]["Labels"].clone()).unwrap_or_default())
}

/// Refuses a file whose content is one of the container's secrets. A path
/// check alone is not enough: Docker follows symlinked directories in an
/// archive path (`/var/run` is `/run` in Debian and Alpine images).
async fn refuse_secret_content(
    s: &AppState,
    ops: &dyn DockerOps,
    id: &str,
    data: &[u8],
) -> Result<()> {
    let Ok(text) = std::str::from_utf8(data) else {
        // Secrets are UTF-8 text.
        return Ok(());
    };
    let key = s.store.file_digest_key();
    let hit = labelled_files(&labels_of(ops, id).await?)
        .iter()
        .any(|f| f.kind == FileKind::Secret && digest(&key, f.kind, &f.name, text) == f.digest);
    if hit {
        return Err(Error::Forbidden("this is a stack secret".into()));
    }
    Ok(())
}

/// A stack container's secrets and configs live in its writable layer, so
/// a copy made outside a deploy would start without them.
async fn refuse_stack_files(ops: &dyn DockerOps, id: &str) -> Result<()> {
    let labels = labels_of(ops, id).await?;
    if labelled_files(&labels).is_empty() {
        return Ok(());
    }
    let stack = labels
        .get(fleetops_core::compose::docker::LABEL_PROJECT)
        .map(String::as_str)
        .unwrap_or("its stack");
    Err(Error::Conflict(format!(
        "this container has secrets or configs from stack '{stack}', which a copy would not get: deploy the stack instead"
    )))
}

/// `path` with `//`, `.`, `..` and a trailing `/` resolved, absolute.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = vec![];
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Refuses a stack secret's path, for every role (stack secrets spec §4);
/// returns the container's secret targets, normalised. They come from the
/// label the deploy set, so containers of older revisions are covered too.
async fn refuse_secret(ops: &dyn DockerOps, id: &str, path: &str) -> Result<Vec<String>> {
    let labels = labels_of(ops, id).await?;
    let secrets: Vec<String> = secret_paths(&labels).iter().map(|p| normalise(p)).collect();
    if secrets.contains(&normalise(path)) {
        return Err(Error::Forbidden("this is a stack secret".into()));
    }
    Ok(secrets)
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
    c.check_container(&s, Permission::new(Row::Containers, Verb::Reveal), &id)
        .await?;
    refuse_secret(c.ops.as_ref(), &id, &q.path).await?;
    let data = c.ops.read_file(&id, &q.path, MAX_DOWNLOAD_BYTES).await?;
    refuse_secret_content(&s, c.ops.as_ref(), &id, &data).await?;
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
