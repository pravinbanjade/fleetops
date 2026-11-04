//! What a scan is about and what it found.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A scan's subject: the manifest digest (`sha256:…`) when the image has
/// one, so the same image on two hosts or clusters shares a result; else
/// `local:<env_id>:<node>:<image_id>` for an image built or loaded on one
/// host; else `ref:<env_id>:<image>` for a Kubernetes template whose image
/// no pod has pulled.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageKey(pub String);

impl ImageKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The manifest digest, when this key is one.
    pub fn digest(&self) -> Option<&str> {
        self.0.starts_with("sha256:").then_some(self.0.as_str())
    }
}

/// The key of a Docker image: its first `RepoDigests` digest, else local.
pub fn image_key(env_id: i64, node: &str, repo_digests: &[String], image_id: &str) -> ImageKey {
    repo_digests
        .iter()
        .find_map(|d| d.rsplit_once('@').map(|(_, h)| h))
        .filter(|h| h.starts_with("sha256:"))
        .map(|h| ImageKey(h.to_string()))
        .unwrap_or_else(|| ImageKey(format!("local:{env_id}:{node}:{image_id}")))
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    // Ascending, so `Ord` puts Critical highest.
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Unknown => "unknown",
        }
    }

    /// Trivy's spelling (`CRITICAL`…) or ours; anything else is unknown.
    pub fn parse(s: &str) -> Severity {
        match s.to_ascii_lowercase().as_str() {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" => Severity::Medium,
            "low" => Severity::Low,
            _ => Severity::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub vuln_id: String,
    pub pkg_name: String,
    pub installed_version: String,
    /// `None` when there is no fixed version yet.
    pub fixed_version: Option<String>,
    pub severity: Severity,
    pub title: Option<String>,
    pub url: Option<String>,
}

impl Finding {
    pub fn fixable(&self) -> bool {
        self.fixed_version.is_some()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Counts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub unknown: u32,
    pub critical_fixable: u32,
    pub high_fixable: u32,
}

impl Counts {
    pub fn total(&self) -> u32 {
        self.critical + self.high + self.medium + self.low + self.unknown
    }
}

pub fn counts(findings: &[Finding]) -> Counts {
    let mut c = Counts::default();
    for f in findings {
        match f.severity {
            Severity::Critical => {
                c.critical += 1;
                c.critical_fixable += u32::from(f.fixable());
            }
            Severity::High => {
                c.high += 1;
                c.high_fixable += u32::from(f.fixable());
            }
            Severity::Medium => c.medium += 1,
            Severity::Low => c.low += 1,
            Severity::Unknown => c.unknown += 1,
        }
    }
    c
}

/// One place an image is used: a container (Docker), a pod's or idle
/// template's container (Kubernetes), or the image alone (`container` empty,
/// not running) when nothing uses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUse {
    pub env_id: i64,
    /// A Swarm node's id; empty elsewhere.
    pub node: String,
    pub key: ImageKey,
    /// The first repo:tag, else the image id (Docker); the spec's image (Kubernetes).
    pub image_ref: String,
    /// The Docker image id.
    pub image_id: Option<String>,
    pub namespace: String,
    /// Compose project (Docker) or `Kind/name` (Kubernetes); may be empty.
    pub workload: String,
    pub container: String,
    pub running: bool,
    pub pull_secrets: Vec<String>,
    /// Kubernetes: `os/arch` of the Pod's node; `None` elsewhere.
    pub platform: Option<String>,
}

/// What one scan runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanTarget {
    pub env_id: i64,
    pub node: String,
    pub key: ImageKey,
    pub image_ref: String,
    /// The namespace whose pull secrets apply (Kubernetes).
    pub namespace: String,
    pub pull_secrets: Vec<String>,
    /// Kubernetes: the `os/arch` the scan pod must run on, if known.
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Arrival,
    Manual,
    Periodic,
    Enable,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Arrival => "arrival",
            Trigger::Manual => "manual",
            Trigger::Periodic => "periodic",
            Trigger::Enable => "enable",
        }
    }

    pub fn parse(s: &str) -> Option<Trigger> {
        [
            Trigger::Arrival,
            Trigger::Manual,
            Trigger::Periodic,
            Trigger::Enable,
        ]
        .into_iter()
        .find(|t| t.as_str() == s)
    }
}

