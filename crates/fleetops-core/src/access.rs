//! Per-environment access (spec §2.2): what a user may do in one
//! environment, and for Kubernetes, in which namespaces. Pure: the store
//! produces grants, the API enforces them.

use crate::kube::{KubeKind, KubeList, WatchEvent};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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

/// One user's access to one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Grant {
    pub role: EnvRole,
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

impl Grant {
    pub fn full(role: EnvRole) -> Self {
        Self {
            role,
            namespaces: None,
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
            _ => self.check_ns(ns),
        }
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
        match ev {
            WatchEvent::Snapshot { mut rows } => {
                rows.retain(|r| self.row_visible(kind, r.namespace.as_deref(), &r.name));
                Some(WatchEvent::Snapshot { rows })
            }
            WatchEvent::Added { row } | WatchEvent::Modified { row }
                if !self.row_visible(kind, row.namespace.as_deref(), &row.name) =>
            {
                None
            }
            WatchEvent::Deleted { namespace, name }
                if !self.row_visible(kind, namespace.as_deref(), &name) =>
            {
                None
            }
            other => Some(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kube::{KubeList, KubeRow, RowHealth, WatchEvent};

    fn limited(ns: &[&str]) -> Grant {
        Grant {
            role: EnvRole::Operator,
            namespaces: Some(ns.iter().map(|s| s.to_string()).collect()),
        }
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
}
