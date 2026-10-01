//! Agent environments: enrollment tokens, the docker run command, and the
//! status updates the connect handler publishes (sub-project 2).

mod axum_io;
pub mod connect;

use crate::{auth::token::generate_token, ws::hub::StatusChange, AppState};
use fleetops_store::{AgentRecord, EnvironmentRecord};
use serde::Serialize;
use utoipa::ToSchema;

/// How long a join token stays valid (spec §4.1).
pub const JOIN_TTL_SECS: i64 = 900;
pub const AGENT_IMAGE: &str = "ghcr.io/pravinbanjade/fleetops-agent";

pub fn new_join_token() -> String {
    format!("fj_{}", generate_token())
}

pub fn new_agent_key() -> String {
    format!("fk_{}", generate_token())
}

/// The command an admin pastes on the remote host. The image tag is pinned
/// to this server's version.
pub fn enrollment_command(server_url: &str, join_token: &str) -> String {
    format!(
        "docker run -d --name fleetops-agent --restart unless-stopped \\\n  \
         -v fleetops-agent:/data -v /var/run/docker.sock:/var/run/docker.sock \\\n  \
         --group-add \"$(stat -c %g /var/run/docker.sock)\" \\\n  \
         {AGENT_IMAGE}:{} \\\n  \
         --server {server_url} --join-token {join_token}",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Enrollment {
    /// Shown once. Valid for 15 minutes and for one agent.
    pub join_token: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub command: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentView {
    pub connected: bool,
    pub enrolled: bool,
    pub last_seen_at: Option<i64>,
    pub agent_version: Option<String>,
    pub host: Option<serde_json::Value>,
    pub last_error: Option<String>,
    pub join_expires_at: Option<i64>,
}

impl AgentView {
    pub fn new(rec: AgentRecord, connected: bool) -> Self {
        Self {
            connected,
            enrolled: rec.enrolled,
            last_seen_at: rec.last_seen_at,
            agent_version: rec.agent_version,
            host: rec.host_info.and_then(|h| serde_json::from_str(&h).ok()),
            last_error: rec.last_error,
            join_expires_at: rec.join_expires_at,
        }
    }
}

/// An environment as listed: agent environments carry their agent's state.
#[derive(Debug, Serialize, ToSchema)]
pub struct EnvView {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub agent: Option<AgentView>,
}

/// A newly created environment; an agent one carries its enrollment once.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedEnv {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub enrollment: Option<Enrollment>,
}

/// Announces environment `env_id`'s current status to every websocket.
pub async fn publish_status(s: &AppState, env_id: i64) {
    let Ok(rec) = s.store.get_environment(env_id).await else {
        return;
    };
    let Ok(env) = s.envs.get(&rec.name) else {
        return;
    };
    let status = env.status().await;
    let _ = s.hubs.status().send(StatusChange {
        env: rec.name,
        status: serde_json::to_value(status).unwrap_or_default(),
    });
}
