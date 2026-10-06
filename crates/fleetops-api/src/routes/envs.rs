use crate::agents::{self, AgentView, CreatedEnv, Enrollment, EnvView, KubeAccess};
use crate::openapi::EnvParam;
use crate::{
    auth::{extract::CurrentUser, token::hash_token},
    authz::{authorize_global, Action},
    extract::{ApiJson, ApiPath},
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use fleetops_core::env::{validate_env_name, validate_env_url, Credential, EnvKind, EnvStatus};
use fleetops_core::{now_unix, Error};
use fleetops_kube::kubeconfig;
use fleetops_store::EnvironmentRecord;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Why open Kubernetes watches end when their environment is edited or deleted.
const ENV_CHANGED: &str = "the environment was removed or changed";

#[derive(Deserialize, ToSchema)]
pub struct CreateEnv {
    pub name: String,
    pub kind: EnvKind,
    /// The Docker URL of a local environment; other kinds take none.
    #[serde(default)]
    pub url: Option<String>,
    /// kube-config: the kubeconfig, stored encrypted and never returned.
    #[serde(default)]
    pub kubeconfig: Option<String>,
    /// kube-config: the context to use; default the current one.
    #[serde(default)]
    pub context: Option<String>,
    /// kube-config: save without checking that the cluster answers.
    #[serde(default)]
    pub skip_check: bool,
    /// kube-agent: what the agent's ClusterRole allows; default `standard`.
    #[serde(default)]
    pub access: Option<KubeAccess>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateEnv {
    pub name: Option<String>,
    pub url: Option<String>,
    /// kube-config: a replacement kubeconfig.
    #[serde(default)]
    pub kubeconfig: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub skip_check: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct KubeconfigText {
    pub kubeconfig: String,
}

#[derive(Serialize, ToSchema)]
pub struct KubeContexts {
    pub contexts: Vec<String>,
    pub current: Option<String>,
}

/// Validates and minifies a kubeconfig, then checks the cluster answers
/// unless told not to. Runs before the mutation lock: the check can take
/// seconds.
async fn prepare_kubeconfig(
    text: &str,
    context: Option<&str>,
    skip_check: bool,
) -> ApiResult<kubeconfig::Prepared> {
    let p = kubeconfig::prepare(text, context)?;
    if !skip_check {
        kubeconfig::check(&p.yaml).await?;
    }
    Ok(p)
}

fn summary_json(p: &kubeconfig::Prepared) -> serde_json::Value {
    serde_json::to_value(&p.summary).expect("summary serialises")
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
    kind: EnvKind,
    rotate: bool,
) -> ApiResult<Enrollment> {
    let server = server_url(headers)?;
    let token = agents::new_join_token();
    let expires_at = now_unix() + agents::JOIN_TTL_SECS;
    let access: KubeAccess = s
        .store
        .get_agent(env_id)
        .await?
        .and_then(|a| a.access)
        .map(|a| a.parse())
        .transpose()?
        .unwrap_or_default();
    if rotate {
        s.store
            .rotate_agent(env_id, &hash_token(&token), expires_at)
            .await?;
    }
    let command = agents::command_for(kind, &server, &token, access);
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
    let grants = s.store.user_grants(cu.user.id).await?;
    let mut agents: std::collections::HashMap<i64, _> = s
        .store
        .list_agents()
        .await?
        .into_iter()
        .map(|a| (a.environment_id, a))
        .collect();
    let summaries = s.store.env_credential_summaries().await?;
    let views = s
        .store
        .list_environments()
        .await?
        .into_iter()
        .filter_map(|env| {
            let access = grants.get(&env.id)?.clone();
            let agent = agents
                .remove(&env.id)
                .map(|a| AgentView::new(a, s.agents.get(env.id).is_some()));
            let kube = summaries
                .get(&env.id)
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            Some(EnvView {
                env,
                agent,
                kube,
                access: Some(access),
            })
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
    authorize_global(&cu.user, Action::EnvManage)?;
    validate_env_name(&body.name)?;
    if body.access.is_some() && body.kind != EnvKind::KubeAgent {
        return Err(
            Error::Invalid("only a kube-agent environment takes an access level".into()).into(),
        );
    }
    if body.kind.is_kube() && body.url.is_some() {
        return Err(Error::Invalid(
            "a Kubernetes environment takes no URL; paste a kubeconfig".into(),
        )
        .into());
    }
    let prepared = match (body.kind, &body.kubeconfig) {
        (EnvKind::KubeConfig, Some(text)) => {
            Some(prepare_kubeconfig(text, body.context.as_deref(), body.skip_check).await?)
        }
        (EnvKind::KubeConfig, None) => {
            return Err(Error::Invalid("a kubeconfig is required".into()).into())
        }
        (_, Some(_)) => {
            return Err(
                Error::Invalid("only a kube-config environment takes a kubeconfig".into()).into(),
            )
        }
        (_, None) => None,
    };
    let _guard = s.envs.lock_mutations().await;
    let (rec, enrollment, credential) = match body.kind {
        EnvKind::DockerLocal => {
            let url = body
                .url
                .ok_or_else(|| Error::Invalid("a local environment needs a Docker URL".into()))?;
            validate_env_url(&url)?;
            let rec = s
                .store
                .create_environment(&body.name, body.kind.as_str(), &url, now_unix())
                .await?;
            (rec, None, None)
        }
        EnvKind::DockerAgent | EnvKind::KubeAgent => {
            if body.url.is_some() {
                return Err(Error::Invalid("an agent environment takes no URL".into()).into());
            }
            let server = server_url(&headers)?;
            let token = agents::new_join_token();
            let expires_at = now_unix() + agents::JOIN_TTL_SECS;
            let rec = s
                .store
                .create_agent_environment(
                    &body.name,
                    body.kind,
                    &hash_token(&token),
                    expires_at,
                    now_unix(),
                )
                .await?;
            let access = body.access.unwrap_or_default();
            if body.kind == EnvKind::KubeAgent {
                s.store.set_agent_access(rec.id, access.as_str()).await?;
            }
            let command = agents::command_for(body.kind, &server, &token, access);
            let enrollment = Enrollment {
                join_token: token,
                expires_at,
                command,
            };
            (rec, Some(enrollment), None)
        }
        EnvKind::KubeConfig => {
            let p = prepared.expect("checked above");
            let rec = s
                .store
                .create_environment_with_credential(
                    &body.name,
                    body.kind.as_str(),
                    &p.summary.server,
                    "kubeconfig",
                    &p.yaml,
                    &summary_json(&p),
                    now_unix(),
                )
                .await?;
            (rec, None, Some(Credential::Kubeconfig(p.yaml)))
        }
    };
    s.envs.insert(&rec, credential)?;
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
    authorize_global(&cu.user, Action::EnvManage)?;
    let kind: EnvKind = find(&s, &name).await?.kind.parse()?;
    if body.url.is_some() && kind == EnvKind::DockerAgent {
        return Err(Error::Invalid("an agent environment's URL cannot be changed".into()).into());
    }
    if body.url.is_some() && kind.is_kube() {
        return Err(Error::Invalid(
            "a Kubernetes environment's URL comes from its kubeconfig".into(),
        )
        .into());
    }
    if body.kubeconfig.is_some() && kind != EnvKind::KubeConfig {
        return Err(
            Error::Invalid("only a kube-config environment takes a kubeconfig".into()).into(),
        );
    }
    let prepared = match &body.kubeconfig {
        Some(t) => Some(prepare_kubeconfig(t, body.context.as_deref(), body.skip_check).await?),
        None => None,
    };
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    let new_name = body.name.unwrap_or(current.name.clone());
    let new_url = match &prepared {
        Some(p) => p.summary.server.clone(),
        None => body.url.unwrap_or(current.url.clone()),
    };
    validate_env_name(&new_name)?;
    if prepared.is_none() && new_url != current.url {
        validate_env_url(&new_url)?;
    }
    if let Some(p) = &prepared {
        s.store
            .put_env_credential(
                current.id,
                "kubeconfig",
                &p.yaml,
                &summary_json(p),
                now_unix(),
            )
            .await?;
    }
    let rec = s
        .store
        .update_environment(current.id, &new_name, &new_url, now_unix())
        .await?;
    let credential = match prepared {
        Some(p) => Some(Credential::Kubeconfig(p.yaml)),
        None => s.store.env_credential(rec.id).await?,
    };
    s.envs.replace(&current.name, &rec, credential)?;
    s.hubs.kube_watches.end_env(rec.id, ENV_CHANGED);
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
    authorize_global(&cu.user, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    s.store.delete_environment(current.id).await?;
    s.envs.remove(&current.name);
    s.hubs.kube_watches.end_env(current.id, ENV_CHANGED);
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
    let (env, _) = crate::authz::env_access(&s, &cu, &name, Action::EnvRead).await?;
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
    authorize_global(&cu.user, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    let kind: EnvKind = current.kind.parse()?;
    if !matches!(kind, EnvKind::DockerAgent | EnvKind::KubeAgent) {
        return Err(Error::Invalid(format!("'{name}' is not an agent environment")).into());
    }
    let enrollment = new_enrollment(&s, &headers, current.id, kind, true).await?;
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

#[utoipa::path(
    post,
    path = "/kube/contexts",
    operation_id = "kubeContexts",
    tag = "environments",
    summary = "List a kubeconfig's contexts without saving it",
    request_body = KubeconfigText,
    responses((status = 200, description = "Context names and the current one", body = KubeContexts))
)]
pub async fn kube_contexts(
    State(_s): State<AppState>,
    cu: CurrentUser,
    ApiJson(body): ApiJson<KubeconfigText>,
) -> ApiResult<Json<KubeContexts>> {
    authorize_global(&cu.user, Action::EnvManage)?;
    let c = kubeconfig::contexts(&body.kubeconfig)?;
    Ok(Json(KubeContexts {
        contexts: c.contexts,
        current: c.current,
    }))
}
