mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_ee::notify::{model::SmtpConfig, send::Transport};
use fleetops_ee::scan::{
    config::Effective,
    db as sdb,
    inventory::Inventory,
    model::{Finding, ImageKey, ImageUse, ScanTarget, Severity, Trigger},
    runner::Runner,
};
use serde_json::{json, Value};
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
struct Fake {
    fail: Mutex<bool>,
}
#[async_trait]
impl Transport for Fake {
    async fn post_json(&self, _: &str, _: &Value, _: &[(String, String)]) -> Result<()> {
        if *self.fail.lock().unwrap() {
            return Err(Error::Upstream {
                status: 502,
                message: "answered 404 (hooks.slack.com): no_team".into(),
            });
        }
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

const SLACK_URL: &str = "https://hooks.slack.com/services/T0/B0/SECRETpart";

struct Fx {
    ee: Ee,
    admin: Session,
    fake: Arc<Fake>,
}

async fn fx(licensed: bool) -> Fx {
    let k = key();
    let fake = Arc::new(Fake::default());
    let ee = ee_notify_app(
        &k,
        StubEnvs::default(),
        (Arc::new(NoRunner), Arc::new(NoInventory)),
        fake.clone(),
    )
    .await;
    let admin = ee.admin().await;
    if licensed {
        let r = ee
            .install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    Fx { ee, admin, fake }
}

impl Fx {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        s: &Session,
    ) -> (StatusCode, Value, String) {
        let r = self
            .ee
            .send(
                method,
                uri,
                body.map(|b| ("application/json", b.to_string())),
                Some(s),
            )
            .await;
        (r.status, r.json(), r.text)
    }

    async fn slack(&self) -> i64 {
        let (st, v, t) = self
            .send(
                "POST",
                "/api/v1/ee/notifications/destinations",
                Some(json!({"name": "on-call", "kind": "slack", "url": SLACK_URL})),
                &self.admin,
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{t}");
        v["id"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn admin_only() {
    let x = fx(true).await;
    x.ee.user(&x.admin, "bob").await;
    let bob = x.ee.login("bob").await;
    for uri in [
        "/api/v1/ee/notifications/destinations",
        "/api/v1/ee/notifications/rules",
        "/api/v1/ee/notifications/deliveries",
    ] {
        let (st, _, _) = x.send("GET", uri, None, &bob).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn unlicensed_writes_refused_reads_and_deletes_allowed() {
    let x = fx(true).await;
    let id = x.slack().await;
    // Lapse: a fresh unlicensed server on the same data is simulated by deleting the license.
    let (st, _, _) = x.send("DELETE", "/api/v1/ee/license", None, &x.admin).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/destinations",
            Some(json!({"name": "x", "kind": "slack", "url": SLACK_URL})),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, v, _) = x
        .send(
            "GET",
            "/api/v1/ee/notifications/destinations",
            None,
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 1, "leftovers stay visible");
    let (st, _, _) = x
        .send(
            "DELETE",
            &format!("/api/v1/ee/notifications/destinations/{id}"),
            None,
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn secrets_never_returned() {
    let x = fx(true).await;
    x.slack().await;
    let (_, v, text) = x
        .send(
            "GET",
            "/api/v1/ee/notifications/destinations",
            None,
            &x.admin,
        )
        .await;
    assert!(!text.contains("SECRETpart"), "{text}");
    assert_eq!(v[0]["host"], "hooks.slack.com");
    assert_eq!(v[0]["secret_set"], true);
}

#[tokio::test]
async fn url_scheme_rules() {
    let x = fx(true).await;
    let (st, _, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/destinations",
            Some(json!({"name": "s", "kind": "slack", "url": "http://hooks.slack.com/x"})),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, t) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/destinations",
            Some(json!({"name": "g", "kind": "webhook", "url": "http://tools.internal/hook"})),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    let (st, _, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/destinations",
            Some(json!({"name": "m", "kind": "email", "recipients": ["not-an-address"]})),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rule_needs_a_destination() {
    let x = fx(true).await;
    let body = json!({"name": "prod", "enabled": true, "envs": [], "min_severity": "critical", "fixable_only": true, "running_only": true, "destinations": []});
    let (st, _, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/rules",
            Some(body),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

async fn seed_known_cve(x: &Fx) -> i64 {
    let e = x.ee.env("prod", "docker-local").await;
    let u = ImageUse {
        env_id: e,
        node: String::new(),
        key: ImageKey("sha256:a".into()),
        image_ref: "nginx:1".into(),
        image_id: None,
        namespace: String::new(),
        workload: String::new(),
        container: "c".into(),
        running: true,
        pull_secrets: vec![],
    };
    sdb::replace_uses(&x.ee.store, e, &[u]).await.unwrap();
    let t = ScanTarget {
        env_id: e,
        node: String::new(),
        key: ImageKey("sha256:a".into()),
        image_ref: "nginx:1".into(),
        namespace: String::new(),
        pull_secrets: vec![],
    };
    let id = sdb::insert_scan(&x.ee.store, &t, Trigger::Arrival, now_unix())
        .await
        .unwrap();
    let f = Finding {
        vuln_id: "CVE-1".into(),
        pkg_name: "p".into(),
        installed_version: "1".into(),
        fixed_version: Some("2".into()),
        severity: Severity::High,
        title: None,
        url: None,
    };
    sdb::finish_ok(&x.ee.store, id, "trivy", &[f], now_unix())
        .await
        .unwrap();
    e
}

async fn pending_count(x: &Fx) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM ee_notify_pending")
        .fetch_one(x.ee.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn creating_a_rule_baselines() {
    let x = fx(true).await;
    seed_known_cve(&x).await;
    let d = x.slack().await;
    let body = json!({"name": "prod", "enabled": true, "envs": ["prod"], "min_severity": "high", "fixable_only": true, "running_only": true, "destinations": [d]});
    let (st, v, t) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/rules",
            Some(body),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    assert_eq!(v["envs"], json!(["prod"]));
    assert_eq!(pending_count(&x).await, 0);
}

#[tokio::test]
async fn widening_a_rule_baselines_the_new_scope() {
    let x = fx(true).await;
    seed_known_cve(&x).await;
    let d = x.slack().await;
    let body = json!({"name": "prod", "enabled": true, "envs": ["prod"], "min_severity": "critical", "fixable_only": true, "running_only": true, "destinations": [d]});
    let (_, v, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/rules",
            Some(body.clone()),
            &x.admin,
        )
        .await;
    let id = v["id"].as_i64().unwrap();
    let mut wider = body;
    wider["min_severity"] = json!("high");
    let (st, _, t) = x
        .send(
            "PUT",
            &format!("/api/v1/ee/notifications/rules/{id}"),
            Some(wider),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    assert_eq!(
        pending_count(&x).await,
        0,
        "the high CVE already there is not news"
    );
}

#[tokio::test]
async fn test_endpoint_returns_the_error() {
    let x = fx(true).await;
    let id = x.slack().await;
    let (st, _, _) = x
        .send(
            "POST",
            &format!("/api/v1/ee/notifications/destinations/{id}/test"),
            None,
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    *x.fake.fail.lock().unwrap() = true;
    let (st, v, t) = x
        .send(
            "POST",
            &format!("/api/v1/ee/notifications/destinations/{id}/test"),
            None,
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_GATEWAY, "{t}");
    assert!(v["message"].as_str().unwrap().contains("404"), "{v}");
}

#[tokio::test]
async fn deliveries_listed_newest_first() {
    let x = fx(true).await;
    let d = x.slack().await;
    use fleetops_ee::notify::{db as ndb, model as nm};
    for c in ["CVE-1", "CVE-2"] {
        let m = nm::Message::NewCve {
            rule: nm::RuleRef {
                id: 1,
                name: "r".into(),
            },
            cve: c.into(),
            severity: Severity::Critical,
            fixed_version: None,
            title: None,
            packages: vec![],
            images: vec![],
            more: 0,
            fixable_only: false,
        };
        ndb::enqueue_delivery(&x.ee.store, d, None, &m, now_unix())
            .await
            .unwrap();
    }
    let (st, v, _) = x
        .send("GET", "/api/v1/ee/notifications/deliveries", None, &x.admin)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(v[0]["title"].as_str().unwrap().starts_with("CVE-2"));
    assert_eq!(v[0]["destination"], "on-call");
    assert_eq!(v[0]["status"], "queued");
}

#[tokio::test]
async fn audit_entries_have_no_secrets() {
    let x = fx(true).await;
    x.slack().await;
    let entries = x.ee.store.list_audit(50, None).await.unwrap();
    let e = entries
        .iter()
        .find(|e| e.action == "notification.destination.create")
        .expect("audited");
    assert!(!format!("{e:?}").contains("SECRETpart"));
}

#[tokio::test]
async fn smtp_password_is_write_only() {
    let x = fx(true).await;
    let body = json!({"host": "email-smtp.eu-west-1.amazonaws.com", "port": 587, "security": "starttls", "username": "AKIA", "password": "pw-secret", "from_address": "fleetops@example.com"});
    let (st, v, t) = x
        .send("PUT", "/api/v1/ee/notifications/smtp", Some(body), &x.admin)
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    assert!(!t.contains("pw-secret"));
    assert_eq!(v["password_set"], true);
}

#[tokio::test]
async fn deleting_a_rules_only_destination_is_refused() {
    let x = fx(true).await;
    let d = x.slack().await;
    let body = json!({"name": "prod", "enabled": true, "envs": [], "min_severity": "critical", "fixable_only": true, "running_only": true, "destinations": [d]});
    let (st, _, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/rules",
            Some(body),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v, _) = x
        .send(
            "DELETE",
            &format!("/api/v1/ee/notifications/destinations/{d}"),
            None,
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    assert!(v["message"].as_str().unwrap().contains("prod"));
}

#[tokio::test]
async fn re_enabling_a_rule_baselines() {
    let x = fx(true).await;
    let e = seed_known_cve(&x).await;
    let d = x.slack().await;
    let body = json!({"name": "prod", "enabled": true, "envs": ["prod"], "min_severity": "high", "fixable_only": true, "running_only": true, "destinations": [d]});
    let (_, v, _) = x
        .send(
            "POST",
            "/api/v1/ee/notifications/rules",
            Some(body.clone()),
            &x.admin,
        )
        .await;
    let id = v["id"].as_i64().unwrap();
    let mut off = body.clone();
    off["enabled"] = json!(false);
    x.send(
        "PUT",
        &format!("/api/v1/ee/notifications/rules/{id}"),
        Some(off),
        &x.admin,
    )
    .await;
    // While it is off, a new CVE is found on the known image.
    let t = ScanTarget {
        env_id: e,
        node: String::new(),
        key: ImageKey("sha256:a".into()),
        image_ref: "nginx:1".into(),
        namespace: String::new(),
        pull_secrets: vec![],
    };
    let scan = sdb::insert_scan(&x.ee.store, &t, Trigger::Periodic, now_unix())
        .await
        .unwrap();
    let f = |id: &str| Finding {
        vuln_id: id.into(),
        pkg_name: "p".into(),
        installed_version: "1".into(),
        fixed_version: Some("2".into()),
        severity: Severity::High,
        title: None,
        url: None,
    };
    sdb::finish_ok(
        &x.ee.store,
        scan,
        "trivy",
        &[f("CVE-1"), f("CVE-OFF")],
        now_unix(),
    )
    .await
    .unwrap();
    let (st, _, t) = x
        .send(
            "PUT",
            &format!("/api/v1/ee/notifications/rules/{id}"),
            Some(body),
            &x.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    use fleetops_ee::scan::engine::ScanEvents;
    fleetops_ee::notify::matcher::Matcher
        .scan_finished(
            &x.ee.state,
            &ImageKey("sha256:a".into()),
            &[f("CVE-1"), f("CVE-OFF")],
        )
        .await;
    assert_eq!(
        pending_count(&x).await,
        0,
        "what happened while it was off is not news"
    );
}
