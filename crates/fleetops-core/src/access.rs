//! Per-environment access (spec §2.2): what a user may do in one
//! environment, and for Kubernetes, in which namespaces. Pure: the store
//! produces grants, the API enforces them.

use crate::kube::{KubeKind, KubeList, WatchEvent};
use crate::perm::{row_of, PermSet, Permission, Row, Verb};
use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use utoipa::ToSchema;

pub const NAMESPACES_MAX: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvRole {
    /// Read everything the grant covers.
    Viewer,
    /// Read and change it, including exec.
    Operator,
}

impl EnvRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvRole::Viewer => "viewer",
            EnvRole::Operator => "operator",
        }
    }
}

impl FromStr for EnvRole {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "viewer" => Ok(EnvRole::Viewer),
            "operator" => Ok(EnvRole::Operator),
            other => Err(Error::Invalid(format!(
                "unknown environment role '{other}'"
            ))),
        }
    }
}

/// A custom role a grant came from (Enterprise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRef {
    pub id: i64,
    pub name: String,
}

/// One grant on one environment: a role, the permissions it carries, and
/// where it applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Grant {
    /// The built-in role; for a custom role, the closest built-in (Viewer
    /// when every permission is one Viewer has), for clients that know
    /// only those two.
    pub role: EnvRole,
    #[serde(skip)]
    #[schema(ignore)]
    pub custom: Option<CustomRef>,
    #[serde(skip)]
    #[schema(ignore)]
    pub perms: PermSet,
    /// `None` is the whole environment; otherwise the only namespaces
    /// (Kubernetes) the grant covers.
    pub namespaces: Option<BTreeSet<String>>,
}

pub fn ns_denied(ns: &str) -> Error {
    Error::Forbidden(format!("namespace '{ns}' is not in your access"))
}

pub fn cluster_scoped_denied() -> Error {
    Error::Forbidden("cluster-scoped objects are outside your access".into())
}

/// An RFC 1123 DNS label: what Kubernetes allows for namespace names.
pub fn is_dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
}

pub fn normalize_namespaces(v: Vec<String>) -> Result<BTreeSet<String>> {
    if v.is_empty() {
        return Err(Error::Invalid(
            "give at least one namespace, or omit for the whole cluster".into(),
        ));
    }
    if let Some(bad) = v.iter().find(|n| !is_dns_label(n)) {
        return Err(Error::Invalid(format!(
            "'{bad}' is not a Kubernetes namespace"
        )));
    }
    let set: BTreeSet<String> = v.into_iter().collect();
    if set.len() > NAMESPACES_MAX {
        return Err(Error::Invalid(format!(
            "at most {NAMESPACES_MAX} namespaces per grant"
        )));
    }
    Ok(set)
}

/// Whether one applied document may be sent: checked after namespace
/// defaulting and before any request, dry run included.
pub fn check_doc(
    allowed: Option<&BTreeSet<String>>,
    namespaced: bool,
    ns: Option<&str>,
) -> Result<()> {
    let Some(set) = allowed else { return Ok(()) };
    match (namespaced, ns) {
        (true, Some(ns)) if set.contains(ns) => Ok(()),
        (true, Some(ns)) => Err(ns_denied(ns)),
        _ => Err(cluster_scoped_denied()),
    }
}

/// One user's grants on one environment from every source, combined: the
/// highest role wins, and any whole-environment grant beats namespace lists,
/// which otherwise add up (teams spec, "Effective access").
pub fn merge_grants(grants: impl IntoIterator<Item = Grant>) -> Option<Grant> {
    grants.into_iter().reduce(|a, b| Grant {
        role: a.role.max(b.role),
        custom: None,
        perms: a.perms.union(b.perms),
        namespaces: match (a.namespaces, b.namespaces) {
            (Some(mut x), Some(y)) => {
                x.extend(y);
                Some(x)
            }
            _ => None,
        },
    })
}

/// Grants from sources other than a user's direct bindings, such as
/// Enterprise teams. `authz` merges them with the direct grant.
#[async_trait]
pub trait ExtraGrants: Send + Sync {
    async fn env_grants(&self, user_id: i64, env_id: i64) -> Result<Vec<Grant>>;
    /// The same for every environment, keyed by environment id.
    async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Vec<Grant>>>;
}

