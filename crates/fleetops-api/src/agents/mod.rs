//! Agent environments: enrollment tokens, the docker run command, and the
//! status updates the connect handler publishes (sub-project 2).

mod axum_io;
pub mod connect;
pub mod manifest;
mod swarm_connect;

use crate::{auth::token::generate_token, ws::hub::StatusChange, AppState};
use fleetops_store::{AgentRecord, EnvironmentRecord};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How long a join token stays valid (spec §4.1).
pub const JOIN_TTL_SECS: i64 = 900;
pub const AGENT_IMAGE: &str = "docker.io/fleets/fleetops-agent";

/// What the in-cluster agent's ClusterRole allows (spec §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum KubeAccess {
    ReadOnly,
    #[default]
    Standard,
    ClusterAdmin,
}

impl KubeAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            KubeAccess::ReadOnly => "read-only",
            KubeAccess::Standard => "standard",
            KubeAccess::ClusterAdmin => "cluster-admin",
        }
    }
}

impl std::str::FromStr for KubeAccess {
    type Err = fleetops_core::Error;

    fn from_str(s: &str) -> fleetops_core::Result<Self> {
        match s {
            "read-only" => Ok(KubeAccess::ReadOnly),
            "standard" => Ok(KubeAccess::Standard),
            "cluster-admin" => Ok(KubeAccess::ClusterAdmin),
            _ => Err(fleetops_core::Error::Invalid(format!(
                "unknown access level '{s}'"
            ))),
        }
    }
}

pub fn new_join_token() -> String {
    format!("fj_{}", generate_token())
}

pub fn new_agent_key() -> String {
    format!("fk_{}", generate_token())
}

/// The command an admin pastes on the remote host. The image tag is pinned
/// to this server's version. It replaces an agent already running there,
/// so the same command works after a key rotation; the agent itself falls
/// back to the join token when its stored key is rejected.
pub fn enrollment_command(server_url: &str, join_token: &str) -> String {
    format!(
        "docker rm -f fleetops-agent 2>/dev/null; \\\n\
         docker run -d --name fleetops-agent --restart unless-stopped \\\n  \
         -v fleetops-agent:/data -v /var/run/docker.sock:/var/run/docker.sock \\\n  \
         --group-add \"$(docker run --rm -v /var/run/docker.sock:/s busybox:1.37 stat -c %g /s)\" \\\n  \
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
    /// kube-agent: the access level its manifest grants.
    pub access: Option<String>,
    /// kube-agent: the manifest also reads every kind, for custom resources.
    pub custom_resources: bool,
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
            access: rec.access,
            custom_resources: rec.custom_resources,
        }
    }
}

/// An environment as listed: agent environments carry their agent's state.
#[derive(Debug, Serialize, ToSchema)]
pub struct EnvView {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub agent: Option<AgentView>,
    /// What may be shown of a stored kubeconfig (kube-config environments).
    pub kube: Option<fleetops_core::env::KubeconfigSummary>,
    /// The caller's access here (admins: everything on the whole environment).
    pub access: Option<CallerAccess>,
}

/// The caller's grants on one environment, for the UI to offer only what
/// they allow. A hint: the server checks every request.
#[derive(Debug, Serialize, ToSchema)]
pub struct CallerAccess {
    /// The strongest built-in role among the grants (custom roles count as
    /// their closest built-in).
    pub role: fleetops_core::access::EnvRole,
    /// The custom role's name, when exactly one grant is a custom role.
    pub role_name: Option<String>,
    /// Every namespace any grant covers; null when one covers the whole environment.
    pub namespaces: Option<Vec<String>>,
    pub grants: Vec<GrantView>,
}

/// One grant: where it applies and what it allows.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrantView {
    /// Null: the whole environment.
    pub namespaces: Option<Vec<String>>,
    /// `row.verb` permissions, e.g. `volumes.delete`.
    pub permissions: Vec<String>,
}

impl CallerAccess {
    pub fn of(g: &fleetops_core::access::Grants) -> Self {
        let role =
            g.0.iter()
                .map(|x| x.role)
                .max()
                .unwrap_or(fleetops_core::access::EnvRole::Viewer);
        let customs: Vec<&str> =
            g.0.iter()
                .filter_map(|x| x.custom.as_ref().map(|c| c.name.as_str()))
                .collect();
        let namespaces = if g.0.iter().any(|x| x.namespaces.is_none()) {
            None
        } else {
            let all: std::collections::BTreeSet<&String> =
                g.0.iter()
                    .flat_map(|x| x.namespaces.iter().flatten())
                    .collect();
            Some(all.into_iter().cloned().collect())
        };
        Self {
            role,
            role_name: (customs.len() == 1).then(|| customs[0].to_string()),
            namespaces,
            grants: g
                .0
                .iter()
                .map(|x| GrantView {
                    namespaces: x.namespaces.as_ref().map(|n| n.iter().cloned().collect()),
                    permissions: x.perms.to_strs(),
                })
                .collect(),
        }
    }
}

/// A newly created environment; an agent one carries its enrollment once.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedEnv {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub enrollment: Option<Enrollment>,
}

/// The enrollment command for an agent environment of `kind`.
pub fn command_for(
    kind: fleetops_core::env::EnvKind,
    server_url: &str,
    join_token: &str,
    access: KubeAccess,
    custom_resources: bool,
) -> String {
    match kind {
        fleetops_core::env::EnvKind::KubeAgent => {
            manifest::command(server_url, join_token, access, custom_resources)
        }
        _ => enrollment_command(server_url, join_token),
    }
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
