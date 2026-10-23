//! A Swarm environment's agents (swarm spec §2, §3): which nodes have one,
//! key rotation, and re-trusting the swarm after its root CA changes.

use crate::agents::swarm_manifest::{self, SwarmEnrollment};
use crate::openapi::EnvParam;
use crate::{
    auth::{extract::CurrentUser, token::hash_token},
    authz::{require_admin, Need},
    extract::ApiPath,
    routes::{audit, envs::find, kube::wrong_kind},
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use fleetops_core::env::EnvKind;
use fleetops_core::swarm::NodeRole;
use fleetops_core::{now_unix, Error};
use fleetops_store::EnvironmentRecord;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;
use utoipa::ToSchema;

/// How long the key before a rotation keeps working (swarm spec §3).
pub const SWARM_KEY_GRACE_SECS: i64 = 86_400;
const NODES_TIMEOUT: Duration = Duration::from_secs(5);

/// One node of the swarm and its agent.
#[derive(Debug, Serialize, ToSchema)]
pub struct SwarmAgentView {
    pub node_id: String,
    pub hostname: Option<String>,
    /// Null when no manager answered.
    pub role: Option<NodeRole>,
    /// `ready`, `down`, ...; null when no manager answered.
    pub state: Option<String>,
    pub connected: bool,
    pub agent_version: Option<String>,
    pub last_seen_at: Option<i64>,
    /// Why the agent was refused or went offline.
    pub last_error: Option<String>,
}

async fn swarm_env(s: &AppState, name: &str) -> ApiResult<EnvironmentRecord> {
    let env = find(s, name).await?;
    let kind: EnvKind = env.kind.parse()?;
    if !kind.is_swarm() {
        return Err(wrong_kind(name, kind).into());
    }
    Ok(env)
}

/// The origin the admin's browser used: where agents outside the swarm dial.
fn server_url(headers: &HeaderMap) -> ApiResult<String> {
    headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| o.starts_with("https://") || o.starts_with("http://"))
        .map(str::to_string)
        .ok_or_else(|| {
            Error::Invalid("an Origin header is needed to build the agent stack".into()).into()
        })
}

#[utoipa::path(
    get,
    path = "/envs/{env}/swarm/agents",
    operation_id = "listSwarmAgents",
    tag = "environments",
    summary = "A Swarm environment's nodes and their agents",
    params(EnvParam),
    responses((status = 200, description = "Every node a manager reports, and every node that has had an agent", body = Vec<SwarmAgentView>))
)]
pub async fn agents(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<Vec<SwarmAgentView>>> {
    let (env, _) = crate::authz::env_access(&s, &cu, &name, Need::AnyGrant).await?;
    if !env.kind().is_swarm() {
        return Err(wrong_kind(&name, env.kind()).into());
    }
    let mut rows: BTreeMap<String, SwarmAgentView> = BTreeMap::new();
    if let Some(ops) = env.swarm() {
        if let Ok(Ok(nodes)) = tokio::time::timeout(NODES_TIMEOUT, ops.nodes()).await {
            for n in nodes {
                rows.insert(
                    n.id.clone(),
                    SwarmAgentView {
                        node_id: n.id,
                        hostname: Some(n.hostname),
                        role: Some(n.role),
                        state: Some(n.state),
                        connected: false,
                        agent_version: None,
                        last_seen_at: None,
                        last_error: None,
                    },
                );
            }
        }
    }
    for rec in s.store.list_swarm_agents(env.id()).await? {
        let row = rows
            .entry(rec.node_id.clone())
            .or_insert_with(|| SwarmAgentView {
                node_id: rec.node_id.clone(),
                hostname: None,
                role: None,
                state: None,
                connected: false,
                agent_version: None,
                last_seen_at: None,
                last_error: None,
            });
        row.hostname = row.hostname.take().or(rec.hostname);
        row.agent_version = rec.agent_version;
        row.last_seen_at = rec.last_seen_at;
        row.last_error = rec.last_error;
    }
    for row in rows.values_mut() {
        row.connected = s.agents.get_node(env.id(), &row.node_id).is_some();
        if !row.connected && row.last_error.is_none() {
            row.last_error = s.agents.node_offline_reason(env.id(), &row.node_id);
        }
    }
    Ok(Json(rows.into_values().collect()))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/swarm/rotate",
    operation_id = "rotateSwarmKey",
    tag = "environments",
    summary = "Issue a new Swarm agent key; the old one works for 24 hours or until the rotation is finished",
    params(EnvParam),
    responses((status = 200, description = "The new key, shown once, and the agent stack that uses it", body = SwarmEnrollment))
)]
pub async fn rotate(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<SwarmEnrollment>> {
    require_admin(&cu.user)?;
    let server = server_url(&headers)?;
    let _guard = s.envs.lock_mutations().await;
    let env = swarm_env(&s, &name).await?;
    let key = crate::agents::new_agent_key();
    let (generation, dropped) = s
        .store
        .rotate_swarm_key(env.id, &hash_token(&key), now_unix() + SWARM_KEY_GRACE_SECS)
        .await?;
    // A rotation still pending: its old key is gone now.
    if let Some(dropped) = dropped {
        s.agents.revoke_tag(env.id, &dropped);
    }
    audit(
        &s,
        Some(cu.user.id),
        "agent.rotate",
        "environment",
        &env.name,
        "success",
    )
    .await;
    Ok(Json(swarm_manifest::enrollment(
        key,
        generation,
        &server,
        s.config.agent_ca_pem.as_deref(),
    )))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/swarm/rotate/finish",
    operation_id = "finishSwarmRotation",
    tag = "environments",
    summary = "Stop accepting the Swarm agent key from before the last rotation",
    params(EnvParam),
    responses((status = 204, description = "The old key no longer works; agents still using it are disconnected"))
)]
pub async fn finish_rotation(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<StatusCode> {
    require_admin(&cu.user)?;
    let _guard = s.envs.lock_mutations().await;
    let env = swarm_env(&s, &name).await?;
    if let Some(prev) = s.store.finish_swarm_rotation(env.id).await? {
        s.agents.revoke_tag(env.id, &prev);
    }
    audit(
        &s,
        Some(cu.user.id),
        "agent.rotate.finish",
        "environment",
        &env.name,
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/swarm/retrust",
    operation_id = "retrustSwarm",
    tag = "environments",
    summary = "Forget the pinned swarm and root CA; the next manager agent pins them again",
    params(EnvParam),
    responses((status = 204, description = "Unpinned; every agent of the environment is disconnected"))
)]
pub async fn retrust(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<StatusCode> {
    require_admin(&cu.user)?;
    let _guard = s.envs.lock_mutations().await;
    let env = swarm_env(&s, &name).await?;
    s.store.clear_swarm_cluster(env.id).await?;
    s.agents.revoke(env.id);
    audit(
        &s,
        Some(cu.user.id),
        "swarm.retrust",
        "environment",
        &env.name,
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
