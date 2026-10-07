//! Helm releases (sub-project 3d): what the API shows of a release and the
//! plan of a rollback or uninstall. `fleetops-kube` reads and writes Helm's
//! own storage; nothing here holds an unmasked value.

use crate::kube::ApplyError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The status of a release whose stored record cannot be decoded.
pub const UNREADABLE: &str = "unreadable";
/// Said on every rollback and uninstall plan.
pub const HOOKS_NOTE: &str = "Chart hooks are not run by FleetOps.";
/// The refusal when the release moved since it was reviewed, or another
/// operation is in progress.
pub const CHANGED: &str = "release changed; review again";

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

/// A rollback or uninstall: the dry run's plan, or a commit's results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
}
