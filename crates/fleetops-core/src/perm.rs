//! Permissions (custom roles spec, "The grid"): what a grant allows, as
//! resource-type rows × verb columns. Only the cells the grid marks valid
//! exist. Built-in roles are fixed sets of them.

use crate::access::EnvRole;
use crate::kube::KubeKind;
use crate::{Error, Result};
use std::fmt;
use std::str::FromStr;

/// A resource type: one row of the grid. Keys are stored in roles, so they
/// never change once shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Row {
    Containers,
    Images,
    Volumes,
    Networks,
    Stacks,
    Workloads,
    Pods,
    ConfigMaps,
    Secrets,
    Networking,
    Storage,
    Helm,
    K8sOther,
    SwarmNodes,
    SwarmServices,
    SwarmSecrets,
    SwarmConfigs,
}

impl Row {
    pub const ALL: [Row; 17] = [
        Row::Containers,
        Row::Images,
        Row::Volumes,
        Row::Networks,
        Row::Stacks,
        Row::Workloads,
        Row::Pods,
        Row::ConfigMaps,
        Row::Secrets,
        Row::Networking,
        Row::Storage,
        Row::Helm,
        Row::K8sOther,
        Row::SwarmNodes,
        Row::SwarmServices,
        Row::SwarmSecrets,
        Row::SwarmConfigs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Row::Containers => "containers",
            Row::Images => "images",
            Row::Volumes => "volumes",
            Row::Networks => "networks",
            Row::Stacks => "stacks",
            Row::Workloads => "workloads",
            Row::Pods => "pods",
            Row::ConfigMaps => "configmaps",
            Row::Secrets => "secrets",
            Row::Networking => "networking",
            Row::Storage => "storage",
            Row::Helm => "helm",
            Row::K8sOther => "k8s-other",
            Row::SwarmNodes => "swarm-nodes",
            Row::SwarmServices => "swarm-services",
            Row::SwarmSecrets => "swarm-secrets",
            Row::SwarmConfigs => "swarm-configs",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Row::Containers => "Containers",
            Row::Images => "Images",
            Row::Volumes => "Volumes",
            Row::Networks => "Networks",
            Row::Stacks => "Stacks (compose)",
            Row::Workloads => "Workloads",
            Row::Pods => "Pods",
            Row::ConfigMaps => "ConfigMaps",
            Row::Secrets => "Secrets",
            Row::Networking => "Networking",
            Row::Storage => "Storage",
            Row::Helm => "Helm releases",
            Row::K8sOther => "Everything else",
            Row::SwarmNodes => "Nodes",
            Row::SwarmServices => "Services & tasks",
            Row::SwarmSecrets => "Secrets (swarm)",
            Row::SwarmConfigs => "Configs",
        }
    }

    /// `docker`, `kubernetes` or `swarm`.
    pub fn group(self) -> &'static str {
        match self {
            Row::Containers | Row::Images | Row::Volumes | Row::Networks | Row::Stacks => "docker",
            Row::SwarmNodes | Row::SwarmServices | Row::SwarmSecrets | Row::SwarmConfigs => "swarm",
            _ => "kubernetes",
        }
    }

    /// The verbs this row has a cell for.
    pub fn verbs(self) -> &'static [Verb] {
        use Verb::*;
        match self {
            Row::Containers => &[View, Create, Operate, Delete, Shell, Reveal],
            Row::Images | Row::Volumes => &[View, Create, Delete],
            Row::Networks => &[View, Create, Operate, Delete],
            Row::Stacks => &[View, Create, Operate, Delete, Reveal],
            Row::Pods => &[View, Create, Operate, Delete, Shell],
            Row::Secrets | Row::Helm => &[View, Create, Operate, Delete, Reveal],
            Row::SwarmNodes => &[View, Operate, Delete],
            Row::SwarmServices => &[View, Create, Operate, Delete],
            Row::SwarmSecrets => &[View, Create, Delete],
            Row::SwarmConfigs => &[View, Create, Delete, Reveal],
            Row::Workloads | Row::ConfigMaps | Row::Networking | Row::Storage | Row::K8sOther => {
                &[View, Create, Operate, Delete]
            }
        }
    }
}

/// What is done to a resource: one column of the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verb {
    View,
    Create,
    Operate,
    Delete,
    Shell,
    Reveal,
}

impl Verb {
    pub const ALL: [Verb; 6] = [
        Verb::View,
        Verb::Create,
        Verb::Operate,
        Verb::Delete,
        Verb::Shell,
        Verb::Reveal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Verb::View => "view",
            Verb::Create => "create",
            Verb::Operate => "operate",
            Verb::Delete => "delete",
            Verb::Shell => "shell",
            Verb::Reveal => "reveal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verb::View => "View",
            Verb::Create => "Create",
            Verb::Operate => "Operate",
            Verb::Delete => "Delete",
            Verb::Shell => "Shell",
            Verb::Reveal => "Reveal",
        }
    }
}

