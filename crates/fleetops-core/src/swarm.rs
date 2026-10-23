//! Docker Swarm: what a Swarm environment reports and the cluster calls a
//! manager answers. Node-level Docker calls go through `DockerOps`.

use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What an up Swarm environment reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SwarmVersion {
    /// The serving manager's engine version.
    pub engine: String,
    pub cluster_id: String,
    pub managers: u32,
    pub workers: u32,
    /// The manager whose agent answers cluster calls.
    pub serving_node: String,
}

/// The swarm's identity and the root CA its node certificates chain to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmInfo {
    pub cluster_id: String,
    /// PEM.
    pub trust_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    Manager,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeSummary {
    pub id: String,
    pub hostname: String,
    pub role: NodeRole,
    /// `ready`, `down`, `unknown` or `disconnected`.
    pub state: String,
    /// `active`, `pause` or `drain`.
    pub availability: String,
    pub leader: bool,
}

/// Cluster calls, answered by a manager.
#[async_trait]
pub trait SwarmOps: Send + Sync {
    async fn swarm_info(&self) -> Result<SwarmInfo>;
    async fn nodes(&self) -> Result<Vec<NodeSummary>>;
    /// `None` when the swarm has no such node.
    async fn node(&self, id: &str) -> Result<Option<NodeSummary>>;
}
