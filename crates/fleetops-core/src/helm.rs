//! Helm releases (sub-project 3d): what the API shows of a release and the
//! plan of a rollback or uninstall. `fleetops-kube` reads and writes Helm's
//! own storage; nothing here holds an unmasked value.

use crate::kube::ApplyError;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The status of a release whose stored record cannot be decoded.
pub const UNREADABLE: &str = "unreadable";
/// Said on every rollback and uninstall plan.
pub const HOOKS_NOTE: &str = "Chart hooks are not run by FleetOps.";
/// The refusal when the release moved since it was reviewed, or another
/// operation is in progress.
pub const CHANGED: &str = "release changed; review again";
/// Install and upgrade without a renderer (spec 3j §2).
pub const HELM_OFF: &str = "helm install and upgrade need --helm-binary on the server";

/// `pending-install`, `pending-upgrade`, `pending-rollback`: another
/// operation holds the release.
pub fn is_pending(status: &str) -> bool {
    status.starts_with("pending-")
}

/// One release, at its latest revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    /// Helm's status (`deployed`, `failed`, `pending-upgrade`, …), or `unreadable`.
    pub status: String,
    pub chart: String,
    pub chart_version: String,
    pub app_version: String,
    /// Last deployed, unix seconds; 0 when unknown.
    pub updated: i64,
    /// Why the record cannot be read, when the status is `unreadable`.
    pub error: Option<String>,
}

/// One revision in a release's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HelmRevision {
    pub revision: u32,
    pub status: String,
    pub chart_version: String,
    pub app_version: String,
    pub description: String,
    pub updated: i64,
}

/// One revision in full, masked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReleaseDetail {
    pub release: HelmRelease,
    pub description: String,
    pub notes: String,
    /// User-supplied values as YAML, credential-looking keys masked; empty when none.
    pub values: String,
    /// The rendered manifest, Secret values masked.
    pub manifest: String,
    /// Where FleetOps last installed or upgraded the chart from (spec 3j §5).
    #[serde(default)]
    pub chart_source: Option<ChartRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HelmChange {
    /// Patched from the current revision's form to the target's.
    Patch,
    /// In the target revision only.
    Create,
    /// In the current revision only, or removed by an uninstall.
    Delete,
    /// Would be deleted, but annotated `helm.sh/resource-policy: keep`.
    Keep,
}

/// What a rollback or uninstall does (or did) to one object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HelmObjectChange {
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub change: HelmChange,
    /// The live object before, cleaned and masked; `None` when absent.
    pub before: Option<String>,
    /// The object after (or as it would be); `None` for a delete or keep.
    pub after: Option<String>,
    pub error: Option<ApplyError>,
}

/// A rollback, uninstall, install or upgrade: the dry run's plan, or a commit's results.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HelmPlan {
    /// The latest revision this was computed against; a commit sends it
    /// back as `expected_current`.
    pub current: u32,
    /// Commit of a rollback only: the revision written.
    pub revision: Option<u32>,
    /// Commit only: the release's status afterwards (`deployed`, `failed`
    /// or `uninstalled`).
    pub status: Option<String>,
    pub changes: Vec<HelmObjectChange>,
    pub note: String,
    /// Install and upgrade: the hooks that run, in order.
    #[serde(default)]
    pub hooks: Vec<HookStep>,
    /// Upgrade: the current values, masked YAML.
    #[serde(default)]
    pub values_before: Option<String>,
    /// Install and upgrade: the new values, masked YAML.
    #[serde(default)]
    pub values_after: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub chart: Option<ChartMeta>,
}

/// Where a chart comes from (spec 3j §5): a repository and chart name, or an OCI reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChartRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci: Option<String>,
    /// Empty for the latest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

fn plain(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(char::is_whitespace)
}

