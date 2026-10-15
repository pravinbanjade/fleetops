mod common;

use common::*;
use fleetops_core::helm::*;
use fleetops_core::kube::{ApplyError, MASK};
use serde_json::json;

fn plan(current: u32) -> HelmPlan {
    HelmPlan {
        current,
        revision: None,
        status: None,
        changes: vec![HelmObjectChange {
            api_version: "v1".into(),
            kind: "ConfigMap".into(),
            namespace: Some("apps".into()),
            name: "c".into(),
            change: HelmChange::Patch,
            before: Some("a".into()),
            after: Some("b".into()),
            error: None,
        }],
        note: HOOKS_NOTE.into(),
        ..Default::default()
    }
}

async fn helm_app() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let k = &t.fake.kube;
    k.helm_release("apps", "demo", 2, "deployed");
    k.helm_release("other", "api", 1, "failed");
    *k.helm_plan.lock().unwrap() = Some(plan(2));
    *k.helm_objects.lock().unwrap() = vec![(true, Some("apps".into()))];
    (t, admin)
}

const DEMO: &str = "/api/v1/envs/k/k8s/helm/releases/apps/demo";

#[tokio::test]
async fn releases_list_in_one_namespace_or_all() {
    let (t, admin) = helm_app().await;
    let all = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(all.status, 200, "{:?}", all.json);
    assert_eq!(all.json.as_array().unwrap().len(), 2);
    let one = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases?namespace=apps",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(one.json[0]["name"], "demo");
    assert_eq!(one.json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_unreadable_release_is_a_row_not_an_error() {
    let (t, admin) = helm_app().await;
    t.fake.kube.helm_release("apps", "broken", 3, UNREADABLE);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200);
    let broken = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "broken")
        .unwrap()
        .clone();
    assert_eq!(broken["status"], UNREADABLE);
    assert_eq!(broken["error"], "not gzip");
}

#[tokio::test]
async fn detail_and_history_are_served_masked() {
    let (t, admin) = helm_app().await;
    let d = t.send(req("GET", DEMO, None, Some(&admin))).await;
    assert_eq!(d.status, 200, "{:?}", d.json);
    assert!(d.json["values"].as_str().unwrap().contains(MASK));
    let h = t
        .send(req("GET", &format!("{DEMO}/history"), None, Some(&admin)))
        .await;
    assert_eq!(h.json.as_array().unwrap().len(), 2);
    let missing = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases/apps/nope",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(missing.status, 404);
    let bad = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases/Apps!/demo",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn a_rollback_dry_run_is_not_audited_and_its_commit_is() {
    let (t, admin) = helm_app().await;
    let body = |dry: bool| json!({"revision": 1, "expected_current": 2, "dry_run": dry});
    let url = format!("{DEMO}/rollback");
    let dry = t
        .send(req("POST", &url, Some(body(true)), Some(&admin)))
        .await;
    assert_eq!(dry.status, 200, "{:?}", dry.json);
    assert_eq!(dry.json["current"], 2);
    assert_eq!(dry.json["note"], HOOKS_NOTE);
    assert!(!t
        .audit_actions()
        .await
        .iter()
        .any(|a| a.starts_with("helm.")));
    let done = t
        .send(req("POST", &url, Some(body(false)), Some(&admin)))
        .await;
    assert_eq!(done.status, 200, "{:?}", done.json);
    assert_eq!(
        (done.json["revision"].as_u64(), done.json["status"].as_str()),
        (Some(3), Some("deployed"))
    );
    assert!(t
        .audit_actions()
        .await
        .contains(&"helm.rollback".to_string()));
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await
        .json
        .to_string();
    assert!(
        audit.contains("apps/demo") && !audit.contains(MASK),
        "{audit}"
    );
}

