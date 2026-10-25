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

/// What a FleetOps service is: `server` or `agent` (self-protection).
pub const ROLE_LABEL: &str = "io.fleetops.role";

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
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ServiceDetail {
    #[serde(flatten)]
    pub summary: ServiceSummary,
    /// Docker's ServiceSpec, verbatim. It never holds secret values: Swarm
    /// keeps those in secrets.
    #[schema(value_type = Object)]
    pub spec: serde_json::Value,
    #[schema(value_type = Option<Object>)]
    pub update_status: Option<serde_json::Value>,
    /// Swarm's object version; an edit names the version it was made from.
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TaskSummary {
    pub id: String,
    pub service_id: String,
    pub service_name: String,
    /// From its container spec's stack label (swarm spec §6).
    pub stack: Option<String>,
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
    pub labels: BTreeMap<String, String>,
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
    pub labels: BTreeMap<String, String>,
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
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Active,
    Pause,
    Drain,
}

/// A change to a node; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeChange {
    pub availability: Option<Availability>,
    /// Promote (`manager`) or demote (`worker`).
    pub role: Option<NodeRole>,
    /// Replaces every label.
    pub labels: Option<BTreeMap<String, String>>,
}

/// A change to a service.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceChange {
    /// A whole new spec, made from `version`.
    Spec {
        spec: serde_json::Value,
        version: u64,
    },
    Scale(u64),
    /// Restart every task with the same spec.
    Redeploy,
    /// Back to Docker's previous spec.
    Rollback,
}

/// A new swarm-scoped (overlay) network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewSwarmNetwork {
    pub name: String,
    /// Standalone containers may join it.
    #[serde(default)]
    pub attachable: bool,
    /// No route outside.
    #[serde(default)]
    pub internal: bool,
    /// IPsec between nodes.
    #[serde(default)]
    pub encrypted: bool,
    /// CIDR, e.g. `10.20.0.0/24`.
    pub subnet: Option<String>,
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
    /// Any network's name by id, such as the swarm's own `host` network,
    /// which [`networks`](Self::networks) lacks; `None` for an unknown id.
    async fn network_name(&self, id: &str) -> Result<Option<String>>;
    fn service_logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    /// The manager's own event stream: cluster events and its local ones.
    fn events(&self) -> BoxStream<DockerEvent>;

    async fn update_node(&self, id: &str, change: &NodeChange) -> Result<()>;
    async fn remove_node(&self, id: &str) -> Result<()>;
    /// `spec` is Docker's ServiceSpec; returns the new service's id.
    async fn create_service(&self, spec: &serde_json::Value) -> Result<String>;
    async fn update_service(&self, id: &str, change: ServiceChange) -> Result<()>;
    async fn remove_service(&self, id: &str) -> Result<()>;
    async fn create_secret(
        &self,
        name: &str,
        data: &[u8],
        labels: &BTreeMap<String, String>,
    ) -> Result<String>;
    async fn remove_secret(&self, id: &str) -> Result<()>;
    async fn create_config(
        &self,
        name: &str,
        data: &[u8],
        labels: &BTreeMap<String, String>,
    ) -> Result<String>;
    /// `None` when the swarm has no such config.
    async fn config_data(&self, id: &str) -> Result<Option<Vec<u8>>>;
    async fn remove_config(&self, id: &str) -> Result<()>;
    /// `labels` and driver `options` come from a stack, never a request.
    async fn create_network(
        &self,
        n: &NewSwarmNetwork,
        labels: &BTreeMap<String, String>,
        options: &BTreeMap<String, String>,
    ) -> Result<String>;
    async fn remove_network(&self, id: &str) -> Result<()>;
}
