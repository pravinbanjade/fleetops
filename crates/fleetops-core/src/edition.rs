//! Editions and entitlements (editions spec §3.1). Community crates see
//! only this trait; the Enterprise crate supplies a license-backed one.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Edition {
    Community,
    Enterprise,
}

/// An Enterprise feature a license can grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Feature {
    /// `GET /ee/audit/export`.
    AuditExport,
    /// Teams that carry environment access (`/ee/teams`). Every active
    /// license has it.
    Teams,
}

impl Feature {
    pub const ALL: &'static [Feature] = &[Feature::AuditExport, Feature::Teams];

    pub fn as_str(self) -> &'static str {
        match self {
            Feature::AuditExport => "audit_export",
            Feature::Teams => "teams",
        }
    }

    /// `None` for names this build does not know (a newer license).
    pub fn parse(name: &str) -> Option<Feature> {
        Self::ALL.iter().copied().find(|f| f.as_str() == name)
    }
}

/// What this server may do beyond Community. Checked per request, so a
/// license change takes effect without a restart.
pub trait Entitlements: Send + Sync {
    fn edition(&self) -> Edition;
    fn has(&self, feature: Feature) -> bool;
    /// The most users the license allows; `None` is unlimited.
    fn max_users(&self) -> Option<u32>;
    /// The most nodes the license allows; `None` is unlimited. A node is a
    /// Docker or Podman host, or a node of a Kubernetes cluster.
    fn max_nodes(&self) -> Option<u32> {
        None
    }
}

/// The Community edition: no Enterprise features, no limits.
pub struct Community;

impl Entitlements for Community {
    fn edition(&self) -> Edition {
        Edition::Community
    }
    fn has(&self, _: Feature) -> bool {
        false
    }
    fn max_users(&self) -> Option<u32> {
        None
    }
}

/// `GET /edition`: the edition and the features in effect right now.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct EditionInfo {
    pub edition: Edition,
    pub features: Vec<Feature>,
}

impl EditionInfo {
    pub fn of(e: &dyn Entitlements) -> Self {
        Self {
            edition: e.edition(),
            features: Feature::ALL.iter().copied().filter(|f| e.has(*f)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Only(Feature);
    impl Entitlements for Only {
        fn edition(&self) -> Edition {
            Edition::Enterprise
        }
        fn has(&self, f: Feature) -> bool {
            f == self.0
        }
        fn max_users(&self) -> Option<u32> {
            Some(3)
        }
    }

    #[test]
    fn features_round_trip_their_wire_names() {
        for f in Feature::ALL {
            assert_eq!(Feature::parse(f.as_str()), Some(*f));
            assert_eq!(serde_json::to_value(f).unwrap(), f.as_str());
        }
        assert_eq!(Feature::parse("audit_export"), Some(Feature::AuditExport));
        assert_eq!(Feature::parse("teleport"), None);
    }

    #[test]
    fn community_has_nothing_and_no_limit() {
        let info = EditionInfo::of(&Community);
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({"edition": "community", "features": []})
        );
        assert_eq!(Community.max_users(), None);
        assert_eq!(Community.max_nodes(), None);
    }

    #[test]
    fn info_lists_only_entitled_features() {
        let info = EditionInfo::of(&Only(Feature::AuditExport));
        assert_eq!(info.edition, Edition::Enterprise);
        assert_eq!(info.features, vec![Feature::AuditExport]);
    }
}