fn chart_name(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

impl ChartRef {
    /// Refuses anything helm could read as a flag or a local path (spec 3j §5).
    pub fn validate(&self, insecure_repos: bool) -> Result<()> {
        let bad = |m: &str| Err(Error::Invalid(m.into()));
        match (&self.repo_url, &self.chart, &self.oci) {
            (Some(url), Some(chart), None) => {
                let scheme_ok =
                    url.starts_with("https://") || (insecure_repos && url.starts_with("http://"));
                if !plain(url) || !scheme_ok {
                    return bad("the repository URL must be https:// (http:// needs --helm-insecure-repos on the server)");
                }
                if !chart_name(chart) {
                    return bad(
                        "the chart name must be lowercase letters, digits, '.', '_' or '-'",
                    );
                }
            }
            (None, None, Some(r)) => {
                if !plain(r) || !r.starts_with("oci://") || r.len() <= "oci://".len() {
                    return bad("an OCI reference starts with oci:// and has no spaces");
                }
            }
            _ => return bad("give a repository URL and chart, or an oci:// reference"),
        }
        match &self.version {
            Some(v) if !plain(v) => bad("the version must be a version, with no spaces"),
            _ => Ok(()),
        }
    }

    /// For audit rows and messages.
    pub fn label(&self) -> String {
        match (&self.repo_url, &self.chart, &self.oci) {
            (_, _, Some(o)) => o.clone(),
            (Some(u), Some(c), _) => format!("{u} {c}"),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChartMeta {
    pub name: String,
    pub version: String,
    pub app_version: String,
    pub description: String,
}

/// A chart's metadata and default values, for the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChartShow {
    pub meta: ChartMeta,
    pub values: String,
}

/// One hook a deploy runs, as the plan lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HookStep {
    pub events: Vec<String>,
    pub kind: String,
    pub name: String,
    pub weight: i32,
    pub delete_policies: Vec<String>,
}

/// A hook as Helm stores it in a release (`helm get hooks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedHook {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub manifest: String,
    pub events: Vec<String>,
    pub weight: i32,
    #[serde(default)]
    pub delete_policies: Vec<String>,
}

/// A chart rendered by helm (spec 3j §2).
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The release manifest in Helm's stored form, hooks left out.
    pub manifest: String,
    pub hooks: Vec<RenderedHook>,
    pub notes: String,
    /// Helm's chart object, stored in the release as Helm stores it.
    pub chart: serde_json::Value,
    pub meta: ChartMeta,
}

/// An install or upgrade to plan or commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Deploy {
    pub namespace: String,
    pub name: String,
    /// The user-supplied values, unmasked.
    pub config: serde_json::Value,
    pub rendered: Rendered,
    pub source: ChartRef,
    pub upgrade: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelmMode {
    Plan,
    Commit { expected_current: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_statuses_are_recognised() {
        for s in ["pending-install", "pending-upgrade", "pending-rollback"] {
            assert!(is_pending(s), "{s}");
        }
        for s in [
            "deployed",
            "failed",
            "superseded",
            "uninstalling",
            UNREADABLE,
        ] {
            assert!(!is_pending(s), "{s}");
        }
    }

    #[test]
    fn a_change_serialises_in_lowercase() {
        assert_eq!(serde_json::to_value(HelmChange::Patch).unwrap(), "patch");
        assert_eq!(serde_json::to_value(HelmChange::Keep).unwrap(), "keep");
    }

    fn repo(url: &str, chart: &str) -> ChartRef {
        ChartRef {
            repo_url: Some(url.into()),
            chart: Some(chart.into()),
            ..Default::default()
        }
    }
    fn oci(r: &str) -> ChartRef {
        ChartRef {
            oci: Some(r.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_repo_chart_or_an_oci_reference_is_valid() {
        assert!(repo("https://charts.example.com", "nginx")
            .validate(false)
            .is_ok());
        assert!(oci("oci://ghcr.io/org/charts/app").validate(false).is_ok());
        let versioned = ChartRef {
            version: Some("1.2.3".into()),
            ..oci("oci://r/c")
        };
        assert!(versioned.validate(false).is_ok());
    }

    #[test]
    fn http_repos_need_the_insecure_flag() {
        assert!(repo("http://127.0.0.1:8080", "demo")
            .validate(false)
            .is_err());
        assert!(repo("http://127.0.0.1:8080", "demo").validate(true).is_ok());
    }

    #[test]
    fn exactly_one_kind_of_source() {
        let both = ChartRef {
            oci: Some("oci://r/c".into()),
            ..repo("https://x", "c")
        };
        for bad in [
            ChartRef::default(),
            both,
            ChartRef {
                chart: Some("c".into()),
                ..Default::default()
            },
        ] {
            let e = bad.validate(false).unwrap_err().to_string();
            assert!(
                e.contains("a repository URL and chart, or an oci:// reference"),
                "{e}"
            );
        }
    }

    #[test]
    fn flags_are_never_chart_fields() {
        for bad in [
            ChartRef {
                version: Some("--set=x".into()),
                ..oci("oci://r/c")
            },
            ChartRef {
                version: Some("".into()),
                ..oci("oci://r/c")
            },
            ChartRef {
                version: Some("1 2".into()),
                ..oci("oci://r/c")
            },
            oci("--post-renderer=/bin/sh"),
            oci("oci://r/c --x"),
            repo("https://x", "-c"),
            repo("https://x", "Upper"),
            repo("https://x y", "c"),
            repo("ftp://x", "c"),
        ] {
            assert!(bad.validate(true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_label_names_the_source() {
        assert_eq!(repo("https://x", "nginx").label(), "https://x nginx");
        assert_eq!(oci("oci://r/c").label(), "oci://r/c");
    }

    #[test]
    fn older_plans_and_details_still_deserialise() {
        let p: HelmPlan = serde_json::from_value(serde_json::json!({"current": 2, "revision": null, "status": null, "changes": [], "note": "n"})).unwrap();
        assert!(p.hooks.is_empty() && p.chart.is_none() && p.notes.is_empty());
    }
}
