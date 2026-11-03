mod common;

use async_trait::async_trait;
use common::*;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_ee::notify::{
    db::{self as ndb, DeliveryStatus, NewDestination, RuleRow},
    model::{DestinationKind, DestinationSecrets, PendingItem, PendingKind, SmtpConfig},
    notifier::Notifier,
    send::Transport,
};
use fleetops_ee::scan::{
    config::Effective,
    inventory::Inventory,
    model::{ImageUse, ScanTarget, Severity},
    runner::Runner,
};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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

#[derive(Default)]
struct FakeTransport {
    posts: Mutex<Vec<(String, Value)>>,
    failing: Mutex<HashSet<String>>,
}

#[async_trait]
impl Transport for FakeTransport {
    async fn post_json(&self, url: &str, body: &Value, _h: &[(String, String)]) -> Result<()> {
        if self.failing.lock().unwrap().contains(url) {
            return Err(Error::Upstream {
                status: 502,
                message: "answered 500".into(),
            });
        }
        self.posts
            .lock()
            .unwrap()
            .push((url.to_string(), body.clone()));
        Ok(())
    }
    async fn send_mail(
        &self,
        _: &SmtpConfig,
        _: &[String],
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<()> {
        Ok(())
    }
}

struct Fx {
    ee: Ee,
    t: Arc<FakeTransport>,
    n: Arc<Notifier>,
}

async fn fx(licensed: bool) -> Fx {
    let k = key();
    let t = Arc::new(FakeTransport::default());
    let ee = ee_notify_app(
        &k,
        StubEnvs::default(),
        (Arc::new(NoRunner), Arc::new(NoInventory)),
        t.clone(),
    )
    .await;
    if licensed {
        let admin = ee.admin().await;
        let r = ee
            .install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    let n = Notifier::new(Some(t.clone()));
    Fx { ee, t, n }
}

impl Fx {
    async fn slack(&self, name: &str) -> i64 {
        ndb::create_destination(
            &self.ee.store,
            &NewDestination {
                name: name.into(),
                kind: DestinationKind::Slack,
                host: Some("hooks.slack.com".into()),
                recipients: vec![],
                secrets: DestinationSecrets {
                    url: Some(format!("https://hooks.slack.com/{name}")),
                    ..Default::default()
                },
            },
            0,
        )
        .await
        .unwrap()
    }

    async fn rule(&self, dests: Vec<i64>) -> i64 {
        ndb::create_rule(
            &self.ee.store,
            &RuleRow {
                name: "prod".into(),
                enabled: true,
                envs: vec![],
                min_severity: Severity::Critical,
                fixable_only: true,
                running_only: true,
                destinations: dests,
            },
            0,
        )
        .await
        .unwrap()
    }

    async fn pending(&self, rule: i64, at: i64) {
        let item = PendingItem {
            repository: "nginx".into(),
            image_ref: "nginx:1".into(),
            env: "prod".into(),
            running: 1,
            cve: "CVE-9".into(),
            severity: Severity::Critical,
            pkg_name: "libssl3".into(),
            installed_version: "3.0.11".into(),
            fixed_version: Some("3.0.14".into()),
            title: None,
        };
        ndb::add_pending(&self.ee.store, rule, PendingKind::Cve, "CVE-9", &[item], at)
            .await
            .unwrap();
    }

    fn posts(&self) -> Vec<String> {
        self.t
            .posts
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.0.clone())
            .collect()
    }
}

#[tokio::test]
async fn due_group_becomes_one_delivery_per_destination() {
    let x = fx(true).await;
    let (a, b) = (x.slack("a").await, x.slack("b").await);
    let r = x.rule(vec![a, b]).await;
    x.pending(r, 0).await;
    x.n.tick(&x.ee.state, 121).await;
    let mut p = x.posts();
    p.sort();
    assert_eq!(
        p,
        ["https://hooks.slack.com/a", "https://hooks.slack.com/b"]
    );
    assert!(ndb::is_reported(&x.ee.store, r, "nginx", "CVE-9")
        .await
        .unwrap());
    assert!(ndb::due_pending(&x.ee.store, 10_000)
        .await
        .unwrap()
        .is_empty());
    let body = &x.t.posts.lock().unwrap()[0].1;
    assert!(body["text"]
        .as_str()
        .unwrap()
        .starts_with("CVE-9 (Critical)"));
}

#[tokio::test]
async fn quiet_window_holds_the_group() {
    let x = fx(true).await;
    let a = x.slack("a").await;
    let r = x.rule(vec![a]).await;
    x.pending(r, 0).await;
    x.n.tick(&x.ee.state, 60).await;
    assert!(x.posts().is_empty());
    x.n.tick(&x.ee.state, 121).await;
    assert_eq!(x.posts().len(), 1);
}

#[tokio::test]
async fn failing_destination_does_not_block_others() {
    let x = fx(true).await;
    let (a, b) = (x.slack("a").await, x.slack("b").await);
    x.t.failing
        .lock()
        .unwrap()
        .insert("https://hooks.slack.com/a".into());
    let r = x.rule(vec![a, b]).await;
    x.pending(r, 0).await;
    x.n.tick(&x.ee.state, 200).await;
    assert_eq!(
        x.posts(),
        ["https://hooks.slack.com/b"],
        "b delivered on the first tick"
    );
    x.n.tick(&x.ee.state, 200 + 119).await;
    x.n.tick(&x.ee.state, 200 + 120).await;
    x.n.tick(&x.ee.state, 200 + 720).await;
    let rows = ndb::recent_deliveries(&x.ee.store, 10).await.unwrap();
    let ra = rows.iter().find(|d| d.destination_id == Some(a)).unwrap();
    assert_eq!(ra.status, DeliveryStatus::Failed);
    assert_eq!(ra.attempts, 3);
    assert!(ra.error.as_deref().unwrap().contains("500"));
    let (da, _) = ndb::get_destination(&x.ee.store, a).await.unwrap();
    assert!(da.last_error.is_some(), "flagged");
    let (db_, _) = ndb::get_destination(&x.ee.store, b).await.unwrap();
    assert!(db_.last_error.is_none());
}

#[tokio::test]
async fn restart_resumes_batches_and_retries_once() {
    let x = fx(true).await;
    let (a, b) = (x.slack("a").await, x.slack("b").await);
    let r = x.rule(vec![a, b]).await;
    x.t.failing
        .lock()
        .unwrap()
        .insert("https://hooks.slack.com/a".into());
    x.pending(r, 0).await;
    x.n.tick(&x.ee.state, 200).await;
    // A new process: same store, fresh notifier; a now works.
    x.t.failing.lock().unwrap().clear();
    let again = Notifier::new(Some(x.t.clone()));
    again.tick(&x.ee.state, 200 + 120).await;
    again.tick(&x.ee.state, 200 + 120).await;
    let mut p = x.posts();
    p.sort();
    assert_eq!(
        p,
        ["https://hooks.slack.com/a", "https://hooks.slack.com/b"],
        "each once"
    );
}

#[tokio::test]
async fn unlicensed_sends_nothing() {
    let x = fx(false).await;
    let a = x.slack("a").await;
    let r = x.rule(vec![a]).await;
    x.pending(r, 0).await;
    x.n.tick(&x.ee.state, 1_000).await;
    assert!(x.posts().is_empty());
}

#[tokio::test]
async fn test_destination_reports_the_error() {
    let x = fx(true).await;
    let a = x.slack("a").await;
    x.n.test_destination(&x.ee.state, a).await.unwrap();
    assert_eq!(x.posts().len(), 1);
    x.t.failing
        .lock()
        .unwrap()
        .insert("https://hooks.slack.com/a".into());
    let e = x.n.test_destination(&x.ee.state, a).await.unwrap_err();
    assert!(e.to_string().contains("500"), "{e}");
}