#[tokio::test]
async fn a_stale_commit_is_409() {
    let (t, admin) = helm_app().await;
    let r = t
        .send(req(
            "POST",
            &format!("{DEMO}/uninstall"),
            Some(json!({"expected_current": 1, "dry_run": false})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409);
    assert!(
        r.json["message"].as_str().unwrap().contains(CHANGED),
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn an_uninstall_commit_is_audited() {
    let (t, admin) = helm_app().await;
    let r = t
        .send(req(
            "POST",
            &format!("{DEMO}/uninstall"),
            Some(json!({"expected_current": 2, "dry_run": false})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["status"], "uninstalled");
    assert!(t
        .audit_actions()
        .await
        .contains(&"helm.uninstall".to_string()));
}

#[tokio::test]
async fn viewers_read_but_cannot_roll_back() {
    let (t, admin) = helm_app().await;
    t.create_user(&admin, "vera", "vera-password-1", "user")
        .await;
    t.grant("vera", "k", "viewer", None).await;
    let vera = session_from(&t.login("vera", "vera-password-1").await);
    let list = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases",
            None,
            Some(&vera),
        ))
        .await;
    assert_eq!(list.status, 200);
    let r = t
        .send(req(
            "POST",
            &format!("{DEMO}/rollback"),
            Some(json!({"revision": 1, "expected_current": 2, "dry_run": true})),
            Some(&vera),
        ))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn a_namespace_limited_grant_filters_and_refuses_the_whole_action() {
    let (t, admin) = helm_app().await;
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["apps"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let all = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(all.json.as_array().unwrap().len(), 1, "{:?}", all.json);
    let other = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases/other/api",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(other.status, 403);
    // The chart also holds a cluster-scoped object: refused before any dry run.
    t.fake.kube.helm_objects.lock().unwrap().push((false, None));
    let r = t
        .send(req(
            "POST",
            &format!("{DEMO}/rollback"),
            Some(json!({"revision": 1, "expected_current": 2, "dry_run": true})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    assert!(t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn a_docker_environment_is_409() {
    let (t, admin) = helm_app().await;
    t.add_env(&admin, "local").await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/k8s/helm/releases",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
}

#[tokio::test]
async fn a_cluster_refusal_passes_through_per_object() {
    let (t, admin) = helm_app().await;
    let mut p = plan(2);
    p.changes[0].error = Some(ApplyError {
        code: "forbidden".into(),
        message: "configmaps is forbidden".into(),
    });
    *t.fake.kube.helm_plan.lock().unwrap() = Some(p);
    let r = t
        .send(req(
            "POST",
            &format!("{DEMO}/rollback"),
            Some(json!({"revision": 1, "expected_current": 2, "dry_run": true})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["changes"][0]["error"]["code"], "forbidden");
}

#[tokio::test]
async fn a_commit_finishes_even_when_the_caller_leaves() {
    let (t, admin) = helm_app().await;
    *t.fake.kube.helm_commit_delay.lock().unwrap() = Some(std::time::Duration::from_millis(300));
    let r = req(
        "POST",
        &format!("{DEMO}/rollback"),
        Some(json!({"revision": 1, "expected_current": 2, "dry_run": false})),
        Some(&admin),
    );
    // The client gives up long before the commit is done.
    let gone = tokio::time::timeout(std::time::Duration::from_millis(50), t.send(r)).await;
    assert!(gone.is_err(), "the request should still have been running");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        t.fake
            .kube
            .calls()
            .iter()
            .any(|c| c.contains("finished dry=false")),
        "{:?}",
        t.fake.kube.calls()
    );
    assert!(t
        .audit_actions()
        .await
        .contains(&"helm.rollback".to_string()));
}

fn with_helm() -> fleetops_api::ApiConfig {
    fleetops_api::ApiConfig {
        helm: Some(std::sync::Arc::new(common::FakeRenderer::default())),
        ..Default::default()
    }
}

fn install_body(dry_run: bool) -> serde_json::Value {
    json!({"name": "demo", "chart": {"repo_url": "https://c.example", "chart": "demo"}, "values": "password: s3cret\n", "dry_run": dry_run, "expected_current": 0})
}

#[tokio::test]
async fn without_the_flag_install_is_off_and_capabilities_say_so() {
    let t = common::app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let caps = t
        .send(common::req(
            "GET",
            "/api/v1/envs/k/k8s/helm/capabilities",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(caps.json, json!({"install": false}));
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps",
            Some(install_body(true)),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400);
    assert!(
        r.json["message"]
            .as_str()
            .unwrap()
            .contains("--helm-binary"),
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn install_plans_then_commits_and_audits_without_values() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let plan = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps",
            Some(install_body(true)),
            Some(&admin),
        ))
        .await;
    assert_eq!(plan.status, 200, "{:?}", plan.json);
    assert_eq!(plan.json["notes"], "Hello");
    let done = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps",
            Some(install_body(false)),
            Some(&admin),
        ))
        .await;
    assert_eq!(done.json["status"], "deployed");
    let deploys = t.fake.kube.deploys.lock().unwrap().clone();
    assert_eq!(deploys.len(), 2);
    assert_eq!(deploys[1].0.config, json!({"password": "s3cret"}));
    assert_eq!(
        deploys[1].1,
        fleetops_core::helm::HelmMode::Commit {
            expected_current: 0
        }
    );
    assert_eq!(deploys[1].0.source.chart.as_deref(), Some("demo"));
    let audit = t
        .send(common::req("GET", "/api/v1/audit", None, Some(&admin)))
        .await
        .json
        .to_string();
    assert!(
        audit.contains("helm.install")
            && audit.contains("https://c.example demo")
            && !audit.contains("s3cret"),
        "{audit}"
    );
}

#[tokio::test]
async fn bad_values_and_bad_references_are_refused_before_rendering() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    for body in [
        json!({"name": "demo", "chart": {"repo_url": "https://c.example", "chart": "demo"}, "values": "- a\n", "dry_run": true}),
        json!({"name": "demo", "chart": {"repo_url": "https://c.example", "chart": "demo"}, "values": "password: ••••\n", "dry_run": true}),
        json!({"name": "demo", "chart": {"oci": "--post-renderer=/x"}, "values": "", "dry_run": true}),
        json!({"name": "demo", "chart": {"repo_url": "http://c.example", "chart": "demo"}, "values": "", "dry_run": true}),
        json!({"name": "Not_A_Name", "chart": {"oci": "oci://r/c"}, "values": "", "dry_run": true}),
    ] {
        let r = t
            .send(common::req(
                "POST",
                "/api/v1/envs/k/k8s/helm/releases/apps",
                Some(body.clone()),
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, 400, "{body}: {:?}", r.json);
    }
    assert!(t.fake.kube.deploys.lock().unwrap().is_empty());
}

#[tokio::test]
async fn upgrade_restores_masked_values_from_the_release() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.helm_release("apps", "demo", 1, "deployed");
    *t.fake.kube.helm_values.lock().unwrap() = json!({"db": {"password": "s3cret"}, "replicas": 1});
    let body = json!({"chart": {"oci": "oci://r/demo", "version": "0.2.0"}, "values": "db:\n  password: ••••\nreplicas: 2\n", "dry_run": true});
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps/demo/upgrade",
            Some(body),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let d = &t.fake.kube.deploys.lock().unwrap()[0].0;
    assert!(d.upgrade);
    assert_eq!(
        d.config,
        json!({"db": {"password": "s3cret"}, "replicas": 2})
    );
}

#[tokio::test]
async fn a_name_in_use_is_a_conflict() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.helm_release("apps", "demo", 1, "deployed");
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps",
            Some(install_body(true)),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("exists; upgrade it"));
}

#[tokio::test]
async fn a_namespace_outside_the_grant_is_refused() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["team-a"])).await;
    let bob = common::session_from(&t.login("bob", "bob-password-1").await);
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps",
            Some(install_body(true)),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn show_returns_the_charts_defaults() {
    let t = common::app_with(with_helm()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs/k/k8s/helm/charts/show",
            Some(json!({"chart": {"oci": "oci://r/c"}})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["values"], "replicas: 1\n");
}