/// One cell, written `row.verb` (for example `volumes.delete`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Permission {
    pub row: Row,
    pub verb: Verb,
}

impl Permission {
    pub const fn new(row: Row, verb: Verb) -> Self {
        Self { row, verb }
    }

    pub fn valid(self) -> bool {
        self.row.verbs().contains(&self.verb)
    }

    fn bit(self) -> u128 {
        1 << (self.row as u32 * Verb::ALL.len() as u32 + self.verb as u32)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.row.as_str(), self.verb.as_str())
    }
}

impl FromStr for Permission {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::Invalid(format!("unknown permission '{s}'"));
        let (r, v) = s.split_once('.').ok_or_else(bad)?;
        let row = Row::ALL
            .into_iter()
            .find(|x| x.as_str() == r)
            .ok_or_else(bad)?;
        let verb = Verb::ALL
            .into_iter()
            .find(|x| x.as_str() == v)
            .ok_or_else(bad)?;
        let p = Permission { row, verb };
        if p.valid() {
            Ok(p)
        } else {
            Err(bad())
        }
    }
}

/// A set of valid permissions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PermSet(u128);

impl PermSet {
    pub fn with(mut self, p: Permission) -> Self {
        debug_assert!(p.valid(), "{p}");
        self.0 |= p.bit();
        self
    }

    pub fn contains(&self, p: Permission) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn is_subset(&self, other: &PermSet) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn union(self, other: PermSet) -> PermSet {
        PermSet(self.0 | other.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Every permission in the set, in grid order.
    pub fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        Row::ALL
            .into_iter()
            .flat_map(|r| r.verbs().iter().map(move |&v| Permission::new(r, v)))
            .filter(|p| self.contains(*p))
    }

    pub fn all_valid() -> PermSet {
        Row::ALL
            .into_iter()
            .flat_map(|r| r.verbs().iter().map(move |&v| Permission::new(r, v)))
            .fold(PermSet::default(), PermSet::with)
    }

    pub fn from_strs(v: &[String]) -> Result<PermSet> {
        v.iter()
            .map(|s| s.parse::<Permission>())
            .try_fold(PermSet::default(), |set, p| Ok(set.with(p?)))
    }

    /// Sorted `row.verb` strings.
    pub fn to_strs(&self) -> Vec<String> {
        let mut v: Vec<String> = self.iter().map(|p| p.to_string()).collect();
        v.sort();
        v
    }
}

impl FromIterator<Permission> for PermSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(it: I) -> Self {
        it.into_iter().fold(PermSet::default(), PermSet::with)
    }
}

impl EnvRole {
    /// The built-in role as a fixed set (spec, "Built-in roles as permission sets").
    pub fn perms(self) -> PermSet {
        let all = PermSet::all_valid();
        let secrets_reveal = Permission::new(Row::Secrets, Verb::Reveal);
        match self {
            EnvRole::Viewer => all
                .iter()
                .filter(|p| p.verb == Verb::View)
                .collect::<PermSet>()
                .with(Permission::new(Row::Containers, Verb::Reveal))
                .with(Permission::new(Row::Helm, Verb::Reveal))
                // Stack variables reach container env, which a Viewer may
                // already reveal.
                .with(Permission::new(Row::Stacks, Verb::Reveal))
                // Configs are not secret material, like ConfigMaps.
                .with(Permission::new(Row::SwarmConfigs, Verb::Reveal)),
            EnvRole::Operator => all.iter().filter(|p| *p != secrets_reveal).collect(),
        }
    }
}

/// What an admin may do in every environment: everything.
pub fn admin_perms() -> PermSet {
    PermSet::all_valid()
}

/// The grid row a curated kind belongs to.
pub fn row_of(kind: KubeKind) -> Row {
    use KubeKind::*;
    match kind {
        Deployments
        | StatefulSets
        | DaemonSets
        | ReplicaSets
        | Jobs
        | CronJobs
        | HorizontalPodAutoscalers => Row::Workloads,
        Pods => Row::Pods,
        ConfigMaps => Row::ConfigMaps,
        Secrets => Row::Secrets,
        Services | Ingresses | NetworkPolicies => Row::Networking,
        PersistentVolumeClaims | PersistentVolumes | StorageClasses => Row::Storage,
        Namespaces | Nodes | Events | ServiceAccounts | Roles | RoleBindings | ClusterRoles
        | ClusterRoleBindings => Row::K8sOther,
    }
}