/// The Community edition: direct bindings only.
pub struct NoExtraGrants;

#[async_trait]
impl ExtraGrants for NoExtraGrants {
    async fn env_grants(&self, _: i64, _: i64) -> Result<Vec<Grant>> {
        Ok(vec![])
    }
    async fn user_grants(&self, _: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        Ok(HashMap::new())
    }
}

impl Grant {
    pub fn full(role: EnvRole) -> Self {
        Self::builtin(role, None)
    }

    pub fn builtin(role: EnvRole, namespaces: Option<BTreeSet<String>>) -> Self {
        Self {
            role,
            custom: None,
            perms: role.perms(),
            namespaces,
        }
    }

    pub fn custom(r: CustomRef, perms: PermSet, namespaces: Option<BTreeSet<String>>) -> Self {
        let role = if perms.is_subset(&EnvRole::Viewer.perms()) {
            EnvRole::Viewer
        } else {
            EnvRole::Operator
        };
        Self {
            role,
            custom: Some(r),
            perms,
            namespaces,
        }
    }

    pub fn require(&self, role: EnvRole) -> Result<()> {
        if self.role >= role {
            Ok(())
        } else {
            Err(Error::Forbidden(format!("{} role required", role.as_str())))
        }
    }

    pub fn allows_ns(&self, ns: &str) -> bool {
        self.namespaces.as_ref().is_none_or(|s| s.contains(ns))
    }

    pub fn check_ns(&self, ns: &str) -> Result<()> {
        if self.allows_ns(ns) {
            Ok(())
        } else {
            Err(ns_denied(ns))
        }
    }

    /// A list request: one namespace must be in the set; all namespaces and
    /// the two readable cluster-scoped kinds are filtered instead.
    pub fn check_list(&self, kind: KubeKind, ns: Option<&str>) -> Result<()> {
        if self.namespaces.is_some() && closed_cluster_kind(kind) {
            return Err(cluster_scoped_denied());
        }
        match ns {
            Some(ns) if !kind.cluster_scoped() => self.check_ns(ns),
            _ => Ok(()),
        }
    }

    /// One object. `ns` is `-` for cluster-scoped kinds.
    pub fn check_object(&self, kind: KubeKind, ns: &str, name: &str, write: bool) -> Result<()> {
        if self.namespaces.is_none() {
            return Ok(());
        }
        match kind {
            KubeKind::Nodes | KubeKind::Namespaces if write => Err(cluster_scoped_denied()),
            KubeKind::Nodes => Ok(()),
            KubeKind::Namespaces => self.check_ns(name),
            k if k.cluster_scoped() => Err(cluster_scoped_denied()),
            _ => self.check_ns(ns),
        }
    }

    /// A list of a discovered kind: cluster-scoped ones need the whole
    /// environment; namespaced ones, as any namespaced kind.
    pub fn check_list_in(&self, namespaced: bool, ns: Option<&str>) -> Result<()> {
        if self.namespaces.is_none() {
            return Ok(());
        }
        if !namespaced {
            return Err(cluster_scoped_denied());
        }
        ns.map_or(Ok(()), |ns| self.check_ns(ns))
    }

    pub fn check_object_in(&self, namespaced: bool, ns: &str) -> Result<()> {
        self.check_list_in(namespaced, Some(ns))
    }

    fn visible_in(&self, namespaced: bool, ns: Option<&str>) -> bool {
        match &self.namespaces {
            None => true,
            Some(set) => namespaced && ns.is_some_and(|n| set.contains(n)),
        }
    }

    pub fn filter_list_in(&self, namespaced: bool, mut list: KubeList) -> KubeList {
        list.rows
            .retain(|r| self.visible_in(namespaced, r.namespace.as_deref()));
        list
    }

    pub fn filter_event_in(&self, namespaced: bool, ev: WatchEvent) -> Option<WatchEvent> {
        filter_by(ev, |ns, _| self.visible_in(namespaced, ns))
    }

