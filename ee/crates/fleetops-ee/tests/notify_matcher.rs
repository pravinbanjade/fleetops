mod common;

use async_trait::async_trait;
use common::*;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Result};
use fleetops_ee::notify::{
    db::{self as ndb, NewDestination, RuleRow},
    matcher::{baseline_rule, Matcher},
    model::{DestinationKind, DestinationSecrets, PendingKind},
};
use fleetops_ee::scan::{
    config::Effective,
    db as sdb,
    engine::ScanEvents,
    inventory::Inventory,
    model::{Finding, ImageKey, ImageUse, ScanTarget, Severity, Trigger},
    runner::Runner,
};
use std::sync::Arc;

struct NoRunner;
#[async_trait]
impl Runner for NoRunner {
    async fn prepare(&self, _: &Arc<dyn Environment>, _: &str, _: &Effective) -> Result<()> {
        Ok(())
    }
    async fn scan(
        &self,
        _: &Arc<dyn Environment>,
        _: &ScanTarget,
        _: &Effective,
        _: i64,
    ) -> Result<Vec<u8>> {
        Ok(b"{}".to_vec())
    }
    async fn cleanup_orphans(&self, _: &Arc<dyn Environment>) -> Result<()> {
        Ok(())
    }
}
struct NoInventory;
#[async_trait]
impl Inventory for NoInventory {
    async fn uses(&self, _: &Arc<dyn Environment>) -> Result<Vec<ImageUse>> {
        Ok(vec![])
    }
}

fn f(id: &str, sev: Severity) -> Finding {
    Finding {
        vuln_id: id.into(),
        pkg_name: "libssl3".into(),
        installed_version: "3.0.11".into(),
        fixed_version: Some("3.0.14".into()),
        severity: sev,
        title: None,
        url: None,
    }
}

fn use_of(env_id: i64, key: &str, image: &str, running: bool) -> ImageUse {
    ImageUse {
        env_id,
        node: String::new(),
        key: ImageKey(key.into()),
        image_ref: image.into(),
        image_id: None,
        namespace: String::new(),
        workload: String::new(),
        container: "c".into(),
        running,
        pull_secrets: vec![],
        platform: None,
    }
}

struct Fx {
    ee: Ee,
    dest: i64,
}

async fn fx(licensed: bool) -> Fx {
    let k = key();
    let ee = ee_scan_app(
        &k,
        StubEnvs::default(),
        (Arc::new(NoRunner), Arc::new(NoInventory)),
    )
    .await;
    if licensed {
        let admin = ee.admin().await;
        let r = ee
            .install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    let dest = ndb::create_destination(
        &ee.store,
        &NewDestination {
            name: "on-call".into(),
            kind: DestinationKind::Slack,
            host: None,
            recipients: vec![],
            secrets: DestinationSecrets::default(),
        },
        0,
    )
    .await
    .unwrap();
    Fx { ee, dest }
}

impl Fx {
    async fn rule(&self, name: &str, envs: Vec<i64>, min: Severity) -> i64 {
        let id = ndb::create_rule(
            &self.ee.store,
            &RuleRow {
                name: name.into(),
                enabled: true,
                envs,
                min_severity: min,
                fixable_only: true,
                running_only: true,
                destinations: vec![self.dest],
            },
            now_unix(),
        )
        .await
        .unwrap();
        baseline_rule(&self.ee.state, id).await.unwrap();
        id
    }

    /// Records a finished scan of `key` with `findings`, then tells the matcher.
    async fn scanned(&self, env_id: i64, key: &str, findings: &[Finding]) {
        let t = ScanTarget {
            env_id,
            node: String::new(),
            key: ImageKey(key.into()),
            image_ref: "x".into(),
            namespace: String::new(),
            pull_secrets: vec![],
            platform: None,
        };
        let id = sdb::insert_scan(&self.ee.store, &t, Trigger::Arrival, now_unix())
            .await
            .unwrap();
        sdb::finish_ok(&self.ee.store, id, "trivy", findings, now_unix())
            .await
            .unwrap();
        Matcher
            .scan_finished(&self.ee.state, &ImageKey(key.into()), findings)
            .await;
    }

    async fn uses(&self, env_id: i64, after: Vec<ImageUse>, enabling: bool) {
        let before = sdb::uses_for_envs(&self.ee.store, &[env_id]).await.unwrap();
        sdb::replace_uses(&self.ee.store, env_id, &after)
            .await
            .unwrap();
        Matcher
            .inventory_changed(&self.ee.state, env_id, &before, &after, enabling)
            .await;
    }

    async fn pending(&self) -> Vec<(i64, PendingKind, String, usize)> {
        let mut v: Vec<_> = ndb::due_pending(&self.ee.store, now_unix() + 10_000)
            .await
            .unwrap()
            .into_iter()
            .map(|g| (g.rule_id, g.kind, g.group_key, g.items.len()))
            .collect();
        v.sort_by(|a, b| a.2.cmp(&b.2));
        v
    }
}

#[tokio::test]
async fn enabling_scanning_sends_nothing() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    let uses: Vec<_> = (0..50)
        .map(|i| use_of(prod, &format!("sha256:{i}"), &format!("app{i}:1"), true))
        .collect();
    x.uses(prod, uses, true).await;
    for i in 0..50 {
        x.scanned(
            prod,
            &format!("sha256:{i}"),
            &[f("CVE-1", Severity::Critical)],
        )
        .await;
    }
    assert!(
        x.pending().await.is_empty(),
        "the existing state is not news"
    );
    x.scanned(
        prod,
        "sha256:7",
        &[
            f("CVE-1", Severity::Critical),
            f("CVE-NEW", Severity::Critical),
        ],
    )
    .await;
    assert_eq!(x.pending().await.len(), 1);
}

