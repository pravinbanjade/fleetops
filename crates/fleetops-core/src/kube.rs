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
/// Largest YAML accepted by apply and replace (spec §8).
pub const APPLY_MAX_BYTES: usize = 1 << 20;
/// Most documents in one apply (spec §8).
pub const APPLY_MAX_DOCS: usize = 100;
/// The field manager every FleetOps write uses.
pub const FIELD_MANAGER: &str = "fleetops";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
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
    Events,
    ReplicaSets,
    HorizontalPodAutoscalers,
    NetworkPolicies,
    PersistentVolumeClaims,
    PersistentVolumes,
    StorageClasses,
    ServiceAccounts,
    Roles,
    RoleBindings,
    ClusterRoles,
    ClusterRoleBindings,
}

impl KubeKind {
    pub const ALL: [KubeKind; 24] = [
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
        KubeKind::Events,
        KubeKind::ReplicaSets,
        KubeKind::HorizontalPodAutoscalers,
        KubeKind::NetworkPolicies,
        KubeKind::PersistentVolumeClaims,
        KubeKind::PersistentVolumes,
        KubeKind::StorageClasses,
        KubeKind::ServiceAccounts,
        KubeKind::Roles,
        KubeKind::RoleBindings,
        KubeKind::ClusterRoles,
        KubeKind::ClusterRoleBindings,
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
            KubeKind::Events => "events",
            KubeKind::ReplicaSets => "replicasets",
            KubeKind::HorizontalPodAutoscalers => "horizontalpodautoscalers",
            KubeKind::NetworkPolicies => "networkpolicies",
            KubeKind::PersistentVolumeClaims => "persistentvolumeclaims",
            KubeKind::PersistentVolumes => "persistentvolumes",
            KubeKind::StorageClasses => "storageclasses",
            KubeKind::ServiceAccounts => "serviceaccounts",
            KubeKind::Roles => "roles",
            KubeKind::RoleBindings => "rolebindings",
            KubeKind::ClusterRoles => "clusterroles",
            KubeKind::ClusterRoleBindings => "clusterrolebindings",
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
            KubeKind::Events => "Event",
            KubeKind::ReplicaSets => "ReplicaSet",
            KubeKind::HorizontalPodAutoscalers => "HorizontalPodAutoscaler",
            KubeKind::NetworkPolicies => "NetworkPolicy",
            KubeKind::PersistentVolumeClaims => "PersistentVolumeClaim",
            KubeKind::PersistentVolumes => "PersistentVolume",
            KubeKind::StorageClasses => "StorageClass",
            KubeKind::ServiceAccounts => "ServiceAccount",
            KubeKind::Roles => "Role",
            KubeKind::RoleBindings => "RoleBinding",
            KubeKind::ClusterRoles => "ClusterRole",
            KubeKind::ClusterRoleBindings => "ClusterRoleBinding",
        }
    }

    pub fn from_kind_name(name: &str) -> Option<KubeKind> {
        KubeKind::ALL.into_iter().find(|k| k.kind_name() == name)
    }

    pub fn cluster_scoped(self) -> bool {
        matches!(
            self,
            KubeKind::Namespaces
                | KubeKind::Nodes
                | KubeKind::PersistentVolumes
                | KubeKind::StorageClasses
                | KubeKind::ClusterRoles
                | KubeKind::ClusterRoleBindings
        )
    }

    pub fn scalable(self) -> bool {
        matches!(self, KubeKind::Deployments | KubeKind::StatefulSets)
    }

    /// Records the cluster writes about other objects: listed and watched,
    /// never created or edited from FleetOps.
    pub fn read_only(self) -> bool {
        self == KubeKind::Events
    }

    /// Can be suspended and resumed: Deployments and StatefulSets by
    /// scaling to zero and back, CronJobs by their own `spec.suspend`.
    pub fn suspendable(self) -> bool {
        matches!(
            self,
            KubeKind::Deployments | KubeKind::StatefulSets | KubeKind::CronJobs
        )
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
pub enum RowHealth {
    Ok,
    Warn,
    Bad,
    Unknown,
    /// Deliberately not running: suspended, so not a failure to chase.
    Off,
}

/// Where a suspended Deployment or StatefulSet keeps the replicas it had,
/// so resuming brings back exactly that many (spec 3f §1).
pub const SUSPENDED_ANNOTATION: &str = "fleetops.io/suspended-replicas";

/// One object in a list: common fields plus columns that depend on its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeRow {
    pub name: String,
    pub namespace: Option<String>,
    /// Unix seconds.
    pub created: i64,
    /// e.g. "Running", "2/3", "Complete".
    pub status: String,
    pub health: RowHealth,
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
    /// `None` when the credentials may not list nodes.
    pub nodes: Option<u32>,
    pub nodes_ready: Option<u32>,
}

