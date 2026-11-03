//! What notifications are about: repositories, rules, and the messages sent.

use crate::scan::model::{Finding, ImageUse, Severity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;

/// At most this many images or CVEs are listed in one message.
pub const LIST_MAX: usize = 20;

/// An image reference without tag or digest: what "already reported" is
/// keyed on, so rebuilds of the same repository stay quiet. Docker Hub's
/// implicit `docker.io/` and `library/` are dropped. A bare image id (a
/// locally built image with no tag) stands for itself.
pub fn repository(image_ref: &str, image_id: Option<&str>) -> String {
    let r = image_ref.trim();
    if r.is_empty() || r.starts_with("sha256:") {
        return image_id.unwrap_or(r).to_string();
    }
    let r = r.split('@').next().unwrap_or(r);
    let slash = r.rfind('/').map_or(0, |i| i + 1);
    let r = match r[slash..].rfind(':') {
        Some(i) => &r[..slash + i],
        None => r,
    };
    let r = r.strip_prefix("docker.io/").unwrap_or(r);
    let r = r.strip_prefix("index.docker.io/").unwrap_or(r);
    r.strip_prefix("library/").unwrap_or(r).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DestinationKind {
    Slack,
    Email,
    /// Generic JSON.
    Webhook,
    PagerDuty,
    Teams,
    Discord,
}

impl DestinationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DestinationKind::Slack => "slack",
            DestinationKind::Email => "email",
            DestinationKind::Webhook => "webhook",
            DestinationKind::PagerDuty => "pagerduty",
            DestinationKind::Teams => "teams",
            DestinationKind::Discord => "discord",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        use DestinationKind::*;
        [Slack, Email, Webhook, PagerDuty, Teams, Discord]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub id: i64,
    pub name: String,
    pub kind: DestinationKind,
    /// The URL's host, shown so admins can tell destinations apart.
    pub host: Option<String>,
    pub recipients: Vec<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
}

/// Sealed at rest; never returned by the API.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationSecrets {
    pub url: Option<String>,
    pub signing_secret: Option<String>,
    pub routing_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    Starttls,
    Tls,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    /// Environment ids; empty is every environment.
    pub envs: Vec<i64>,
    pub min_severity: Severity,
    pub fixable_only: bool,
    pub running_only: bool,
    pub destinations: Vec<i64>,
}

/// Whether `u` is somewhere `rule` watches.
pub fn in_scope(rule: &Rule, u: &ImageUse) -> bool {
    (rule.envs.is_empty() || rule.envs.contains(&u.env_id)) && (!rule.running_only || u.running)
}

/// Whether `f` is bad enough for `rule`.
pub fn matches(rule: &Rule, f: &Finding) -> bool {
    f.severity >= rule.min_severity && (!rule.fixable_only || f.fixable())
}

/// One (repository, CVE) waiting in a batch, with where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingItem {
    pub repository: String,
    pub image_ref: String,
    pub env: String,
    pub running: u32,
    pub cve: String,
    pub severity: Severity,
    pub pkg_name: String,
    pub installed_version: String,
    pub fixed_version: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingKind {
    /// One CVE across repositories the rule knows.
    Cve,
    /// One repository new to the rule.
    Image,
}

