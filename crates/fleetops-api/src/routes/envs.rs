use crate::agents::{self, AgentView, CreatedEnv, Enrollment, EnvView};
use crate::openapi::EnvParam;
use crate::{
    auth::{extract::CurrentUser, token::hash_token},
    authz::{authorize, Action},
    extract::{ApiJson, ApiPath},
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use fleetops_core::env::{validate_env_name, validate_env_url, EnvKind, EnvStatus};
use fleetops_core::{now_unix, Error};
use fleetops_store::EnvironmentRecord;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct CreateEnv {
    pub name: String,
    pub kind: EnvKind,
    /// The Docker URL of a local environment; agent environments take none.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateEnv {
    pub name: Option<String>,
    pub url: Option<String>,
}

/// The URL agents should dial: the origin the admin's browser used.
fn server_url(headers: &HeaderMap) -> ApiResult<String> {
    headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| o.starts_with("https://") || o.starts_with("http://"))
        .map(str::to_string)
        .ok_or_else(|| {
            Error::Invalid("an Origin header is needed to build the agent command".into()).into()
        })
}

/// A fresh join token for `env_id`, stored as its only credential when
/// `rotate` is set, with the command that uses it.
async fn new_enrollment(
    s: &AppState,
    headers: &HeaderMap,
    env_id: i64,
    rotate: bool,
) -> ApiResult<Enrollment> {
    let server = server_url(headers)?;
    let token = agents::new_join_token();
    let expires_at = now_unix() + agents::JOIN_TTL_SECS;
    if rotate {
        s.store
            .rotate_agent(env_id, &hash_token(&token), expires_at)
            .await?;
    }
    let command = agents::enrollment_command(&server, &token);
    Ok(Enrollment {
        join_token: token,
        expires_at,
        command,
    })
}

#[utoipa::path(
    get,
    path = "/envs",
    operation_id = "listEnvs",
    tag = "environments",
    summary = "List environments",
    responses((status = 200, description = "All environments; agent environments include their agent's state", body = Vec<EnvView>))
)]
pub async fn list(State(s): State<AppState>, cu: CurrentUser) -> ApiResult<Json<Vec<EnvView>>> {
    authorize(&cu.user, None, Action::EnvRead)?;
    let mut agents: std::collections::HashMap<i64, _> = s
        .store
        .list_agents()
        .await?
        .into_iter()
        .map(|a| (a.environment_id, a))
        .collect();
    let views = s
        .store
        .list_environments()
        .await?
        .into_iter()
        .map(|env| {
            let agent = agents
                .remove(&env.id)
                .map(|a| AgentView::new(a, s.agents.get(env.id).is_some()));
            EnvView { env, agent }
        })
        .collect();
    Ok(Json(views))
}

#[utoipa::path(
    post,
    path = "/envs",
    operation_id = "createEnv",
    tag = "environments",
    summary = "Add an environment",
    request_body = CreateEnv,
    responses((status = 201, description = "Created; an agent environment includes its one-time enrollment", body = CreatedEnv))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateEnv>,
) -> ApiResult<(StatusCode, Json<CreatedEnv>)> {
    authorize(&cu.user, None, Action::EnvManage)?;
    validate_env_name(&body.name)?;
    let _guard = s.envs.lock_mutations().await;
    let (rec, enrollment) = match body.kind {
        EnvKind::DockerLocal => {
            let url = body
                .url
                .ok_or_else(|| Error::Invalid("a local environment needs a Docker URL".into()))?;
            validate_env_url(&url)?;
            let rec = s
                .store
                .create_environment(&body.name, body.kind.as_str(), &url, now_unix())
                .await?;
            (rec, None)
        }
        EnvKind::DockerAgent => {
            if body.url.is_some() {
                return Err(Error::Invalid("an agent environment takes no URL".into()).into());
            }
            let server = server_url(&headers)?;
            let token = agents::new_join_token();
            let expires_at = now_unix() + agents::JOIN_TTL_SECS;
            let rec = s
                .store
                .create_agent_environment(&body.name, &hash_token(&token), expires_at, now_unix())
                .await?;
            let command = agents::enrollment_command(&server, &token);
            let enrollment = Enrollment {
                join_token: token,
                expires_at,
                command,
            };
            (rec, Some(enrollment))
        }
        EnvKind::KubeConfig | EnvKind::KubeAgent => {
            return Err(
                Error::Invalid("Kubernetes environments are not available yet".into()).into(),
            )
        }
    };
    s.envs.insert(&rec, None)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.create",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(CreatedEnv {
            env: rec,
            enrollment,
        }),
    ))
}

async fn find(s: &AppState, name: &str) -> ApiResult<EnvironmentRecord> {
    let env = s.envs.get(name)?;
    Ok(s.store.get_environment(env.id()).await?)
}

#[utoipa::path(
    patch,
    path = "/envs/{env}",
    operation_id = "updateEnv",
    tag = "environments",
    summary = "Rename an environment or change its URL",
    params(EnvParam),
    request_body = UpdateEnv,
    responses((status = 200, description = "Updated", body = EnvironmentRecord))
)]
pub async fn update(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<UpdateEnv>,
) -> ApiResult<Json<EnvironmentRecord>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    if current.kind == EnvKind::DockerAgent.as_str() && body.url.is_some() {
        return Err(Error::Invalid("an agent environment's URL cannot be changed".into()).into());
    }
    let new_name = body.name.unwrap_or(current.name.clone());
    let new_url = body.url.unwrap_or(current.url.clone());
    validate_env_name(&new_name)?;
    if new_url != current.url {
        validate_env_url(&new_url)?;
    }
    let rec = s
        .store
        .update_environment(current.id, &new_name, &new_url, now_unix())
        .await?;
    let credential = s.store.env_credential(rec.id).await?;
    s.envs.replace(&current.name, &rec, credential)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.update",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok(Json(rec))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}",
    operation_id = "deleteEnv",
    tag = "environments",
    summary = "Remove an environment",
    params(EnvParam),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<StatusCode> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    s.store.delete_environment(current.id).await?;
    s.envs.remove(&current.name);
    s.agents.revoke(current.id);
    audit(
        &s,
        Some(cu.user.id),
        "env.delete",
        "environment",
        &current.name,
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/envs/{env}/status",
    operation_id = "getEnvStatus",
    tag = "environments",
    summary = "Whether the endpoint answers, with its engine version",
    params(EnvParam),
    responses((status = 200, description = "Current status", body = EnvStatus))
)]
pub async fn status(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<EnvStatus>> {
    let env = s.envs.get(&name)?;
    authorize(&cu.user, Some(env.id()), Action::EnvRead)?;
    Ok(Json(env.status().await))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/agent/rotate",
    operation_id = "rotateAgent",
    tag = "environments",
    summary = "Revoke an agent's key and issue a new join command",
    params(EnvParam),
    responses((status = 200, description = "The new one-time enrollment", body = Enrollment))
)]
pub async fn rotate_agent(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<Enrollment>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    if current.kind != EnvKind::DockerAgent.as_str() {
        return Err(Error::Invalid(format!("'{name}' is not an agent environment")).into());
    }
    let enrollment = new_enrollment(&s, &headers, current.id, true).await?;
    s.agents.revoke(current.id);
    audit(
        &s,
        Some(cu.user.id),
        "agent.rotate",
        "environment",
        &current.name,
        "success",
    )
    .await;
    Ok(Json(enrollment))
}