/// A server-side narrowing of a list or watch (spec 3e §4). Only Events
/// have one: their warnings, the rows an overview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum KubeFilter {
    Warnings,
}

impl KubeFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            KubeFilter::Warnings => "warnings",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "warnings" => Ok(KubeFilter::Warnings),
            _ => Err(Error::Invalid(format!("unknown filter '{s}'"))),
        }
    }

    /// The kinds the filter applies to.
    pub fn check(self, kind: KubeKind) -> Result<()> {
        if kind == KubeKind::Events {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "the {} filter is for events, not {}",
                self.as_str(),
                kind.as_str()
            )))
        }
    }

    /// The apiserver field selector that does the same.
    pub fn field_selector(self) -> &'static str {
        match self {
            KubeFilter::Warnings => "type=Warning",
        }
    }

    pub fn keeps(self, row: &KubeRow) -> bool {
        match self {
            KubeFilter::Warnings => row.status == "Warning",
        }
    }

    /// A watch event as the filtered list sees it.
    pub fn event(self, ev: WatchEvent) -> Option<WatchEvent> {
        match ev {
            WatchEvent::Snapshot { mut rows } => {
                rows.retain(|r| self.keeps(r));
                Some(WatchEvent::Snapshot { rows })
            }
            WatchEvent::Added { row } if !self.keeps(&row) => None,
            WatchEvent::Modified { row } if !self.keeps(&row) => {
                // Left the filtered set: gone, as far as its viewers know.
                Some(WatchEvent::Deleted {
                    namespace: row.namespace,
                    name: row.name,
                })
            }
            other => Some(other),
        }
    }
}

/// How many objects of one kind there are, or why that is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeCount {
    pub count: Option<u64>,
    /// An `ErrorBody` code (`forbidden`, `upstream`…) when `count` is absent.
    pub error: Option<String>,
}

/// Live CPU and memory of one node or pod, from metrics-server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeUsageRow {
    pub namespace: Option<String>,
    pub name: String,
    /// Millicores in use; a pod's is the sum of its containers.
    pub cpu: u64,
    /// Bytes of working set.
    pub memory: u64,
    /// Nodes only: allocatable millicores, when the node is visible.
    pub cpu_allocatable: Option<u64>,
    /// Nodes only: allocatable bytes.
    pub memory_allocatable: Option<u64>,
}

/// Usage of one kind (nodes or pods). `available` is false when the
/// cluster has no metrics API, which is common and not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeUsage {
    pub available: bool,
    /// Why there are no metrics, when `available` is false.
    pub reason: Option<String>,
    pub rows: Vec<KubeUsageRow>,
}

impl KubeUsage {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
            rows: vec![],
        }
    }
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

/// One change to a watched list. A watch that fails for good ends its
/// stream with `Err` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WatchEvent {
    /// Every row, replacing what the client holds.
    Snapshot {
        rows: Vec<KubeRow>,
    },
    Added {
        row: KubeRow,
    },
    Modified {
        row: KubeRow,
    },
    Deleted {
        namespace: Option<String>,
        name: String,
    },
    /// The upstream watch restarted; a `Snapshot` follows.
    Resync,
}

/// An object as the YAML editor opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EditableObject {
    /// `managedFields`, `status` and the last-applied annotation removed;
    /// Secret values masked.
    pub yaml: String,
    /// Sent back with the edit; a newer version on the server is a conflict.
    pub resource_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    DryRun,
    Commit { force: bool },
}

impl ApplyMode {
    pub fn dry_run(self) -> bool {
        matches!(self, ApplyMode::DryRun)
    }

