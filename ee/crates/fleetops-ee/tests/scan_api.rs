mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use fleetops_core::access::EnvRole;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Result};
use fleetops_ee::scan::{
    config::{Effective, GlobalConfig},
    db::{self, PasswordChange},
    inventory::Inventory,
    model::{Finding, ImageKey, ImageUse, ScanTarget, Severity, Trigger},
    runner::Runner,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
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

struct Fx {
    ee: Ee,
    admin: Session,
}

async fn fx(licensed: bool) -> Fx {
    let k = key();
    let ee = ee_scan_app(
        &k,
        StubEnvs::default(),
        (Arc::new(NoRunner), Arc::new(NoInventory)),
    )
    .await;
    let admin = ee.admin().await;
    if licensed {
        let r = ee
            .install(&admin, &license(&k, now_unix() + 86_400, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    Fx { ee, admin }
}

fn use_of(env_id: i64, key: &str, image: &str, ns: &str, running: bool) -> ImageUse {
    ImageUse {
        env_id,
        node: String::new(),
        key: ImageKey(key.into()),
        image_ref: image.into(),
        image_id: Some(format!("sha256:cfg-{key}")),
        namespace: ns.into(),
        workload: if ns.is_empty() {
            String::new()
        } else {
            "Deployment/web".into()
        },
        container: "c".into(),
        running,
        pull_secrets: vec![],
    }
}

fn f(id: &str, sev: Severity, fixed: bool) -> Finding {
    Finding {
        vuln_id: id.into(),
        pkg_name: "libssl3".into(),
        installed_version: "3.0.11".into(),
        fixed_version: fixed.then(|| "3.0.14".into()),
        severity: sev,
        title: None,
        url: None,
    }
}

impl Fx {
    async fn get(&self, uri: &str, s: &Session) -> (StatusCode, Value) {
        let r = self.ee.send("GET", uri, None, Some(s)).await;
        (r.status, r.json())
    }

    async fn send(&self, method: &str, uri: &str, body: Value, s: &Session) -> (StatusCode, Value) {
        let r = self
            .ee
            .send(
                method,
                uri,
                Some(("application/json", body.to_string())),
                Some(s),
            )
            .await;
        (r.status, r.json())
    }

    async fn done(&self, env_id: i64, key: &str, image: &str, findings: &[Finding]) {
        let t = ScanTarget {
            env_id,
            node: String::new(),
            key: ImageKey(key.into()),
            image_ref: image.into(),
            namespace: String::new(),
            pull_secrets: vec![],
        };
        let id = db::insert_scan(&self.ee.store, &t, Trigger::Arrival, now_unix())
            .await
            .unwrap();
        db::finish_ok(&self.ee.store, id, "trivy:1", findings, now_unix())
            .await
            .unwrap();
    }

    async fn failed(&self, env_id: i64, key: &str) {
        let t = ScanTarget {
            env_id,
            node: String::new(),
            key: ImageKey(key.into()),
            image_ref: "x".into(),
            namespace: String::new(),
            pull_secrets: vec![],
        };
        let id = db::insert_scan(&self.ee.store, &t, Trigger::Arrival, now_unix())
            .await
            .unwrap();
        db::finish_failed(&self.ee.store, id, "trivy exited 1: boom", now_unix())
            .await
            .unwrap();
    }

    /// A user with `role` on `env_id`, limited to `ns` when given.
    async fn member(&self, name: &str, env_id: i64, role: EnvRole, ns: Option<&[&str]>) -> Session {
        let r = self.ee.user(&self.admin, name).await;
        let uid = r.json()["id"].as_i64().unwrap();
        let ns: Option<BTreeSet<String>> = ns.map(|n| n.iter().map(|s| s.to_string()).collect());
        self.ee
            .store
            .set_binding(uid, env_id, role, ns.as_ref(), 0)
            .await
            .unwrap();
        self.ee.login(name).await
    }

    async fn enable(&self) {
        let g = GlobalConfig {
            enabled: true,
            mirror: None,
        };
        db::put_global(&self.ee.store, &g, PasswordChange::Keep, 0)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn unlicensed_is_feature_error() {
    let f = fx(false).await;
    for uri in ["/api/v1/ee/security/images", "/api/v1/ee/security/settings"] {
        let (st, body) = f.get(uri, &f.admin).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}: {body}");
    }
}

#[tokio::test]
async fn summary_defaults_to_running_and_fixable() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[
            use_of(e, "sha256:a", "nginx:1", "", true),
            use_of(e, "sha256:b", "redis:7", "", false),
            use_of(e, "sha256:c", "clean:1", "", true),
            use_of(e, "sha256:d", "more:1", "", true),
        ],
    )
    .await
    .unwrap();
    fx.done(
        e,
        "sha256:a",
        "nginx:1",
        &[f("CVE-1", Severity::Critical, true)],
    )
    .await;
    fx.done(
        e,
        "sha256:b",
        "redis:7",
        &[f("CVE-2", Severity::Critical, true)],
    )
    .await;
    fx.done(
        e,
        "sha256:c",
        "clean:1",
        &[f("CVE-3", Severity::Critical, false)],
    )
    .await;
    fx.done(
        e,
        "sha256:d",
        "more:1",
        &[
            f("CVE-4", Severity::Critical, true),
            f("CVE-5", Severity::Critical, true),
        ],
    )
    .await;
    let (st, body) = fx.get("/api/v1/ee/security/images", &fx.admin).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let images: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["image"].as_str().unwrap())
        .collect();
    assert_eq!(
        images,
        ["more:1", "nginx:1"],
        "running, fixable, most criticals first"
    );
    assert_eq!(body["critical_fixable_running"], 2);
    let (_, all) = fx
        .get(
            "/api/v1/ee/security/images?running=false&fixable=false",
            &fx.admin,
        )
        .await;
    assert_eq!(all["rows"].as_array().unwrap().len(), 4);
    assert_eq!(all["rows"][3]["image"], "redis:7", "not running goes last");
}

#[tokio::test]
async fn summary_counts_failed_and_unscanned() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[
            use_of(e, "sha256:ok", "ok:1", "", true),
            use_of(e, "sha256:bad", "bad:1", "", true),
            use_of(e, "sha256:new", "new:1", "", true),
        ],
    )
    .await
    .unwrap();
    fx.done(e, "sha256:ok", "ok:1", &[]).await;
    fx.failed(e, "sha256:bad").await;
    let (_, body) = fx.get("/api/v1/ee/security/images", &fx.admin).await;
    assert_eq!(
        (
            body["scanned"].as_u64(),
            body["failed"].as_u64(),
            body["unscanned"].as_u64()
        ),
        (Some(1), Some(1), Some(1))
    );
    let rows = body["rows"].as_array().unwrap();
    let bad = rows
        .iter()
        .find(|r| r["image"] == "bad:1")
        .expect("a failed row is never filtered out as clean");
    assert_eq!(bad["state"], "failed");
    assert_eq!(bad["has_result"], false);
    assert!(bad["error"].as_str().unwrap().contains("boom"));
    let new = rows.iter().find(|r| r["image"] == "new:1").unwrap();
    assert_eq!(new["state"], "never_scanned");
    assert!(
        rows.iter().all(|r| r["image"] != "ok:1"),
        "clean and fix-free is filtered by default"
    );
}