    fn row_visible(&self, kind: KubeKind, ns: Option<&str>, name: &str) -> bool {
        let Some(set) = &self.namespaces else {
            return true;
        };
        match kind {
            KubeKind::Nodes => true,
            KubeKind::Namespaces => set.contains(name),
            _ => ns.is_some_and(|n| set.contains(n)),
        }
    }

    pub fn filter_list(&self, kind: KubeKind, mut list: KubeList) -> KubeList {
        list.rows
            .retain(|r| self.row_visible(kind, r.namespace.as_deref(), &r.name));
        list
    }

    /// The event as this grant sees it; `None` when it is about an object
    /// outside the grant.
    pub fn filter_event(&self, kind: KubeKind, ev: WatchEvent) -> Option<WatchEvent> {
        filter_by(ev, |ns, name| self.row_visible(kind, ns, name))
    }
}

/// Every grant one user holds on one environment, kept apart: a permission
/// is allowed when a single grant both holds it and covers the target
/// (custom roles spec, "How grants combine").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants(pub Vec<Grant>);

pub fn perm_denied(p: Permission) -> Error {
    Error::Forbidden(format!("{p} permission required"))
}

/// Reading Nodes and Namespaces needs only some grant (the namespace
/// picker must work for any role).
fn open_read(p: Permission, kind: KubeKind) -> bool {
    p.verb == Verb::View && matches!(kind, KubeKind::Nodes | KubeKind::Namespaces)
}

impl Grants {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every permission any grant holds, wherever it applies: a UI hint.
    pub fn perms_any(&self) -> PermSet {
        self.0.iter().fold(PermSet::default(), |a, g| a.union(g.perms))
    }

    fn holding(&self, p: Permission) -> impl Iterator<Item = &Grant> {
        self.0.iter().filter(move |g| g.perms.contains(p))
    }

    /// Ok when some candidate passes `check`; otherwise the first
    /// candidate's refusal, or the missing permission when there is none.
    fn first_ok<'a>(
        p: Permission,
        candidates: impl Iterator<Item = &'a Grant>,
        check: impl Fn(&Grant) -> Result<()>,
    ) -> Result<()> {
        let mut first = None;
        for g in candidates {
            match check(g) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        Err(first.unwrap_or_else(|| perm_denied(p)))
    }

    /// `p` in namespace `ns`, or with `None`, across the whole environment.
    pub fn allows(&self, p: Permission, ns: Option<&str>) -> bool {
        self.holding(p).any(|g| match ns {
            Some(ns) => g.allows_ns(ns),
            None => g.namespaces.is_none(),
        })
    }

    /// An action on the environment as a whole (Docker, cordon).
    pub fn check_env(&self, p: Permission) -> Result<()> {
        Self::first_ok(p, self.holding(p), |g| match g.namespaces {
            None => Ok(()),
            Some(_) => Err(cluster_scoped_denied()),
        })
    }

    pub fn check_ns(&self, p: Permission, ns: &str) -> Result<()> {
        Self::first_ok(p, self.holding(p), |g| g.check_ns(ns))
    }

    pub fn check_list(&self, p: Permission, kind: KubeKind, ns: Option<&str>) -> Result<()> {
        if open_read(p, kind) {
            return Self::first_ok(p, self.0.iter(), |g| g.check_list(kind, ns));
        }
        Self::first_ok(p, self.holding(p), |g| g.check_list(kind, ns))
    }

    pub fn check_object(&self, p: Permission, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        if open_read(p, kind) {
            return Self::first_ok(p, self.0.iter(), |g| g.check_object(kind, ns, name, false));
        }
        let write = p.verb != Verb::View;
        Self::first_ok(p, self.holding(p), |g| g.check_object(kind, ns, name, write))
    }

    pub fn check_list_in(&self, p: Permission, namespaced: bool, ns: Option<&str>) -> Result<()> {
        Self::first_ok(p, self.holding(p), |g| g.check_list_in(namespaced, ns))
    }

    pub fn check_object_in(&self, p: Permission, namespaced: bool, ns: &str) -> Result<()> {
        Self::first_ok(p, self.holding(p), |g| g.check_object_in(namespaced, ns))
    }

    /// One applied document: `create` and `operate` from a single grant
    /// that covers it.
    pub fn check_doc(
        &self,
        create: Permission,
        operate: Permission,
        namespaced: bool,
        ns: Option<&str>,
    ) -> Result<()> {
        let both = self
            .0
            .iter()
            .filter(|g| g.perms.contains(create) && g.perms.contains(operate));
        let missing = if self.holding(create).next().is_none() {
            create
        } else {
            operate
        };
        Self::first_ok(missing, both, |g| {
            check_doc(g.namespaces.as_ref(), namespaced, ns)
        })
    }

    /// Where `p` applies: `None` is the whole environment, otherwise the
    /// union of the namespace lists of the grants holding it.
    pub fn namespaces_for(&self, p: Permission) -> Option<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for g in self.holding(p) {
            out.extend(g.namespaces.as_ref()?.iter().cloned());
        }
        Some(out)
    }

    fn row_visible(&self, kind: KubeKind, ns: Option<&str>, name: &str) -> bool {
        let view = Permission::new(row_of(kind), Verb::View);
        let open = open_read(view, kind);
        self.0
            .iter()
            .filter(|g| open || g.perms.contains(view))
            .any(|g| g.row_visible(kind, ns, name))
    }

    fn visible_in(&self, namespaced: bool, ns: Option<&str>) -> bool {
        let view = Permission::new(Row::K8sOther, Verb::View);
        self.holding(view).any(|g| g.visible_in(namespaced, ns))
    }

    pub fn filter_list(&self, kind: KubeKind, mut list: KubeList) -> KubeList {
        list.rows
            .retain(|r| self.row_visible(kind, r.namespace.as_deref(), &r.name));
        list
    }

    pub fn filter_list_in(&self, namespaced: bool, mut list: KubeList) -> KubeList {
        list.rows
            .retain(|r| self.visible_in(namespaced, r.namespace.as_deref()));
        list
    }

    pub fn filter_event(&self, kind: KubeKind, ev: WatchEvent) -> Option<WatchEvent> {
        filter_by(ev, |ns, name| self.row_visible(kind, ns, name))
    }

    pub fn filter_event_in(&self, namespaced: bool, ev: WatchEvent) -> Option<WatchEvent> {
        filter_by(ev, |ns, _| self.visible_in(namespaced, ns))
    }
}