/// The row of a document's `kind` (e.g. `Deployment`); unknown and custom
/// kinds are "everything else".
pub fn row_of_kind_name(kind: &str) -> Row {
    KubeKind::from_kind_name(kind).map_or(Row::K8sOther, row_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Permission {
        s.parse().unwrap()
    }

    #[test]
    fn valid_cells_match_the_grid() {
        assert_eq!(PermSet::all_valid().iter().count(), 70);
        for bad in ["images.operate", "stacks.shell", "pods.reveal"] {
            assert!(bad.parse::<Permission>().is_err(), "{bad}");
        }
        for good in ["pods.shell", "secrets.reveal"] {
            assert!(good.parse::<Permission>().is_ok(), "{good}");
        }
    }

    #[test]
    fn stacks_verbs() {
        use Verb::*;
        assert_eq!(
            Row::Stacks.verbs(),
            &[View, Create, Operate, Delete, Reveal]
        );
    }

    #[test]
    fn viewer_has_stacks_view_and_reveal() {
        let v = EnvRole::Viewer.perms();
        assert!(v.contains(p("stacks.view")) && v.contains(p("stacks.reveal")));
        assert!(!v.contains(p("stacks.create")) && !v.contains(p("stacks.operate")));
    }

    #[test]
    fn operator_has_all_stack_verbs() {
        let o = EnvRole::Operator.perms();
        for s in ["view", "create", "operate", "delete", "reveal"] {
            assert!(o.contains(p(&format!("stacks.{s}"))), "{s}");
        }
    }

    #[test]
    fn bit_layout_unchanged() {
        // Stored role sets are bitmasks; adding verbs to a row must not move
        // existing cells.
        assert_eq!(p("containers.view").bit(), 1);
        assert_eq!(p("stacks.view").bit(), 1 << (4 * Verb::ALL.len()));
    }

    #[test]
    fn swarm_rows_append_after_k8s_other() {
        assert_eq!(p("swarm-nodes.view").bit(), 1 << (13 * Verb::ALL.len()));
        assert_eq!(Row::SwarmNodes.group(), "swarm");
        for bad in ["swarm-nodes.create", "swarm-secrets.reveal"] {
            assert!(bad.parse::<Permission>().is_err(), "{bad}");
        }
    }

    #[test]
    fn builtin_swarm_cells() {
        let v = EnvRole::Viewer.perms();
        for c in [
            "swarm-nodes.view",
            "swarm-services.view",
            "swarm-secrets.view",
            "swarm-configs.view",
            "swarm-configs.reveal",
        ] {
            assert!(v.contains(p(c)), "{c}");
        }
        assert!(!v.contains(p("swarm-services.operate")));
        assert!(EnvRole::Operator.perms().contains(p("swarm-nodes.delete")));
    }

    #[test]
    fn strings_round_trip_and_reject_invalid() {
        assert_eq!(p("volumes.delete").to_string(), "volumes.delete");
        let e = "volumes.shell".parse::<Permission>().unwrap_err();
        assert_eq!(
            e.to_string(),
            crate::Error::Invalid("unknown permission 'volumes.shell'".into()).to_string()
        );
        assert!("nope.view".parse::<Permission>().is_err());
        let set = PermSet::from_strs(&["pods.view".into(), "containers.delete".into()]).unwrap();
        assert_eq!(set.to_strs(), ["containers.delete", "pods.view"]);
        assert!(PermSet::from_strs(&["x".into()]).is_err());
    }

    #[test]
    fn builtin_sets() {
        let v = EnvRole::Viewer.perms();
        assert!(
            v.contains(p("pods.view"))
                && v.contains(p("containers.reveal"))
                && v.contains(p("helm.reveal"))
        );
        assert!(!v.contains(p("secrets.reveal")) && !v.contains(p("containers.delete")));
        let o = EnvRole::Operator.perms();
        assert!(o.contains(p("containers.delete")) && o.contains(p("pods.shell")));
        assert!(!o.contains(p("secrets.reveal")));
        assert!(admin_perms().contains(p("secrets.reveal")));
        assert!(v.is_subset(&o) && o.is_subset(&admin_perms()));
        assert_eq!(admin_perms(), PermSet::all_valid());
    }

    #[test]
    fn kind_rows() {
        use KubeKind::*;
        for k in KubeKind::ALL {
            let want = match k {
                Deployments
                | StatefulSets
                | DaemonSets
                | ReplicaSets
                | Jobs
                | CronJobs
                | HorizontalPodAutoscalers => Row::Workloads,
                Pods => Row::Pods,
                ConfigMaps => Row::ConfigMaps,
                Secrets => Row::Secrets,
                Services | Ingresses | NetworkPolicies => Row::Networking,
                PersistentVolumeClaims | PersistentVolumes | StorageClasses => Row::Storage,
                _ => Row::K8sOther,
            };
            assert_eq!(row_of(k), want, "{k:?}");
            assert_eq!(row_of_kind_name(k.kind_name()), want, "{k:?}");
        }
        assert_eq!(row_of_kind_name("Certificate"), Row::K8sOther);
    }
}