#[tokio::test]
async fn ns_scoped_user_sees_only_own_uses() {
    let fx = fx(true).await;
    let e = fx.ee.env("prod", "kube-config").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[
            use_of(e, "sha256:a", "nginx:1", "team-a", true),
            use_of(e, "sha256:a", "nginx:1", "team-b", true),
        ],
    )
    .await
    .unwrap();
    fx.done(
        e,
        "sha256:a",
        "nginx:1",
        &[f("CVE-1", Severity::Critical, true)],
    )
    .await;
    let bob = fx
        .member("bob", e, EnvRole::Viewer, Some(&["team-a"]))
        .await;
    let (_, body) = fx.get("/api/v1/ee/security/images", &bob).await;
    assert_eq!(body["rows"][0]["running"], 1, "only team-a's pod");
    assert_eq!(body["rows"][0]["critical_fixable"], 1);
    let (st, detail) = fx
        .get("/api/v1/ee/security/image?image=nginx:1", &bob)
        .await;
    assert_eq!(st, StatusCode::OK);
    let used: Vec<&str> = detail["digests"][0]["used_by"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["namespace"].as_str().unwrap())
        .collect();
    assert_eq!(used, ["team-a"]);
    assert_eq!(detail["digests"][0]["findings"][0]["vuln_id"], "CVE-1");
}

#[tokio::test]
async fn image_outside_scope_is_not_found() {
    let fx = fx(true).await;
    let e = fx.ee.env("prod", "kube-config").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[use_of(e, "sha256:b", "secret:1", "team-b", true)],
    )
    .await
    .unwrap();
    let bob = fx
        .member("bob", e, EnvRole::Viewer, Some(&["team-a"]))
        .await;
    let (st, _) = fx
        .get("/api/v1/ee/security/image?image=secret:1", &bob)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, body) = fx
        .get(
            "/api/v1/ee/security/images?running=false&fixable=false",
            &bob,
        )
        .await;
    assert!(body["rows"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn viewer_cannot_scan_operator_can() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    fx.enable().await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[use_of(e, "sha256:a", "nginx:1", "", true)],
    )
    .await
    .unwrap();
    let viewer = fx.member("vee", e, EnvRole::Viewer, None).await;
    let body = json!({"env": "local", "images": ["nginx:1"]});
    let (st, _) = fx
        .send("POST", "/api/v1/ee/security/scan", body.clone(), &viewer)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let op = fx.member("opp", e, EnvRole::Operator, None).await;
    let (st, r) = fx.send("POST", "/api/v1/ee/security/scan", body, &op).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["queued"], 1);
    assert_eq!(
        db::active_scan(&fx.ee.store, &ImageKey("sha256:a".into()))
            .await
            .unwrap()
            .is_some(),
        true
    );
    assert!(fx.ee.actions().await.contains(&"security.scan".to_string()));
}