#[tokio::test]
async fn new_rule_baselines_silently() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.uses(prod, vec![use_of(prod, "sha256:a", "nginx:1", true)], false)
        .await;
    x.scanned(prod, "sha256:a", &[f("CVE-1", Severity::Critical)])
        .await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    assert!(x.pending().await.is_empty());
    x.scanned(prod, "sha256:a", &[f("CVE-1", Severity::Critical)])
        .await;
    assert!(x.pending().await.is_empty(), "re-scan of a known CVE");
}

#[tokio::test]
async fn new_cve_on_known_repository_queues_one_cve_group() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(
        prod,
        vec![
            use_of(prod, "sha256:a", "nginx:1", true),
            use_of(prod, "sha256:b", "app/api:2", true),
        ],
        true,
    )
    .await;
    x.scanned(prod, "sha256:a", &[]).await;
    x.scanned(prod, "sha256:b", &[]).await;
    x.scanned(prod, "sha256:a", &[f("CVE-9", Severity::Critical)])
        .await;
    x.scanned(prod, "sha256:b", &[f("CVE-9", Severity::Critical)])
        .await;
    let p = x.pending().await;
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!(
        (p[0].1, p[0].2.as_str(), p[0].3),
        (PendingKind::Cve, "CVE-9", 2)
    );
}

#[tokio::test]
async fn rebuild_with_known_cve_is_silent() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(
        prod,
        vec![use_of(prod, "sha256:v1", "app/api:1", true)],
        true,
    )
    .await;
    x.scanned(prod, "sha256:v1", &[f("CVE-1", Severity::Critical)])
        .await;
    x.uses(
        prod,
        vec![use_of(prod, "sha256:v2", "app/api:2", true)],
        false,
    )
    .await;
    x.scanned(prod, "sha256:v2", &[f("CVE-1", Severity::Critical)])
        .await;
    assert!(x.pending().await.is_empty(), "same repository, same CVE");
    x.uses(
        prod,
        vec![use_of(prod, "sha256:v3", "app/api:3", true)],
        false,
    )
    .await;
    x.scanned(
        prod,
        "sha256:v3",
        &[
            f("CVE-1", Severity::Critical),
            f("CVE-2", Severity::Critical),
        ],
    )
    .await;
    let p = x.pending().await;
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].1, p[0].2.as_str()), (PendingKind::Cve, "CVE-2"));
}

#[tokio::test]
async fn new_repository_queues_one_image_group() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(
        prod,
        vec![use_of(prod, "sha256:w", "app/worker:1", true)],
        false,
    )
    .await;
    let many: Vec<_> = (0..25)
        .map(|i| f(&format!("CVE-{i:02}"), Severity::Critical))
        .collect();
    x.scanned(prod, "sha256:w", &many).await;
    let p = x.pending().await;
    assert_eq!(p.len(), 1, "one summary, not 25 messages");
    assert_eq!(
        (p[0].1, p[0].2.as_str(), p[0].3),
        (PendingKind::Image, "app/worker", 25)
    );
}

#[tokio::test]
async fn promotion_alerts_the_prod_rule_once() {
    let x = fx(true).await;
    let dev = x.ee.env("dev", "docker-local").await;
    let prod = x.ee.env("prod", "docker-agent").await;
    let dev_rule = x.rule("dev", vec![dev], Severity::Critical).await;
    let prod_rule = x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(dev, vec![use_of(dev, "sha256:d", "app/api:5", true)], true)
        .await;
    x.scanned(dev, "sha256:d", &[f("CVE-1", Severity::Critical)])
        .await;
    assert!(x.pending().await.is_empty());
    x.uses(
        prod,
        vec![use_of(prod, "sha256:d", "app/api:5", true)],
        false,
    )
    .await;
    let p = x.pending().await;
    assert_eq!(p.len(), 1, "{p:?}");
    assert_eq!((p[0].0, p[0].1), (prod_rule, PendingKind::Image));
    assert_ne!(p[0].0, dev_rule);
    // The flush records it; until then, a second inventory adds nothing new.
    x.uses(
        prod,
        vec![use_of(prod, "sha256:d", "app/api:5", true)],
        false,
    )
    .await;
    assert_eq!(x.pending().await.len(), 1);
}

