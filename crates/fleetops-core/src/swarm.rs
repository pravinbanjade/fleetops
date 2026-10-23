//! Docker Swarm: what a Swarm environment reports and the cluster calls a
//! manager answers. Node-level Docker calls go through `DockerOps`.

use crate::streams::{BoxStream, DockerEvent, LogLine, LogOptions};
use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

/// The label `docker stack deploy` (and FleetOps' Swarm stacks) put on a
/// stack's services, networks, secrets and configs.
pub const STACK_LABEL: &str = "com.docker.stack.namespace";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeDetail {
    #[serde(flatten)]
    pub summary: NodeSummary,
    pub labels: BTreeMap<String, String>,
    /// The node's engine version.
    pub engine: String,
    pub os: String,
    pub arch: String,
    pub nano_cpus: i64,
    pub memory_bytes: i64,
    pub addr: String,
    /// Managers only.
    pub manager_addr: Option<String>,
    pub reachability: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ServiceSummary {
    pub id: String,
    pub name: String,
    /// From the stack label, when a stack owns it.
    pub stack: Option<String>,
    /// `replicated`, `global`, `replicated-job` or `global-job`.
    pub mode: String,
    /// Replicated services only.
    pub replicas_desired: Option<u64>,
    pub tasks_running: u64,
    pub tasks_desired: u64,
    /// Without the digest Docker pins it to.
    pub image: String,
    /// `published:target/protocol`.
    pub ports: Vec<String>,
    /// `updating`, `paused`, `completed`, `rollback_started`, ...
    pub update_state: Option<String>,
    /// Unix seconds.
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ServiceDetail {
    #[serde(flatten)]
    pub summary: ServiceSummary,
    pub labels: BTreeMap<String, String>,
    /// Docker's ServiceSpec, verbatim. It never holds secret values: Swarm
    /// keeps those in secrets.
    #[schema(value_type = Object)]
    pub spec: serde_json::Value,
    #[schema(value_type = Option<Object>)]
    pub update_status: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TaskSummary {
    pub id: String,
    pub service_id: String,
    pub service_name: String,
    pub slot: Option<u64>,
    pub node_id: Option<String>,
    pub desired_state: String,
    pub state: String,
    pub message: String,
    pub error: Option<String>,
    pub container_id: Option<String>,
    /// Unix seconds.
    pub updated_at: i64,
    /// Swarm's object version: it grows with every change.
    pub version: u64,
}

/// A secret's metadata. Swarm never returns secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SecretSummary {
    pub id: String,
    pub name: String,
    pub stack: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Names of the services that mount it.
    pub used_by: Vec<String>,
}

/// A config's metadata; its content is read separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConfigSummary {
    pub id: String,
    pub name: String,
    pub stack: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SwarmNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub stack: Option<String>,
    pub attachable: bool,
    pub internal: bool,
    pub encrypted: bool,
    pub subnets: Vec<String>,
    pub used_by: Vec<String>,
}

/// Cluster calls, answered by a manager.
#[async_trait]
pub trait SwarmOps: Send + Sync {
    async fn swarm_info(&self) -> Result<SwarmInfo>;
    async fn nodes(&self) -> Result<Vec<NodeSummary>>;
    /// `None` when the swarm has no such node.
    async fn node(&self, id: &str) -> Result<Option<NodeSummary>>;
    async fn node_detail(&self, id: &str) -> Result<Option<NodeDetail>>;
    async fn services(&self) -> Result<Vec<ServiceSummary>>;
    async fn service(&self, id: &str) -> Result<Option<ServiceDetail>>;
    /// Every task Swarm still holds; callers filter.
    async fn tasks(&self) -> Result<Vec<TaskSummary>>;
    async fn secrets(&self) -> Result<Vec<SecretSummary>>;
    async fn configs(&self) -> Result<Vec<ConfigSummary>>;
    /// Swarm-scoped networks only.
    async fn networks(&self) -> Result<Vec<SwarmNetwork>>;
    fn service_logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    /// The manager's own event stream: cluster events and its local ones.
    fn events(&self) -> BoxStream<DockerEvent>;
}