    pub fn force(self) -> bool {
        matches!(self, ApplyMode::Commit { force: true })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApplyAction {
    Created,
    Configured,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyError {
    /// An `ErrorBody` code: `conflict` for a field-manager conflict,
    /// `forbidden`, `invalid`, `not_found`, `upstream`…
    pub code: String,
    /// Kubernetes' message, verbatim.
    pub message: String,
}

impl From<&Error> for ApplyError {
    fn from(e: &Error) -> Self {
        Self {
            code: e.code().into(),
            message: e.detail(),
        }
    }
}

/// What happened to one document of an apply or edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyResult {
    /// Position among the submitted non-empty documents, from 0.
    pub index: usize,
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    /// `None` when the document failed.
    pub action: Option<ApplyAction>,
    /// The live object before the write, cleaned and masked; `None` when created.
    pub before: Option<String>,
    /// The object as written (or as it would be, for a dry run).
    pub after: Option<String>,
    pub error: Option<ApplyError>,
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
    /// Suspends (`on`) or resumes a suspendable object (spec 3f §1).
    async fn suspend(&self, kind: KubeKind, _ns: &str, _name: &str, _on: bool) -> Result<()> {
        Err(Error::Invalid(format!(
            "{} cannot be suspended",
            kind.as_str()
        )))
    }
    /// Cordons (`on`: no new pods are scheduled there) or uncordons a node,
    /// as `kubectl cordon` and `uncordon` do.
    async fn cordon(&self, _node: &str, _on: bool) -> Result<()> {
        Err(Error::Invalid("only nodes can be cordoned".into()))
    }
    /// Starts a Job from a CronJob's template now, as `kubectl create job
    /// --from=cronjob/…` does. Returns the new Job's name.
    async fn trigger(&self, _ns: &str, _cronjob: &str) -> Result<String> {
        Err(Error::Invalid("only cronjobs can be run now".into()))
    }
    /// `opts.container` `None` means the pod's first container.
    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec>;
    /// Rows of `kind`, live: a `Snapshot`, then changes. Ends with `Err`
    /// when the watch cannot continue (spec §2.3).
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent>;
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject>;
    /// Server-side apply of every document in `yaml`, in order. `Err` only
    /// when nothing could be attempted; per-document failures are in the results.
    /// With `allowed`, a document that is cluster-scoped or names a namespace
    /// outside it is refused (code `forbidden`) without being sent, dry run
    /// included.
    async fn apply(
        &self,
        yaml: &str,
        default_ns: &str,
        mode: ApplyMode,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<Vec<ApplyResult>>;
    /// Replaces one object, refusing with `Error::Conflict` when it changed
    /// since `resource_version`.
    async fn replace(
        &self,
        kind: KubeKind,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult>;
    /// The rows `filter` keeps. The default lists everything and filters
    /// here; a client that can ask the apiserver to filter should.
    async fn list_filtered(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
        filter: KubeFilter,
    ) -> Result<KubeList> {
        let mut l = self.list(kind, ns).await?;
        l.rows.retain(|r| filter.keeps(r));
        Ok(l)
    }
    /// `watch`, narrowed by `filter`; the default filters here.
    fn watch_filtered(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
        filter: KubeFilter,
    ) -> BoxStream<WatchEvent> {
        use futures_util::StreamExt;
        Box::pin(self.watch(kind, ns).filter_map(move |r| async move {
            match r {
                Ok(ev) => filter.event(ev).map(Ok),
                Err(e) => Some(Err(e)),
            }
        }))
    }
    /// How many objects of `kind` there are (spec 3e §5), without their
    /// bodies where the client can manage it. The default counts a list.
    async fn count(&self, kind: KubeKind, ns: Option<&str>) -> Result<u64> {
        Ok(self.list(kind, ns).await?.rows.len() as u64)
    }
    /// CPU and memory now, for `Nodes` or `Pods` (spec 3e §3). `ns` limits
    /// pods to one namespace.
    async fn usage(&self, _kind: KubeKind, _ns: Option<&str>) -> Result<KubeUsage> {
        Ok(KubeUsage::unavailable(
            "metrics are not available for this environment",
        ))
    }
    /// Helm releases at their latest revision; `None` is every namespace.
    async fn helm_list(&self, _ns: Option<&str>) -> Result<Vec<crate::helm::HelmRelease>> {
        Err(helm_unsupported())
    }
    /// One revision (the latest when `None`), masked.
    async fn helm_get(
        &self,
        _ns: &str,
        _name: &str,
        _revision: Option<u32>,
    ) -> Result<crate::helm::ReleaseDetail> {
        Err(helm_unsupported())
    }
    /// Every stored revision, oldest first.
    async fn helm_history(&self, _ns: &str, _name: &str) -> Result<Vec<crate::helm::HelmRevision>> {
        Err(helm_unsupported())
    }
    /// Rolls back to `revision` (spec 3d §3). A dry run plans; a commit
    /// refuses with `Conflict(CHANGED)` unless the latest revision is still
    /// `expected_current`. With `allowed`, any object cluster-scoped or
    /// outside it refuses the whole action before anything is sent.
    async fn helm_rollback(
        &self,
        _ns: &str,
        _name: &str,
        _revision: u32,
        _expected_current: u32,
        _dry_run: bool,
        _allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<crate::helm::HelmPlan> {
        Err(helm_unsupported())
    }
    /// Uninstalls (spec 3d §4), with the same checks as `helm_rollback`.
    async fn helm_uninstall(
        &self,
        _ns: &str,
        _name: &str,
        _expected_current: u32,
        _dry_run: bool,
        _allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<crate::helm::HelmPlan> {
        Err(helm_unsupported())
    }
}

fn helm_unsupported() -> Error {
    Error::Invalid("Helm is not available for this environment".into())
}

#[cfg(test)]
mod watch_and_apply_tests {
    use super::*;
    use serde_json::json;

    fn row(name: &str) -> KubeRow {
        KubeRow {
            name: name.into(),
            namespace: Some("apps".into()),
            created: 1,
            status: "Running".into(),
            health: RowHealth::Ok,
            columns: Default::default(),
        }
    }

    #[test]
    fn watch_events_are_tagged_by_type() {
        assert_eq!(
            serde_json::to_value(WatchEvent::Snapshot {
                rows: vec![row("a")]
            })
            .unwrap()["type"],
            "snapshot"
        );
        assert_eq!(
            serde_json::to_value(WatchEvent::Deleted {
                namespace: Some("apps".into()),
                name: "a".into()
            })
            .unwrap(),
            json!({"type": "deleted", "namespace": "apps", "name": "a"})
        );
        assert_eq!(
            serde_json::to_value(WatchEvent::Resync).unwrap(),
            json!({"type": "resync"})
        );
    }

    #[test]
    fn apply_mode_says_whether_it_writes_and_forces() {
        assert!(ApplyMode::DryRun.dry_run() && !ApplyMode::DryRun.force());
        let c = ApplyMode::Commit { force: true };
        assert!(!c.dry_run() && c.force());
    }

    #[test]
    fn an_apply_error_keeps_the_code_and_the_bare_message() {
        let e = ApplyError::from(&Error::Conflict(
            "field manager kubectl owns .spec.replicas".into(),
        ));
        assert_eq!(e.code, "conflict");
        assert_eq!(e.message, "field manager kubectl owns .spec.replicas");
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_url_name_and_kind_name() {
        assert_eq!(KubeKind::ALL.len(), 24);
        for k in KubeKind::ALL {
            assert_eq!(k.as_str().parse::<KubeKind>().unwrap(), k);
            assert_eq!(KubeKind::from_kind_name(k.kind_name()), Some(k));
        }
        assert_eq!(
            KubeKind::HorizontalPodAutoscalers.as_str(),
            "horizontalpodautoscalers"
        );
        assert_eq!(
            KubeKind::ClusterRoleBindings.kind_name(),
            "ClusterRoleBinding"
        );
    }

    #[test]
    fn the_new_cluster_scoped_kinds_say_so() {
        for k in [
            KubeKind::PersistentVolumes,
            KubeKind::StorageClasses,
            KubeKind::ClusterRoles,
            KubeKind::ClusterRoleBindings,
        ] {
            assert!(k.cluster_scoped(), "{k:?}");
        }
        for k in [
            KubeKind::PersistentVolumeClaims,
            KubeKind::Roles,
            KubeKind::ReplicaSets,
        ] {
            assert!(!k.cluster_scoped(), "{k:?}");
        }
    }
}