#[tokio::test]
async fn rule_threshold_and_scope_filter() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    let other = x.ee.env("other", "docker-agent").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(
        prod,
        vec![
            use_of(prod, "sha256:a", "nginx:1", true),
            use_of(prod, "sha256:idle", "idle:1", false),
        ],
        true,
    )
    .await;
    x.uses(
        other,
        vec![use_of(other, "sha256:o", "other:1", true)],
        true,
    )
    .await;
    x.scanned(prod, "sha256:a", &[]).await;
    x.scanned(prod, "sha256:a", &[f("CVE-LOW", Severity::Low)])
        .await;
    x.scanned(prod, "sha256:idle", &[f("CVE-IDLE", Severity::Critical)])
        .await;
    x.scanned(other, "sha256:o", &[f("CVE-OTHER", Severity::Critical)])
        .await;
    assert!(x.pending().await.is_empty());
}

#[tokio::test]
async fn unlicensed_matches_nothing() {
    let x = fx(false).await;
    let prod = x.ee.env("prod", "docker-local").await;
    ndb::create_rule(
        &x.ee.store,
        &RuleRow {
            name: "prod".into(),
            enabled: true,
            envs: vec![prod],
            min_severity: Severity::Critical,
            fixable_only: true,
            running_only: true,
            destinations: vec![x.dest],
        },
        0,
    )
    .await
    .unwrap();
    x.uses(
        prod,
        vec![use_of(prod, "sha256:w", "app/worker:1", true)],
        false,
    )
    .await;
    x.scanned(prod, "sha256:w", &[f("CVE-1", Severity::Critical)])
        .await;
    assert!(x.pending().await.is_empty());
}

#[tokio::test]
async fn baseline_covers_every_digest_of_a_repository() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(
        prod,
        vec![
            use_of(prod, "sha256:v1", "app/api:1", true),
            use_of(prod, "sha256:v2", "app/api:2", true),
        ],
        true,
    )
    .await;
    x.scanned(prod, "sha256:v1", &[f("CVE-1", Severity::Critical)])
        .await;
    x.scanned(
        prod,
        "sha256:v2",
        &[
            f("CVE-1", Severity::Critical),
            f("CVE-2", Severity::Critical),
        ],
    )
    .await;
    assert!(
        x.pending().await.is_empty(),
        "both digests are the existing state"
    );
    x.scanned(
        prod,
        "sha256:v2",
        &[
            f("CVE-1", Severity::Critical),
            f("CVE-2", Severity::Critical),
            f("CVE-3", Severity::Critical),
        ],
    )
    .await;
    assert_eq!(x.pending().await.len(), 1, "only the genuinely new CVE");
}

#[tokio::test]
async fn a_restart_does_not_rebaseline() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(prod, vec![use_of(prod, "sha256:a", "nginx:1", true)], true)
        .await;
    x.scanned(prod, "sha256:a", &[]).await;
    // A new image arrives; its scan is still queued when the server restarts,
    // and the engine's first inventory after a restart says "enabling".
    let both = vec![
        use_of(prod, "sha256:a", "nginx:1", true),
        use_of(prod, "sha256:w", "app/worker:1", true),
    ];
    x.uses(prod, both.clone(), false).await;
    x.uses(prod, both, true).await;
    x.scanned(prod, "sha256:w", &[f("CVE-1", Severity::Critical)])
        .await;
    let p = x.pending().await;
    assert_eq!(p.len(), 1, "the new image still alerts: {p:?}");
    assert_eq!(p[0].1, PendingKind::Image);
}

#[tokio::test]
async fn turning_scanning_off_and_on_rebaselines() {
    let x = fx(true).await;
    let prod = x.ee.env("prod", "docker-local").await;
    x.rule("prod", vec![prod], Severity::Critical).await;
    x.uses(prod, vec![use_of(prod, "sha256:a", "nginx:1", true)], true)
        .await;
    x.scanned(prod, "sha256:a", &[]).await;
    Matcher.scanning_disabled(&x.ee.state, prod).await;
    x.uses(prod, vec![], false).await;
    x.uses(prod, vec![use_of(prod, "sha256:a", "nginx:1", true)], true)
        .await;
    x.scanned(prod, "sha256:a", &[f("CVE-OLD", Severity::Critical)])
        .await;
    assert!(
        x.pending().await.is_empty(),
        "turning it on again is a fresh start"
    );
}