/// Where an image's results stand, worst last in [`RowState::worst`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowState {
    Scanned,
    Queued,
    Scanning,
    Stale,
    NeverScanned,
    ScannerUnavailable,
    Failed,
}

impl RowState {
    fn badness(self) -> u8 {
        match self {
            RowState::Scanned => 0,
            RowState::Queued => 1,
            RowState::Scanning => 2,
            RowState::Stale => 3,
            RowState::NeverScanned => 4,
            RowState::ScannerUnavailable => 5,
            RowState::Failed => 6,
        }
    }

    /// The state an image row shows for several digests: the worst.
    pub fn worst(self, other: RowState) -> RowState {
        if other.badness() > self.badness() {
            other
        } else {
            self
        }
    }
}

/// Re-scan an image whose last good scan is older than this.
pub const RESCAN_SECS: i64 = 86_400;
/// A result older than this is shown as stale.
pub const STALE_SECS: i64 = 172_800;

/// The patch-queue order, compared descending: images with running
/// containers first, then more fixable criticals, fixable highs, and total.
pub fn rank(
    running: u32,
    critical_fixable: u32,
    high_fixable: u32,
    total: u32,
) -> (bool, u32, u32, u32) {
    (running > 0, critical_fixable, high_fixable, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, sev: Severity, fixed: Option<&str>) -> Finding {
        Finding {
            vuln_id: id.into(),
            pkg_name: "openssl".into(),
            installed_version: "3.0.13".into(),
            fixed_version: fixed.map(Into::into),
            severity: sev,
            title: None,
            url: None,
        }
    }

    #[test]
    fn key_prefers_repo_digest() {
        let k = image_key(1, "", &["nginx@sha256:abc".into()], "sha256:cfg");
        assert_eq!(k.as_str(), "sha256:abc");
        assert_eq!(k.digest(), Some("sha256:abc"));
    }

    #[test]
    fn key_local_when_no_digest() {
        let k = image_key(3, "n1", &[], "sha256:cfg");
        assert_eq!(k.as_str(), "local:3:n1:sha256:cfg");
        assert_eq!(k.digest(), None);
    }

    #[test]
    fn counts_split_fixable() {
        let c = counts(&[
            f("a", Severity::Critical, Some("1")),
            f("b", Severity::Critical, None),
            f("c", Severity::High, Some("2")),
            f("d", Severity::Low, None),
        ]);
        assert_eq!(
            c,
            Counts {
                critical: 2,
                high: 1,
                low: 1,
                critical_fixable: 1,
                high_fixable: 1,
                ..Default::default()
            }
        );
        assert_eq!(c.total(), 4);
    }

    #[test]
    fn rank_puts_running_before_more_criticals() {
        let running_one = rank(3, 1, 0, 1);
        let idle_many = rank(0, 9, 9, 50);
        let running_more = rank(1, 2, 0, 2);
        let mut v = [idle_many, running_one, running_more];
        v.sort_by(|a, b| b.cmp(a));
        assert_eq!(v, [running_more, running_one, idle_many]);
    }

    #[test]
    fn severity_orders_critical_highest_and_parses_trivy_spelling() {
        assert!(Severity::Critical > Severity::High);
        assert_eq!(Severity::parse("CRITICAL"), Severity::Critical);
        assert_eq!(Severity::parse("NEGLIGIBLE"), Severity::Unknown);
    }

    #[test]
    fn worst_state_wins() {
        assert_eq!(RowState::Scanned.worst(RowState::Failed), RowState::Failed);
        assert_eq!(
            RowState::ScannerUnavailable.worst(RowState::Queued),
            RowState::ScannerUnavailable
        );
    }
}