impl PendingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingKind::Cve => "cve",
            PendingKind::Image => "image",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGroup {
    pub rule_id: i64,
    pub kind: PendingKind,
    /// The CVE id, or the repository.
    pub group_key: String,
    pub items: Vec<PendingItem>,
    pub first_at: i64,
    pub last_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageLine {
    pub image_ref: String,
    pub repository: String,
    pub envs: Vec<String>,
    pub running: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CveLine {
    pub cve: String,
    pub severity: Severity,
    pub pkg_name: String,
    pub installed_version: String,
    pub fixed_version: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    NewCve {
        rule: RuleRef,
        cve: String,
        severity: Severity,
        fixed_version: Option<String>,
        title: Option<String>,
        /// (package, installed version).
        packages: Vec<(String, String)>,
        images: Vec<ImageLine>,
        /// Images beyond [`LIST_MAX`].
        more: u32,
        fixable_only: bool,
    },
    NewImage {
        rule: RuleRef,
        repository: String,
        image_ref: String,
        envs: Vec<String>,
        running: u32,
        cves: Vec<CveLine>,
        more: u32,
        fixable_only: bool,
    },
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "Critical",
        Severity::High => "High",
        Severity::Medium => "Medium",
        Severity::Low => "Low",
        Severity::Unknown => "Unknown",
    }
}

impl Message {
    pub fn rule(&self) -> &RuleRef {
        match self {
            Message::NewCve { rule, .. } | Message::NewImage { rule, .. } => rule,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Message::NewCve {
                cve,
                severity,
                fixed_version,
                ..
            } => match fixed_version {
                Some(v) => format!("{cve} ({}) — fix available: {v}", severity_word(*severity)),
                None => format!("{cve} ({}) — no fix", severity_word(*severity)),
            },
            Message::NewImage {
                image_ref,
                cves,
                more,
                fixable_only,
                ..
            } => {
                let mut by: BTreeMap<std::cmp::Reverse<Severity>, u32> = BTreeMap::new();
                for c in cves {
                    *by.entry(std::cmp::Reverse(c.severity)).or_default() += 1;
                }
                // Listed CVEs only; `more` is added to the worst severity's count.
                if let Some(first) = by.values_mut().next() {
                    *first += more;
                }
                let n = by.len();
                let counts: Vec<String> = by
                    .iter()
                    .enumerate()
                    .map(|(i, (s, c))| {
                        let w = severity_word(s.0).to_lowercase();
                        if i + 1 == n {
                            format!("{c} {w} CVE{}", if *c == 1 && n == 1 { "" } else { "s" })
                        } else {
                            format!("{c} {w}")
                        }
                    })
                    .collect();
                let fixes = if *fixable_only { " with fixes" } else { "" };
                format!("New image {image_ref} — {}{fixes}", counts.join(", "))
            }
        }
    }
}

/// The message a due batch becomes.
pub fn build(group: &PendingGroup, rule: &RuleRef, fixable_only: bool) -> Message {
    match group.kind {
        PendingKind::Cve => {
            let first = &group.items[0];
            let mut images: BTreeMap<String, ImageLine> = BTreeMap::new();
            let mut packages: BTreeSet<(String, String)> = BTreeSet::new();
            for i in &group.items {
                packages.insert((i.pkg_name.clone(), i.installed_version.clone()));
                let line = images
                    .entry(i.image_ref.clone())
                    .or_insert_with(|| ImageLine {
                        image_ref: i.image_ref.clone(),
                        repository: i.repository.clone(),
                        envs: vec![],
                        running: 0,
                    });
                if !line.envs.contains(&i.env) {
                    line.envs.push(i.env.clone());
                    line.envs.sort();
                }
                line.running += i.running;
            }
            let mut images: Vec<ImageLine> = images.into_values().collect();
            images.sort_by(|a, b| {
                b.running
                    .cmp(&a.running)
                    .then_with(|| a.image_ref.cmp(&b.image_ref))
            });
            let more = images.len().saturating_sub(LIST_MAX) as u32;
            images.truncate(LIST_MAX);
            Message::NewCve {
                rule: rule.clone(),
                cve: first.cve.clone(),
                severity: first.severity,
                fixed_version: group.items.iter().find_map(|i| i.fixed_version.clone()),
                title: first.title.clone(),
                packages: packages.into_iter().collect(),
                images,
                more,
                fixable_only,
            }
        }
        PendingKind::Image => {
            let first = &group.items[0];
            let mut envs: BTreeSet<String> = BTreeSet::new();
            let mut running: BTreeMap<String, u32> = BTreeMap::new();
            let mut cves: BTreeMap<String, CveLine> = BTreeMap::new();
            for i in &group.items {
                envs.insert(i.env.clone());
                running.insert(
                    i.env.clone(),
                    running.get(&i.env).copied().unwrap_or(0).max(i.running),
                );
                cves.entry(i.cve.clone()).or_insert_with(|| CveLine {
                    cve: i.cve.clone(),
                    severity: i.severity,
                    pkg_name: i.pkg_name.clone(),
                    installed_version: i.installed_version.clone(),
                    fixed_version: i.fixed_version.clone(),
                    title: i.title.clone(),
                });
            }
            let mut cves: Vec<CveLine> = cves.into_values().collect();
            cves.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.cve.cmp(&b.cve)));
            let more = cves.len().saturating_sub(LIST_MAX) as u32;
            cves.truncate(LIST_MAX);
            Message::NewImage {
                rule: rule.clone(),
                repository: group.group_key.clone(),
                image_ref: first.image_ref.clone(),
                envs: envs.into_iter().collect(),
                running: running.values().sum(),
                cves,
                more,
                fixable_only,
            }
        }
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub fn image_link(public: &str, image_ref: &str) -> String {
    format!(
        "{}/security/image?image={}",
        public.trim_end_matches('/'),
        enc(image_ref)
    )
}

pub fn security_link(public: &str, envs: &[String]) -> String {
    let base = format!("{}/security", public.trim_end_matches('/'));
    if envs.is_empty() {
        base
    } else {
        format!("{base}?envs={}", enc(&envs.join(",")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::model::ImageKey;

    fn rule() -> Rule {
        Rule {
            id: 1,
            name: "Prod → on-call".into(),
            enabled: true,
            envs: vec![1],
            min_severity: Severity::Critical,
            fixable_only: true,
            running_only: true,
            destinations: vec![1],
        }
    }

    fn item(repo: &str, env: &str, cve: &str, sev: Severity, running: u32) -> PendingItem {
        PendingItem {
            repository: repo.into(),
            image_ref: format!("{repo}:1"),
            env: env.into(),
            running,
            cve: cve.into(),
            severity: sev,
            pkg_name: "libssl3".into(),
            installed_version: "3.0.11".into(),
            fixed_version: Some("3.0.14".into()),
            title: None,
        }
    }

    fn group(kind: PendingKind, key: &str, items: Vec<PendingItem>) -> PendingGroup {
        PendingGroup {
            rule_id: 1,
            kind,
            group_key: key.into(),
            items,
            first_at: 0,
            last_at: 0,
        }
    }

    fn rref() -> RuleRef {
        RuleRef {
            id: 1,
            name: "Prod".into(),
        }
    }

    #[test]
    fn repository_normalises_every_spelling() {
        assert_eq!(repository("nginx:1.25", None), "nginx");
        assert_eq!(
            repository("docker.io/library/nginx@sha256:abc", None),
            "nginx"
        );
        assert_eq!(repository("library/nginx", None), "nginx");
        assert_eq!(
            repository("reg.local:5000/app/api:2.3", None),
            "reg.local:5000/app/api"
        );
        assert_eq!(repository("docker.io/acme/x:1", None), "acme/x");
        assert_eq!(repository("sha256:cfg", Some("sha256:cfg")), "sha256:cfg");
        assert_eq!(repository("app/api:2.3@sha256:d", None), "app/api");
    }

    #[test]
    fn scope_and_match_follow_the_rule() {
        let mut u = ImageUse {
            env_id: 1,
            node: String::new(),
            key: ImageKey("sha256:d".into()),
            image_ref: "nginx:1".into(),
            image_id: None,
            namespace: String::new(),
            workload: String::new(),
            container: "c".into(),
            running: true,
            pull_secrets: vec![],
        };
        assert!(in_scope(&rule(), &u));
        u.running = false;
        assert!(!in_scope(&rule(), &u), "running only");
        u.running = true;
        u.env_id = 2;
        assert!(!in_scope(&rule(), &u), "other environment");
        let all = Rule {
            envs: vec![],
            ..rule()
        };
        assert!(in_scope(&all, &u), "empty is every environment");
        let f = |sev, fixed: Option<&str>| Finding {
            vuln_id: "CVE-1".into(),
            pkg_name: "p".into(),
            installed_version: "1".into(),
            fixed_version: fixed.map(Into::into),
            severity: sev,
            title: None,
            url: None,
        };
        assert!(matches(&rule(), &f(Severity::Critical, Some("2"))));
        assert!(!matches(&rule(), &f(Severity::High, Some("2"))));
        assert!(!matches(&rule(), &f(Severity::Critical, None)), "fix only");
    }

    #[test]
    fn title_new_cve_with_and_without_fix() {
        let m = build(
            &group(
                PendingKind::Cve,
                "CVE-2024-5535",
                vec![item(
                    "nginx",
                    "prod",
                    "CVE-2024-5535",
                    Severity::Critical,
                    2,
                )],
            ),
            &rref(),
            true,
        );
        assert_eq!(
            m.title(),
            "CVE-2024-5535 (Critical) — fix available: 3.0.14"
        );
        let mut i = item("nginx", "prod", "CVE-2024-5535", Severity::Critical, 2);
        i.fixed_version = None;
        let m = build(
            &group(PendingKind::Cve, "CVE-2024-5535", vec![i]),
            &rref(),
            false,
        );
        assert_eq!(m.title(), "CVE-2024-5535 (Critical) — no fix");
    }

    #[test]
    fn title_new_image_counts_by_severity() {
        let items = vec![
            item("app/worker", "prod", "CVE-1", Severity::Critical, 1),
            item("app/worker", "prod", "CVE-2", Severity::Critical, 1),
            item("app/worker", "prod", "CVE-3", Severity::High, 1),
        ];
        let m = build(
            &group(PendingKind::Image, "app/worker", items),
            &rref(),
            true,
        );
        assert_eq!(
            m.title(),
            "New image app/worker:1 — 2 critical, 1 high CVEs with fixes"
        );
        let one = build(
            &group(
                PendingKind::Image,
                "app/worker",
                vec![item("app/worker", "prod", "CVE-1", Severity::Critical, 1)],
            ),
            &rref(),
            false,
        );
        assert_eq!(one.title(), "New image app/worker:1 — 1 critical CVE");
    }

    #[test]
    fn build_new_cve_lists_each_image_once_with_envs() {
        let m = build(
            &group(
                PendingKind::Cve,
                "CVE-9",
                vec![
                    item("nginx", "prod", "CVE-9", Severity::Critical, 2),
                    item("nginx", "edge", "CVE-9", Severity::Critical, 1),
                    item("app/api", "prod", "CVE-9", Severity::Critical, 5),
                ],
            ),
            &rref(),
            true,
        );
        let Message::NewCve { images, more, .. } = m else {
            panic!()
        };
        assert_eq!(more, 0);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].image_ref, "app/api:1", "most running first");
        assert_eq!(images[1].envs, ["edge", "prod"]);
        assert_eq!(images[1].running, 3);
    }

    #[test]
    fn lists_cap_at_20_with_more() {
        let items: Vec<_> = (0..25)
            .map(|i| item(&format!("r{i:02}"), "prod", "CVE-9", Severity::Critical, 1))
            .collect();
        let Message::NewCve { images, more, .. } =
            build(&group(PendingKind::Cve, "CVE-9", items), &rref(), true)
        else {
            panic!()
        };
        assert_eq!((images.len(), more), (20, 5));
        let items: Vec<_> = (0..23)
            .map(|i| item("app", "prod", &format!("CVE-{i:02}"), Severity::High, 1))
            .collect();
        let Message::NewImage { cves, more, .. } =
            build(&group(PendingKind::Image, "app", items), &rref(), true)
        else {
            panic!()
        };
        assert_eq!((cves.len(), more), (20, 3));
    }

    #[test]
    fn links_encode_the_image() {
        assert_eq!(
            image_link("https://fleet.test/", "app/api:1"),
            "https://fleet.test/security/image?image=app%2Fapi%3A1"
        );
        assert_eq!(
            security_link("https://fleet.test", &["prod".into(), "edge".into()]),
            "https://fleet.test/security?envs=prod%2Cedge"
        );
    }
}
