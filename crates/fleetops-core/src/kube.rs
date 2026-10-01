//! Kubernetes domain types and the operations a Kubernetes environment
//! offers. No kube-rs here: `fleetops-kube` implements `KubeOps`.

use crate::streams::{BoxStream, ExecOptions, LogLine, LogOptions};
use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};
use tokio::sync::{mpsc, oneshot};
use utoipa::ToSchema;

/// Most rows one list returns; more sets `truncated` (spec §5).
pub const LIST_CAP: usize = 5000;
/// Rows asked of the apiserver per page.
pub const PAGE_SIZE: u32 = 500;
/// Largest YAML document returned; longer is cut with a note.
pub const YAML_CAP: usize = 1 << 20;
/// Shown in place of every Secret value.
pub const MASK: &str = "••••";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum KubeKind {
    Namespaces,
    Nodes,
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
    CronJobs,
    Pods,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
}

impl KubeKind {
    pub const ALL: [KubeKind; 12] = [
        KubeKind::Namespaces,
        KubeKind::Nodes,
        KubeKind::Deployments,
        KubeKind::StatefulSets,
        KubeKind::DaemonSets,
        KubeKind::Jobs,
        KubeKind::CronJobs,
        KubeKind::Pods,
        KubeKind::Services,
        KubeKind::Ingresses,
        KubeKind::ConfigMaps,
        KubeKind::Secrets,
    ];

    /// The plural lowercase name used in URLs.
    pub fn as_str(self) -> &'static str {
        match self {
            KubeKind::Namespaces => "namespaces",
            KubeKind::Nodes => "nodes",
            KubeKind::Deployments => "deployments",
            KubeKind::StatefulSets => "statefulsets",
            KubeKind::DaemonSets => "daemonsets",
            KubeKind::Jobs => "jobs",
            KubeKind::CronJobs => "cronjobs",
            KubeKind::Pods => "pods",
            KubeKind::Services => "services",
            KubeKind::Ingresses => "ingresses",
            KubeKind::ConfigMaps => "configmaps",
            KubeKind::Secrets => "secrets",
        }
    }

    /// The Kubernetes `kind`, e.g. `Deployment`.
    pub fn kind_name(self) -> &'static str {
        match self {
            KubeKind::Namespaces => "Namespace",
            KubeKind::Nodes => "Node",
            KubeKind::Deployments => "Deployment",
            KubeKind::StatefulSets => "StatefulSet",
            KubeKind::DaemonSets => "DaemonSet",
            KubeKind::Jobs => "Job",
            KubeKind::CronJobs => "CronJob",
            KubeKind::Pods => "Pod",
            KubeKind::Services => "Service",
            KubeKind::Ingresses => "Ingress",
            KubeKind::ConfigMaps => "ConfigMap",
            KubeKind::Secrets => "Secret",
        }
    }

    pub fn from_kind_name(name: &str) -> Option<KubeKind> {
        KubeKind::ALL.into_iter().find(|k| k.kind_name() == name)
    }

    pub fn cluster_scoped(self) -> bool {
        matches!(self, KubeKind::Namespaces | KubeKind::Nodes)
    }

    pub fn scalable(self) -> bool {
        matches!(self, KubeKind::Deployments | KubeKind::StatefulSets)
    }

    pub fn restartable(self) -> bool {
        matches!(
            self,
            KubeKind::Deployments | KubeKind::StatefulSets | KubeKind::DaemonSets
        )
    }
}

impl FromStr for KubeKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        KubeKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| Error::NotFound(format!("unknown kind '{s}'")))
    }
}

/// The status dot's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warn,
    Bad,
    Unknown,
}

/// One object in a list: common fields plus columns that depend on its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeRow {
    pub name: String,
    pub namespace: Option<String>,
    /// Unix seconds.
    pub created: i64,
    /// e.g. "Running", "2/3", "Complete".
    pub status: String,
    pub health: Health,
    /// Kind-specific: ready, restarts, node, type, ports, hosts…
    pub columns: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeList {
    pub rows: Vec<KubeRow>,
    /// More objects exist than `LIST_CAP`.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

/// An owner, e.g. a Pod's ReplicaSet. `link` is set when FleetOps has a
/// page for the owner's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeRef {
    pub kind: String,
    pub name: String,
    pub link: Option<KubeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeObject {
    pub row: KubeRow,
    /// `managedFields` stripped; Secret values masked.
    pub yaml: String,
    pub conditions: Vec<KubeCondition>,
    /// Nearest first: Pod → ReplicaSet → Deployment.
    pub owners: Vec<KubeRef>,
    /// Pods only: container names, first is the default.
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeEvent {
    /// "Normal" or "Warning".
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i32,
    /// Unix seconds.
    pub last_seen: Option<i64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeVersion {
    pub git_version: String,
    pub platform: String,
    pub nodes: u32,
    pub nodes_ready: u32,
}

/// A running TTY exec in a pod.
pub struct KubeExec {
    /// Raw TTY output; slow reads are backpressure.
    pub output: BoxStream<Vec<u8>>,
    /// Dropping it closes stdin.
    pub input: mpsc::Sender<Vec<u8>>,
    /// (cols, rows).
    pub resize: mpsc::Sender<(u16, u16)>,
    /// Resolves when the process ends; `None` when the code is unknown.
    pub exit_code: oneshot::Receiver<Option<i64>>,
}

#[async_trait]
pub trait KubeOps: Send + Sync {
    async fn version(&self) -> Result<KubeVersion>;
    /// `ns` is ignored for cluster-scoped kinds; `None` lists all namespaces.
    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList>;
    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject>;
    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>>;
    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()>;
    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    /// `opts.container` `None` means the pod's first container.
    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec>;
}