/// A curated cluster-scoped kind other than the two everyone may read.
fn closed_cluster_kind(kind: KubeKind) -> bool {
    kind.cluster_scoped() && !matches!(kind, KubeKind::Nodes | KubeKind::Namespaces)
}

/// A watch event narrowed to the rows `visible` keeps.
fn filter_by(ev: WatchEvent, visible: impl Fn(Option<&str>, &str) -> bool) -> Option<WatchEvent> {
    match ev {
        WatchEvent::Snapshot { mut rows } => {
            rows.retain(|r| visible(r.namespace.as_deref(), &r.name));
            Some(WatchEvent::Snapshot { rows })
        }
        WatchEvent::Added { row } | WatchEvent::Modified { row }
            if !visible(row.namespace.as_deref(), &row.name) =>
        {
            None
        }
        WatchEvent::Deleted { namespace, name } if !visible(namespace.as_deref(), &name) => None,
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kube::{KubeList, KubeRow, RowHealth, WatchEvent};

    fn limited(ns: &[&str]) -> Grant {
        Grant::builtin(
            EnvRole::Operator,
            Some(ns.iter().map(|s| s.to_string()).collect()),
        )
    }
    #[test]
    fn merge_empty_is_none() {
        assert_eq!(merge_grants([]), None);
    }

    #[test]
    fn merge_highest_role_wins() {
        let g = merge_grants([Grant::full(EnvRole::Viewer), Grant::full(EnvRole::Operator)]);
        assert_eq!(g, Some(Grant::full(EnvRole::Operator)));
    }

    #[test]
    fn merge_whole_beats_list() {
        let g = merge_grants([Grant::full(EnvRole::Viewer), limited(&["a"])]);
        assert_eq!(g, Some(Grant::full(EnvRole::Operator)));
    }

    #[test]
    fn merge_unions_lists() {
        let g = merge_grants([limited(&["a"]), limited(&["b", "c"])]);
        assert_eq!(g, Some(limited(&["a", "b", "c"])));
    }

    fn row(name: &str, ns: Option<&str>) -> KubeRow {
        KubeRow {
            name: name.into(),
            namespace: ns.map(str::to_string),
            created: 0,
            status: String::new(),
            health: RowHealth::Ok,
            columns: Default::default(),
        }
    }

    #[test]
    fn roles_order_and_round_trip() {
        assert!(EnvRole::Viewer < EnvRole::Operator);
        assert_eq!("viewer".parse::<EnvRole>().unwrap(), EnvRole::Viewer);
        assert_eq!(EnvRole::Operator.as_str(), "operator");
        assert!("admin".parse::<EnvRole>().is_err());
        assert_eq!(serde_json::to_value(EnvRole::Viewer).unwrap(), "viewer");
    }

    #[test]
    fn a_viewer_is_refused_writes_with_the_reason() {
        let g = Grant::full(EnvRole::Viewer);
        assert!(g.require(EnvRole::Viewer).is_ok());
        let e = g.require(EnvRole::Operator).unwrap_err();
        assert!(
            matches!(&e, Error::Forbidden(m) if m == "operator role required"),
            "{e}"
        );
        assert!(Grant::full(EnvRole::Operator)
            .require(EnvRole::Operator)
            .is_ok());
    }

    #[test]
    fn a_whole_environment_grant_allows_every_namespace_and_kind() {
        let g = Grant::full(EnvRole::Operator);
        assert!(g.allows_ns("anything"));
        assert!(g.check_list(KubeKind::Pods, None).is_ok());
        assert!(g.check_object(KubeKind::Nodes, "-", "n1", true).is_ok());
        assert!(g
            .check_object(KubeKind::Namespaces, "-", "kube-system", true)
            .is_ok());
    }

    #[test]
    fn a_limited_grant_checks_namespaces() {
        let g = limited(&["team-a"]);
        assert!(g.check_ns("team-a").is_ok());
        let e = g.check_ns("team-b").unwrap_err();
        assert!(
            matches!(&e, Error::Forbidden(m) if m == "namespace 'team-b' is not in your access")
        );
        assert!(g.check_list(KubeKind::Pods, Some("team-a")).is_ok());
        assert!(g.check_list(KubeKind::Pods, Some("team-b")).is_err());
        assert!(
            g.check_list(KubeKind::Pods, None).is_ok(),
            "all namespaces is filtered, not refused"
        );
        assert!(g.check_object(KubeKind::Pods, "team-a", "p", true).is_ok());
        assert!(g
            .check_object(KubeKind::Pods, "team-b", "p", false)
            .is_err());
    }

    #[test]
    fn a_limited_grant_reads_nodes_and_its_namespaces_and_writes_nothing_cluster_scoped() {
        let g = limited(&["team-a"]);
        assert!(g.check_list(KubeKind::Nodes, None).is_ok());
        assert!(g.check_list(KubeKind::Namespaces, None).is_ok());
        assert!(g.check_object(KubeKind::Nodes, "-", "n1", false).is_ok());
        assert!(g
            .check_object(KubeKind::Namespaces, "-", "team-a", false)
            .is_ok());
        assert!(g
            .check_object(KubeKind::Namespaces, "-", "team-b", false)
            .is_err());
        for (k, name) in [(KubeKind::Nodes, "n1"), (KubeKind::Namespaces, "team-a")] {
            let e = g.check_object(k, "-", name, true).unwrap_err();
            assert!(
                matches!(&e, Error::Forbidden(m) if m == "cluster-scoped objects are outside your access")
            );
        }
    }

    #[test]
    fn lists_are_filtered_to_the_set() {
        let g = limited(&["team-a"]);
        let pods = KubeList {
            rows: vec![row("a", Some("team-a")), row("b", Some("team-b"))],
            truncated: true,
        };
        let out = g.filter_list(KubeKind::Pods, pods);
        assert_eq!(
            out.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        assert!(out.truncated);
        let nss = KubeList {
            rows: vec![row("team-a", None), row("kube-system", None)],
            truncated: false,
        };
        assert_eq!(g.filter_list(KubeKind::Namespaces, nss).rows.len(), 1);
        let nodes = KubeList {
            rows: vec![row("n1", None)],
            truncated: false,
        };
        assert_eq!(g.filter_list(KubeKind::Nodes, nodes).rows.len(), 1);
        let all = Grant::full(EnvRole::Viewer);
        let both = KubeList {
            rows: vec![row("a", Some("x")), row("b", Some("y"))],
            truncated: false,
        };
        assert_eq!(all.filter_list(KubeKind::Pods, both).rows.len(), 2);
    }

    #[test]
    fn watch_events_are_filtered_to_the_set() {
        let g = limited(&["team-a"]);
        let snap = g
            .filter_event(
                KubeKind::Pods,
                WatchEvent::Snapshot {
                    rows: vec![row("a", Some("team-a")), row("b", Some("team-b"))],
                },
            )
            .unwrap();
        assert!(matches!(snap, WatchEvent::Snapshot { rows } if rows.len() == 1));
        assert!(g
            .filter_event(
                KubeKind::Pods,
                WatchEvent::Added {
                    row: row("b", Some("team-b"))
                }
            )
            .is_none());
        assert!(g
            .filter_event(
                KubeKind::Pods,
                WatchEvent::Modified {
                    row: row("a", Some("team-a"))
                }
            )
            .is_some());
        assert!(g
            .filter_event(
                KubeKind::Pods,
                WatchEvent::Deleted {
                    namespace: Some("team-b".into()),
                    name: "b".into()
                }
            )
            .is_none());
        assert!(g
            .filter_event(
                KubeKind::Namespaces,
                WatchEvent::Deleted {
                    namespace: None,
                    name: "team-a".into()
                }
            )
            .is_some());
        assert!(g.filter_event(KubeKind::Pods, WatchEvent::Resync).is_some());
    }

    #[test]
    fn an_applied_document_must_be_namespaced_and_in_the_set() {
        let set: BTreeSet<String> = ["team-a".to_string()].into();
        assert!(check_doc(None, false, None).is_ok());
        assert!(check_doc(Some(&set), true, Some("team-a")).is_ok());
        assert!(
            matches!(check_doc(Some(&set), true, Some("team-b")), Err(Error::Forbidden(m)) if m.contains("team-b"))
        );
        assert!(
            matches!(check_doc(Some(&set), false, None), Err(Error::Forbidden(m)) if m.contains("cluster-scoped"))
        );
    }

    #[test]
    fn namespaces_are_validated_deduplicated_and_capped() {
        let n = normalize_namespaces(vec!["b".into(), "a".into(), "b".into()]).unwrap();
        assert_eq!(n.into_iter().collect::<Vec<_>>(), ["a", "b"]);
        assert!(
            matches!(normalize_namespaces(vec![]), Err(Error::Invalid(m)) if m.contains("at least one"))
        );
        assert!(
            matches!(normalize_namespaces(vec!["Bad_NS".into()]), Err(Error::Invalid(m)) if m.contains("Bad_NS"))
        );
        let many = (0..=NAMESPACES_MAX).map(|i| format!("ns{i}")).collect();
        assert!(matches!(normalize_namespaces(many), Err(Error::Invalid(m)) if m.contains("50")));
    }

    #[test]
    fn a_limited_grant_is_refused_the_new_cluster_scoped_kinds() {
        let g = limited(&["team-a"]);
        for k in [
            KubeKind::PersistentVolumes,
            KubeKind::StorageClasses,
            KubeKind::ClusterRoles,
            KubeKind::ClusterRoleBindings,
        ] {
            let e = g.check_list(k, None).unwrap_err();
            assert_eq!(e.to_string(), cluster_scoped_denied().to_string(), "{k:?}");
            assert!(g.check_object(k, "-", "x", false).is_err(), "{k:?}");
        }
        // Nodes keep today's rule: readable, never writable.
        assert!(g.check_list(KubeKind::Nodes, None).is_ok());
        assert!(Grant::full(EnvRole::Viewer)
            .check_list(KubeKind::PersistentVolumes, None)
            .is_ok());
    }

    #[test]
    fn generic_kinds_follow_their_scope() {
        let g = limited(&["team-a"]);
        assert!(g.check_list_in(true, Some("team-a")).is_ok());
        assert!(g.check_list_in(true, None).is_ok());
        assert!(g.check_list_in(true, Some("team-b")).is_err());
        assert_eq!(
            g.check_list_in(false, None).unwrap_err().to_string(),
            cluster_scoped_denied().to_string()
        );
        assert!(g.check_object_in(true, "team-a").is_ok());
        assert!(g.check_object_in(true, "team-b").is_err());
        assert!(g.check_object_in(false, "-").is_err());
        let full = Grant::full(EnvRole::Viewer);
        assert!(
            full.check_list_in(false, None).is_ok() && full.check_object_in(false, "-").is_ok()
        );

        let list = KubeList {
            rows: vec![row("a", Some("team-a")), row("b", Some("team-b"))],
            truncated: false,
        };
        let kept = g.filter_list_in(true, list);
        assert_eq!(
            kept.rows
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
        assert!(g
            .filter_event_in(
                true,
                WatchEvent::Added {
                    row: row("b", Some("team-b"))
                }
            )
            .is_none());
        assert!(g
            .filter_event_in(
                true,
                WatchEvent::Added {
                    row: row("a", Some("team-a"))
                }
            )
            .is_some());
    }

    // --- Grants: each grant on its own, per permission (custom roles spec) ---

    fn pm(s: &str) -> crate::perm::Permission {
        s.parse().unwrap()
    }

    fn custom(perms: &[&str], ns: Option<&[&str]>) -> Grant {
        Grant::custom(
            CustomRef { id: 1, name: "r".into() },
            perms.iter().map(|p| pm(p)).collect(),
            ns.map(|n| n.iter().map(|s| s.to_string()).collect()),
        )
    }

    fn op_in(ns: &[&str]) -> Grants {
        Grants(vec![limited(ns)])
    }

    #[test]
    fn grants_port_the_namespace_rules() {
        let g = op_in(&["team-a"]);
        let v = pm("pods.view");
        assert!(g.check_ns(v, "team-a").is_ok());
        let e = g.check_ns(v, "team-b").unwrap_err();
        assert!(matches!(&e, Error::Forbidden(m) if m == "namespace 'team-b' is not in your access"));
        assert!(g.check_list(v, KubeKind::Pods, Some("team-a")).is_ok());
        assert!(g.check_list(v, KubeKind::Pods, Some("team-b")).is_err());
        assert!(g.check_list(v, KubeKind::Pods, None).is_ok(), "filtered, not refused");
        assert!(g.check_object(pm("pods.delete"), KubeKind::Pods, "team-a", "p").is_ok());
        assert!(g.check_object(v, KubeKind::Pods, "team-b", "p").is_err());
        // nodes and own namespaces readable, nothing cluster-scoped writable
        let k = pm("k8s-other.view");
        assert!(g.check_list(k, KubeKind::Nodes, None).is_ok());
        assert!(g.check_object(k, KubeKind::Namespaces, "-", "team-a").is_ok());
        assert!(g.check_object(k, KubeKind::Namespaces, "-", "team-b").is_err());
        let e = g.check_object(pm("k8s-other.operate"), KubeKind::Nodes, "-", "n1").unwrap_err();
        assert_eq!(e.to_string(), cluster_scoped_denied().to_string());
        assert!(g.check_list(pm("storage.view"), KubeKind::PersistentVolumes, None).is_err());
        // generic kinds
        assert!(g.check_list_in(k, true, Some("team-a")).is_ok());
        assert!(g.check_list_in(k, true, Some("team-b")).is_err());
        assert!(g.check_list_in(k, false, None).is_err());
        assert!(g.check_object_in(pm("k8s-other.delete"), true, "team-a").is_ok());
        // whole environment
        let full = Grants(vec![Grant::full(EnvRole::Operator)]);
        assert!(full.check_object(pm("k8s-other.operate"), KubeKind::Nodes, "-", "n1").is_ok());
        assert!(full.check_env(pm("containers.delete")).is_ok());
    }

    #[test]
    fn grants_filter_lists_and_events() {
        let g = op_in(&["team-a"]);
        let pods = KubeList {
            rows: vec![row("a", Some("team-a")), row("b", Some("team-b"))],
            truncated: false,
        };
        assert_eq!(g.filter_list(KubeKind::Pods, pods.clone()).rows.len(), 1);
        assert_eq!(g.filter_list_in(true, pods).rows.len(), 1);
        let nss = KubeList {
            rows: vec![row("team-a", None), row("kube-system", None)],
            truncated: false,
        };
        assert_eq!(g.filter_list(KubeKind::Namespaces, nss).rows.len(), 1);
        let added = |ns: &str| WatchEvent::Added { row: row("x", Some(ns)) };
        assert!(g.filter_event(KubeKind::Pods, added("team-b")).is_none());
        assert!(g.filter_event(KubeKind::Pods, added("team-a")).is_some());
        assert!(g.filter_event_in(true, added("team-b")).is_none());
        assert!(g.filter_event(KubeKind::Pods, WatchEvent::Resync).is_some());
    }

    #[test]
    fn narrow_operator_plus_wide_viewer_is_not_escalated() {
        let g = Grants(vec![limited(&["a"]), Grant::full(EnvRole::Viewer)]);
        let del = pm("pods.delete");
        assert!(g.check_object(del, KubeKind::Pods, "b", "p").is_err());
        assert!(g.check_object(del, KubeKind::Pods, "a", "p").is_ok());
        assert!(g.check_list(pm("pods.view"), KubeKind::Pods, Some("b")).is_ok());
        assert!(g.allows(del, Some("a")) && !g.allows(del, Some("b")));
        assert_eq!(g.namespaces_for(del), Some(["a".to_string()].into()));
        assert_eq!(g.namespaces_for(pm("pods.view")), None);
    }

    #[test]
    fn filter_is_per_row() {
        let g = Grants(vec![custom(&["pods.view"], None)]);
        let list = || KubeList {
            rows: vec![row("a", Some("x"))],
            truncated: false,
        };
        assert_eq!(g.filter_list(KubeKind::Pods, list()).rows.len(), 1);
        assert_eq!(g.filter_list(KubeKind::Secrets, list()).rows.len(), 0);
        let e = g.check_list(pm("secrets.view"), KubeKind::Secrets, None).unwrap_err();
        assert!(matches!(&e, Error::Forbidden(m) if m == "secrets.view permission required"), "{e}");
        assert_eq!(g.perms_any().to_strs(), ["pods.view"]);
        assert!(g.check_env(pm("containers.view")).is_err());
    }

    #[test]
    fn nodes_and_namespaces_need_only_a_grant() {
        let g = Grants(vec![custom(&["configmaps.view"], Some(&["a"]))]);
        let k = pm("k8s-other.view");
        assert!(g.check_list(k, KubeKind::Namespaces, None).is_ok());
        assert!(g.check_list(k, KubeKind::Nodes, None).is_ok());
        let nss = KubeList {
            rows: vec![row("a", None), row("b", None)],
            truncated: false,
        };
        let kept = g.filter_list(KubeKind::Namespaces, nss);
        assert_eq!(kept.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["a"]);
        assert!(g.check_list(k, KubeKind::ServiceAccounts, Some("a")).is_err());
    }

    #[test]
    fn check_doc_needs_both_from_one_grant() {
        let (c, o) = (pm("configmaps.create"), pm("configmaps.operate"));
        let split = Grants(vec![
            custom(&["configmaps.create"], Some(&["a"])),
            custom(&["configmaps.operate"], Some(&["a"])),
        ]);
        assert!(split.check_doc(c, o, true, Some("a")).is_err());
        let one = Grants(vec![custom(&["configmaps.create", "configmaps.operate"], Some(&["a"]))]);
        assert!(one.check_doc(c, o, true, Some("a")).is_ok());
        assert!(one.check_doc(c, o, true, Some("b")).is_err());
        assert!(one.check_doc(c, o, false, None).is_err());
    }
}