#[tokio::test]
async fn kube_scan_needs_workloads_operate_in_a_using_namespace() {
    let fx = fx(true).await;
    let e = fx.ee.env("prod", "kube-config").await;
    fx.enable().await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[use_of(e, "sha256:a", "nginx:1", "team-a", true)],
    )
    .await
    .unwrap();
    let elsewhere = fx
        .member("ella", e, EnvRole::Operator, Some(&["team-b"]))
        .await;
    let body = json!({"env": "prod", "images": ["nginx:1"]});
    let (st, _) = fx
        .send("POST", "/api/v1/ee/security/scan", body.clone(), &elsewhere)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "the image is invisible to her");
    let here = fx
        .member("hank", e, EnvRole::Operator, Some(&["team-a"]))
        .await;
    let (st, r) = fx
        .send("POST", "/api/v1/ee/security/scan", body.clone(), &here)
        .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let viewer = fx
        .member("vic", e, EnvRole::Viewer, Some(&["team-a"]))
        .await;
    let (st, _) = fx
        .send("POST", "/api/v1/ee/security/scan", body, &viewer)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn scan_now_refuses_when_scanning_is_off() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[use_of(e, "sha256:a", "nginx:1", "", true)],
    )
    .await
    .unwrap();
    let (st, body) = fx
        .send(
            "POST",
            "/api/v1/ee/security/scan",
            json!({"env": "local"}),
            &fx.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn settings_admin_only() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    let op = fx.member("opp", e, EnvRole::Operator, None).await;
    let (st, _) = fx.get("/api/v1/ee/security/settings", &op).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/settings",
            json!({"enabled": true}),
            &op,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = fx.get("/api/v1/ee/security/settings", &fx.admin).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["global"]["enabled"], false, "off by default");
    assert_eq!(body["envs"][0]["status"]["state"], "off");
    assert!(body["global"]["default_trivy_image"]
        .as_str()
        .unwrap()
        .contains("@sha256:"));
}

#[tokio::test]
async fn password_never_returned_and_clear_works() {
    let fx = fx(true).await;
    let mirror = json!({"trivy_image": "mirror.local/trivy:0.76.1", "username": "bot", "password": "hunter2"});
    let (st, body) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/settings",
            json!({"enabled": false, "mirror": mirror}),
            &fx.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("hunter2"));
    assert_eq!(body["global"]["mirror"]["password_set"], true);
    let keep = json!({"trivy_image": "mirror.local/trivy:0.76.1", "username": "bot"});
    let (_, body) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/settings",
            json!({"enabled": false, "mirror": keep}),
            &fx.admin,
        )
        .await;
    assert_eq!(
        body["global"]["mirror"]["password_set"], true,
        "omitted keeps it"
    );
    let clear = json!({"trivy_image": "mirror.local/trivy:0.76.1", "username": "bot", "clear_password": true});
    let (_, body) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/settings",
            json!({"enabled": false, "mirror": clear}),
            &fx.admin,
        )
        .await;
    assert_eq!(body["global"]["mirror"]["password_set"], false);
}

#[tokio::test]
async fn env_setting_change_is_audited_and_notifies_engine() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    let (st, body) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/envs/local/settings",
            json!({"enabled": true}),
            &fx.admin,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let env = body["envs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["env"] == "local")
        .unwrap();
    assert_eq!(env["enabled"], true);
    assert_eq!(env["effective_enabled"], true);
    assert_ne!(env["status"]["state"], "off", "the engine heard about it");
    assert!(fx
        .ee
        .actions()
        .await
        .contains(&"security.env.update".to_string()));
    let _ = e;
}

#[tokio::test]
async fn bad_socket_path_rejected() {
    let fx = fx(true).await;
    fx.ee.env("local", "docker-local").await;
    let (st, _) = fx
        .send(
            "PUT",
            "/api/v1/ee/security/envs/local/settings",
            json!({"socket_path": "docker.sock"}),
            &fx.admin,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn badges_mark_fixable_severity_per_image() {
    let fx = fx(true).await;
    let e = fx.ee.env("local", "docker-local").await;
    db::replace_uses(
        &fx.ee.store,
        e,
        &[
            use_of(e, "sha256:a", "nginx:1", "", true),
            use_of(e, "sha256:b", "new:1", "", false),
        ],
    )
    .await
    .unwrap();
    fx.done(
        e,
        "sha256:a",
        "nginx:1",
        &[f("CVE-1", Severity::High, true)],
    )
    .await;
    let (st, body) = fx
        .get("/api/v1/ee/security/envs/local/badges", &fx.admin)
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!([
            {"image_id": "sha256:cfg-sha256:a", "severity": "high", "fixable": 1, "state": "scanned"},
            {"image_id": "sha256:cfg-sha256:b", "severity": null, "fixable": 0, "state": "never_scanned"},
        ])
    );
}
