//! What the security pages show, built from uses and scan results.

use super::config::Mirror;
use super::db::{FindingRow, KeyState, Status};
use super::engine::ScannerStatus;
use super::model::{rank, Counts, ImageKey, ImageUse, RowState, Severity, STALE_SECS};
use fleetops_core::env::EnvKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SecuritySummary {
    /// Images with a fixable critical vulnerability and a running container.
    pub critical_fixable_running: u32,
    /// Images with a result.
    pub scanned: u32,
    pub failed: u32,
    /// Images without a result yet, including those waiting for a scanner.
    pub unscanned: u32,
    pub rows: Vec<ImageRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ImageRow {
    /// repo:tag (or the image id when it has none).
    pub image: String,
    /// How many digests the image has across environments.
    pub digests: u32,
    pub critical: u32,
    pub high: u32,
    pub critical_fixable: u32,
    pub high_fixable: u32,
    pub total: u32,
    /// Running containers or pods.
    pub running: u32,
    pub envs: Vec<String>,
    pub state: RowState,
    /// Whether any digest has a result: without one the counts mean nothing.
    pub has_result: bool,
    pub last_scan_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ImageDetail {
    pub image: String,
    pub digests: Vec<DigestDetail>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct DigestDetail {
    pub key: String,
    pub state: RowState,
    pub last_scan_at: Option<i64>,
    pub trivy_image: Option<String>,
    pub error: Option<String>,
    pub counts: Option<Counts>,
    pub findings: Vec<FindingView>,
    pub used_by: Vec<UseView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct FindingView {
    pub vuln_id: String,
    pub severity: Severity,
    pub pkg_name: String,
    pub installed_version: String,
    pub fixed_version: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub first_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct UseView {
    pub env: String,
    pub node: String,
    pub namespace: String,
    pub workload: String,
    pub container: String,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ImageBadge {
    pub image_id: String,
    /// The highest severity that has a fix.
    pub severity: Option<Severity>,
    /// Fixable critical and high vulnerabilities.
    pub fixable: u32,
    pub state: RowState,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct MirrorView {
    pub trivy_image: Option<String>,
    pub db_repository: Option<String>,
    pub java_db_repository: Option<String>,
    pub username: Option<String>,
    pub password_set: bool,
}

impl MirrorView {
    pub fn of(m: &Mirror, password_set: bool) -> Self {
        MirrorView {
            trivy_image: m.trivy_image.clone(),
            db_repository: m.db_repository.clone(),
            java_db_repository: m.java_db_repository.clone(),
            username: m.username.clone(),
            password_set,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct GlobalView {
    pub enabled: bool,
    pub mirror: Option<MirrorView>,
    /// The Trivy image scans use when no mirror names one.
    pub default_trivy_image: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct EnvSettingsView {
    pub env: String,
    pub kind: EnvKind,
    /// `None` inherits the global switch.
    pub enabled: Option<bool>,
    pub effective_enabled: bool,
    pub mirror: Option<MirrorView>,
    pub socket_path: Option<String>,
    pub status: ScannerStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SecuritySettings {
    pub global: GlobalView,
    pub envs: Vec<EnvSettingsView>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, ToSchema)]
pub struct MirrorBody {
    pub trivy_image: Option<String>,
    pub db_repository: Option<String>,
    pub java_db_repository: Option<String>,
    pub username: Option<String>,
    /// Write-only; omit to keep the saved one.
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
pub struct GlobalBody {
    pub enabled: bool,
    pub mirror: Option<MirrorBody>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
pub struct EnvBody {
    /// `None` inherits the global switch.
    pub enabled: Option<bool>,
    pub mirror: Option<MirrorBody>,
    pub socket_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
pub struct ScanNowBody {
    pub env: String,
    /// repo:tag or Docker image ids; omit for every image in the environment.
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ScanQueued {
    pub queued: u32,
}

/// One digest's standing.
pub struct KeyView {
    pub state: RowState,
    pub counts: Option<Counts>,
    pub last_scan_at: Option<i64>,
    pub error: Option<String>,
    pub trivy_image: Option<String>,
    pub done_scan: Option<i64>,
}

pub fn key_view(st: Option<&KeyState>, unavailable: bool, now: i64) -> KeyView {
    let done = st.and_then(|s| s.last_done.as_ref());
    let state = match st.map(|s| &s.last) {
        None if unavailable => RowState::ScannerUnavailable,
        None => RowState::NeverScanned,
        Some(l) => match l.status {
            Status::Queued if unavailable => RowState::ScannerUnavailable,
            Status::Queued => RowState::Queued,
            Status::Running => RowState::Scanning,
            Status::Failed => RowState::Failed,
            Status::Done => {
                if l.finished_at.is_some_and(|t| now - t > STALE_SECS) {
                    RowState::Stale
                } else {
                    RowState::Scanned
                }
            }
        },
    };
    KeyView {
        state,
        counts: done.and_then(|d| d.counts),
        last_scan_at: done.and_then(|d| d.finished_at),
        error: st
            .filter(|s| s.last.status == Status::Failed)
            .and_then(|s| s.last.error.clone()),
        trivy_image: done.and_then(|d| d.trivy_image.clone()),
        done_scan: done.map(|d| d.id),
    }
}

pub struct Ctx<'a> {
    pub states: &'a HashMap<ImageKey, KeyState>,
    pub unavailable: &'a HashSet<i64>,
    pub env_names: &'a HashMap<i64, String>,
    pub now: i64,
}

impl Ctx<'_> {
    fn key(&self, key: &ImageKey, uses: &[&ImageUse]) -> KeyView {
        let unavailable = uses.iter().any(|u| self.unavailable.contains(&u.env_id));
        key_view(self.states.get(key), unavailable, self.now)
    }
}

pub fn summarize(
    uses: &[ImageUse],
    c: &Ctx,
    running_only: bool,
    fixable_only: bool,
) -> SecuritySummary {
    let mut by_image: HashMap<&str, Vec<&ImageUse>> = HashMap::new();
    for u in uses {
        by_image.entry(u.image_ref.as_str()).or_default().push(u);
    }
    let mut rows = Vec::new();
    for (image, us) in by_image {
        let mut by_key: HashMap<&ImageKey, Vec<&ImageUse>> = HashMap::new();
        for u in &us {
            by_key.entry(&u.key).or_default().push(u);
        }
        let mut row = ImageRow {
            image: image.to_string(),
            digests: by_key.len() as u32,
            critical: 0,
            high: 0,
            critical_fixable: 0,
            high_fixable: 0,
            total: 0,
            running: us.iter().filter(|u| u.running).count() as u32,
            envs: us
                .iter()
                .filter_map(|u| c.env_names.get(&u.env_id).cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            state: RowState::Scanned,
            has_result: false,
            last_scan_at: None,
            error: None,
        };
        for (key, kus) in by_key {
            let v = c.key(key, &kus);
            row.state = row.state.worst(v.state);
            if let Some(n) = v.counts {
                row.has_result = true;
                row.critical = row.critical.max(n.critical);
                row.high = row.high.max(n.high);
                row.critical_fixable = row.critical_fixable.max(n.critical_fixable);
                row.high_fixable = row.high_fixable.max(n.high_fixable);
                row.total = row.total.max(n.total());
            }
            row.last_scan_at = match (row.last_scan_at, v.last_scan_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if row.error.is_none() {
                row.error = v.error;
            }
        }
        rows.push(row);
    }
    let summary_counts = |rows: &[ImageRow]| {
        (
            rows.iter()
                .filter(|r| r.critical_fixable > 0 && r.running > 0)
                .count() as u32,
            rows.iter().filter(|r| r.has_result).count() as u32,
            rows.iter().filter(|r| r.state == RowState::Failed).count() as u32,
            rows.iter()
                .filter(|r| !r.has_result && r.state != RowState::Failed)
                .count() as u32,
        )
    };
    let (critical_fixable_running, scanned, failed, unscanned) = summary_counts(&rows);
    rows.retain(|r| !running_only || r.running > 0);
    rows.retain(|r| {
        !fixable_only
            || !r.has_result
            || r.critical_fixable + r.high_fixable > 0
            || r.state != RowState::Scanned
    });
    rows.sort_by(|a, b| {
        rank(b.running, b.critical_fixable, b.high_fixable, b.total)
            .cmp(&rank(
                a.running,
                a.critical_fixable,
                a.high_fixable,
                a.total,
            ))
            .then_with(|| a.image.cmp(&b.image))
    });
    SecuritySummary {
        critical_fixable_running,
        scanned,
        failed,
        unscanned,
        rows,
    }
}

pub fn finding_views(mut rows: Vec<FindingRow>) -> Vec<FindingView> {
    rows.sort_by(|a, b| {
        b.finding
            .severity
            .cmp(&a.finding.severity)
            .then_with(|| b.finding.fixable().cmp(&a.finding.fixable()))
            .then_with(|| a.finding.vuln_id.cmp(&b.finding.vuln_id))
    });
    rows.into_iter()
        .map(|r| FindingView {
            vuln_id: r.finding.vuln_id,
            severity: r.finding.severity,
            pkg_name: r.finding.pkg_name,
            installed_version: r.finding.installed_version,
            fixed_version: r.finding.fixed_version,
            title: r.finding.title,
            url: r.finding.url,
            first_seen_at: r.first_seen_at,
        })
        .collect()
}

pub fn use_view(u: &ImageUse, env_names: &HashMap<i64, String>) -> UseView {
    UseView {
        env: env_names.get(&u.env_id).cloned().unwrap_or_default(),
        node: u.node.clone(),
        namespace: u.namespace.clone(),
        workload: u.workload.clone(),
        container: u.container.clone(),
        running: u.running,
    }
}

/// The badge of each Docker image id among `uses`.
pub fn badges(uses: &[ImageUse], c: &Ctx) -> Vec<ImageBadge> {
    let mut by_id: HashMap<&str, Vec<&ImageUse>> = HashMap::new();
    for u in uses {
        if let Some(id) = u.image_id.as_deref() {
            by_id.entry(id).or_default().push(u);
        }
    }
    let mut out: Vec<ImageBadge> = by_id
        .into_iter()
        .map(|(id, us)| {
            let v = c.key(&us[0].key, &us);
            let n = v.counts.unwrap_or_default();
            ImageBadge {
                image_id: id.to_string(),
                severity: if n.critical_fixable > 0 {
                    Some(Severity::Critical)
                } else if n.high_fixable > 0 {
                    Some(Severity::High)
                } else {
                    None
                },
                fixable: n.critical_fixable + n.high_fixable,
                state: v.state,
            }
        })
        .collect();
    out.sort_by(|a, b| a.image_id.cmp(&b.image_id));
    out
}
