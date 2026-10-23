//! Environments are the things fleetops manages: a local Docker socket, a
//! remote host behind an agent, a Kubernetes cluster, or a Docker Swarm
//! reached through one agent per node.

use crate::kube::{KubeOps, KubeVersion};
use crate::swarm::{SwarmOps, SwarmVersion};
use crate::{docker::DockerOps, docker::EngineVersion, Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EnvKind {
    DockerLocal,
    /// A remote Docker host reached through a `fleetops-agent` tunnel.
    DockerAgent,
    /// A cluster reached directly with a stored kubeconfig.
    KubeConfig,
    /// A cluster reached through an in-cluster `fleetops-agent --kube`.
    KubeAgent,
    /// A Docker Swarm reached through `fleetops-agent --swarm`, one per node.
    SwarmAgent,
    /// One node of a Swarm environment, as a Docker host (`<env>@<node>`).
    /// Never stored: it exists only as a view of its swarm.
    SwarmNode,
}

impl EnvKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvKind::DockerLocal => "docker-local",
            EnvKind::DockerAgent => "docker-agent",
            EnvKind::KubeConfig => "kube-config",
            EnvKind::KubeAgent => "kube-agent",
            EnvKind::SwarmAgent => "swarm-agent",
            EnvKind::SwarmNode => "swarm-node",
        }
    }

    pub fn is_kube(self) -> bool {
        matches!(self, EnvKind::KubeConfig | EnvKind::KubeAgent)
    }

    pub fn is_swarm(self) -> bool {
        matches!(self, EnvKind::SwarmAgent)
    }

    /// Reached through `fleetops-agent` rather than a URL or kubeconfig.
    pub fn is_agent(self) -> bool {
        matches!(
            self,
            EnvKind::DockerAgent | EnvKind::KubeAgent | EnvKind::SwarmAgent
        )
    }
}

impl FromStr for EnvKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "docker-local" => Ok(EnvKind::DockerLocal),
            "docker-agent" => Ok(EnvKind::DockerAgent),
            "kube-config" => Ok(EnvKind::KubeConfig),
            "kube-agent" => Ok(EnvKind::KubeAgent),
            "swarm-agent" => Ok(EnvKind::SwarmAgent),
            other => Err(Error::Invalid(format!(
                "unknown environment kind '{other}'"
            ))),
        }
    }
}

/// What an up environment reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EnvVersion {
    Docker(EngineVersion),
    Kube(KubeVersion),
    Swarm(SwarmVersion),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum EnvStatus {
    Up { version: EnvVersion },
    Down { reason: String },
}

#[async_trait]
pub trait Environment: Send + Sync {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
    fn kind(&self) -> EnvKind;
    async fn status(&self) -> EnvStatus;
    /// Docker operations, when this environment is Docker-kind and its
    /// endpoint can be reached. `None` means "unavailable right now".
    fn docker(&self) -> Option<Arc<dyn DockerOps>>;
    /// Kubernetes operations, for Kubernetes-kind environments that can be
    /// reached.
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        None
    }
    /// Cluster operations, for Swarm environments with a manager's agent
    /// connected.
    fn swarm(&self) -> Option<Arc<dyn SwarmOps>> {
        None
    }
    /// One Swarm node's Docker engine, while that node's agent is connected.
    fn node(&self, _node_id: &str) -> Option<Arc<dyn DockerOps>> {
        None
    }
    /// A Swarm node as an environment of its own (`<env>@<node>`), for the
    /// Docker pages. The same instance for the same node, while it exists.
    fn node_env(&self, _node_id: &str) -> Option<Arc<dyn Environment>> {
        None
    }
    /// Resolves when this environment's reachability may have changed, so
    /// a waiting reconnect can retry at once instead of sleeping out its
    /// backoff. Environments with no such signal never resolve.
    async fn wait_change(&self) {
        std::future::pending::<()>().await
    }
}

/// Everything a factory needs to build one live environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSpec {
    pub id: i64,
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
    pub credential: Option<Credential>,
}

/// Builds live environments from stored records. The server supplies the
/// real factory; tests supply fakes.
pub trait EnvFactory: Send + Sync {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>>;
}

/// Environment names appear in URLs (`/envs/{name}/...`).
pub fn validate_env_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "environment name must be 1-64 letters, digits, '_', '-' or '.'".into(),
        ))
    }
}

pub fn validate_env_url(url: &str) -> Result<()> {
    let rest = ["unix://", "tcp://", "http://", "https://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme));
    match rest {
        Some(r) if !r.is_empty() => Ok(()),
        _ => Err(Error::Invalid(
            "environment URL must be unix://, tcp://, http:// or https://".into(),
        )),
    }
}

/// The name of a Swarm node's view: `<env>@<node>`. `@` is not allowed in
/// environment names, so it cannot collide with one.
pub fn node_view_name(env: &str, node: &str) -> String {
    format!("{env}@{node}")
}

/// `(env, node)` for a node view's name.
pub fn split_node_view(name: &str) -> Option<(&str, &str)> {
    name.split_once('@').filter(|(e, n)| !e.is_empty() && !n.is_empty())
}

/// The URL of an agent environment: made by the server, never typed.
pub fn agent_url(env_id: i64) -> String {
    format!("agent://{env_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_round_trips_and_has_a_server_made_url() {
        assert_eq!(
            "docker-agent".parse::<EnvKind>().unwrap(),
            EnvKind::DockerAgent
        );
        assert_eq!(EnvKind::DockerAgent.as_str(), "docker-agent");
        assert_eq!(agent_url(42), "agent://42");
        // Users cannot type one in.
        assert!(validate_env_url("agent://42").is_err());
    }
}
/// A stored secret an environment is built from, already decrypted by
/// the store. `Unreadable` carries why it could not be decrypted.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Kubeconfig(String),
    Unreadable(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Kubeconfig(_) => f.write_str("Kubeconfig(<redacted>)"),
            Credential::Unreadable(why) => write!(f, "Unreadable({why:?})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserKind {
    Token,
    Cert,
    /// An exec plugin the server runs (spec 3k).
    Exec,
}

/// What may be shown about a stored kubeconfig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeconfigSummary {
    pub server: String,
    pub context: String,
    pub user_kind: UserKind,
    /// Exec users only: the plugin's command name, never its arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_command: Option<String>,
}
