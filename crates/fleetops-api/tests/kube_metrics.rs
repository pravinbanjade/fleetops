mod common;

use common::*;
use fleetops_core::kube::{KubeKind, KubeUsage, KubeUsageRow};

fn usage(ns: Option<&str>, name: &str, cpu: u64) -> KubeUsageRow {
    KubeUsageRow {
        namespace: ns.map(str::to_string),
        name: name.into(),
        cpu,
        memory: 1 << 20,
        cpu_allocatable: None,
        memory_allocatable: None,
    }
}

async fn kube_app() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "team-a", "web-1");
    (t, admin)
}

#[tokio::test]
async fn a_cluster_without_metrics_server_says_so_instead_of_failing() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/metrics/nodes",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["available"], false);
    assert!(r.json["reason"]
        .as_str()
        .unwrap()
        .contains("metrics-server"));
}

#[tokio::test]
async fn usage_is_only_for_nodes_and_pods() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/metrics/secrets",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400);
    assert!(t.fake.kube.calls().iter().all(|c| !c.starts_with("usage")));
}

#[tokio::test]
async fn pod_usage_passes_the_namespace_and_hides_namespaces_outside_the_grant() {
    let (t, admin) = kube_app().await;
    *t.fake.kube.usage.lock().unwrap() = Some(KubeUsage {
        available: true,
        reason: None,
        rows: vec![
            usage(Some("team-a"), "web-1", 120),
            usage(Some("team-b"), "db-0", 900),
        ],
    });
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/metrics/pods?namespace=team-a",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert!(t
        .fake
        .kube
        .calls()
        .contains(&"usage pods team-a".to_string()));

    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["team-a"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let all = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/metrics/pods",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(all.status, 200, "{:?}", all.json);
    let rows = all.json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "web-1");
    assert_eq!(rows[0]["cpu"], 120);
    let other = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/metrics/pods?namespace=team-b",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(other.status, 403);
}

#[tokio::test]
async fn events_are_read_only() {
    let (t, admin) = kube_app().await;
    t.fake.kube.object(KubeKind::Events, "team-a", "web-1.17f");
    let r = t
        .send(req("GET", "/api/v1/envs/k/k8s/events", None, Some(&admin)))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json["rows"][0]["name"], "web-1.17f");
    let d = t
        .send(req(
            "DELETE",
            "/api/v1/envs/k/k8s/events/team-a/web-1.17f",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(d.status, 400, "{:?}", d.json);
    assert!(d.json["message"].as_str().unwrap().contains("read-only"));
    assert!(t.fake.kube.calls().iter().all(|c| !c.starts_with("delete")));
}

#[tokio::test]
async fn counts_answer_per_kind_and_a_refusal_is_per_kind_too() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.object(KubeKind::Services, "team-a", "web");
    k.object(KubeKind::Services, "team-b", "db");
    k.object(KubeKind::ConfigMaps, "team-a", "cfg");
    k.set_forbidden(KubeKind::Secrets);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=services,configmaps,secrets,services",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["services"]["count"], 2);
    assert_eq!(r.json["configmaps"]["count"], 1);
    assert_eq!(r.json["secrets"]["count"], serde_json::Value::Null);
    assert_eq!(r.json["secrets"]["error"], "forbidden");
    let one = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=services&namespace=team-b",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(one.json["services"]["count"], 1);
    let bad = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=widgets",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(bad.status, 404);
}

#[tokio::test]
async fn a_namespace_limited_count_sums_only_its_namespaces() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.object(KubeKind::Services, "team-a", "web");
    k.object(KubeKind::Services, "team-b", "db");
    k.object(KubeKind::Namespaces, "-", "team-a");
    k.object(KubeKind::Namespaces, "-", "team-b");
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["team-a"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=services,namespaces",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["services"]["count"], 1);
    assert_eq!(r.json["namespaces"]["count"], 1);
    let other = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=services&namespace=team-b",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(other.status, 403);
}

#[tokio::test]
async fn the_warnings_filter_keeps_warning_events_and_is_for_events_only() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.object(KubeKind::Events, "team-a", "a.1");
    k.object(KubeKind::Events, "team-a", "b.1");
    k.set_status(KubeKind::Events, "team-a", "a.1", "Warning");
    k.set_status(KubeKind::Events, "team-a", "b.1", "Normal");
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/events?filter=warnings",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let rows = r.json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "a.1");
    let pods = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/pods?filter=warnings",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(pods.status, 400);
}
