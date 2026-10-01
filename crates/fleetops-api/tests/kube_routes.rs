mod common;

use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::json;

async fn kube_app() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let k = &t.fake.kube;
    k.object(KubeKind::Deployments, "apps", "web");
    k.object(KubeKind::Deployments, "other", "api");
    k.object(KubeKind::Pods, "apps", "web-1");
    k.object(KubeKind::Nodes, "-", "n1");
    (t, admin)
}

#[tokio::test]
async fn a_list_is_scoped_to_the_namespace_asked_for() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments?namespace=apps",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["rows"].as_array().unwrap().len(), 1);
    assert_eq!(r.json["rows"][0]["name"], "web");
    assert_eq!(r.json["truncated"], false);
    let all = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(all.json["rows"].as_array().unwrap().len(), 2);
    let nodes = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/nodes?namespace=apps",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(nodes.json["rows"][0]["name"], "n1");
}

#[tokio::test]
async fn an_unknown_kind_is_404() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/replicasets",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn kube_and_docker_routes_refuse_the_other_kind() {
    let (t, admin) = kube_app().await;
    t.add_env(&admin, "local").await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/k8s/pods",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(
        r.json["message"],
        "conflict: wrong environment kind: 'local' is docker-local"
    );
    let r = t
        .send(req("GET", "/api/v1/envs/k/containers", None, Some(&admin)))
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(
        r.json["message"],
        "conflict: wrong environment kind: 'k' is kube-config"
    );
}

#[tokio::test]
async fn get_and_events_return_the_object() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/pods/apps/web-1",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["row"]["name"], "web-1");
    assert_eq!(r.json["containers"], json!(["main"]));
    let e = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/pods/apps/web-1/events",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(e.json[0]["reason"], "Started");
    let n = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/nodes/-/n1",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(n.status, 200);
    let missing = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/pods/apps/nope",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(missing.status, 404);
}

#[tokio::test]
async fn scale_restart_and_delete_act_and_are_audited() {
    let (t, admin) = kube_app().await;
    let s = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/scale",
            Some(json!({"replicas": 3})),
            Some(&admin),
        ))
        .await;
    assert_eq!(s.status, 204, "{:?}", s.json);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/restart",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 204);
    let d = t
        .send(req(
            "DELETE",
            "/api/v1/envs/k/k8s/pods/apps/web-1",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(d.status, 204);
    let calls = t.fake.kube.calls();
    assert!(
        calls.contains(&"scale deployments/apps/web 3".into()),
        "{calls:?}"
    );
    assert!(calls.contains(&"restart deployments/apps/web".into()));
    assert!(calls.contains(&"delete pods/apps/web-1".into()));
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let text = audit.json.to_string();
    for (action, target) in [
        ("k8s.scale", "deployments/apps/web"),
        ("k8s.restart", "deployments/apps/web"),
        ("k8s.delete", "pods/apps/web-1"),
    ] {
        assert!(
            text.contains(action) && text.contains(target),
            "{action} {target}: {text}"
        );
    }
}

#[tokio::test]
async fn kinds_that_cannot_scale_or_restart_are_refused_before_the_cluster() {
    let (t, admin) = kube_app().await;
    let s = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/pods/apps/web-1/scale",
            Some(json!({"replicas": 2})),
            Some(&admin),
        ))
        .await;
    assert_eq!(s.status, 400);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/pods/apps/web-1/restart",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400);
    assert!(t
        .fake
        .kube
        .calls()
        .iter()
        .all(|c| !c.starts_with("scale") && !c.starts_with("restart")));
}

#[tokio::test]
async fn a_cluster_403_passes_through_with_its_reason() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_forbidden(KubeKind::Secrets);
    let r = t
        .send(req("GET", "/api/v1/envs/k/k8s/secrets", None, Some(&admin)))
        .await;
    assert_eq!(r.status, 403);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("cannot list resource"));
}

#[tokio::test]
async fn an_unreachable_cluster_is_503_and_truncation_is_reported() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_truncated();
    let r = t
        .send(req("GET", "/api/v1/envs/k/k8s/pods", None, Some(&admin)))
        .await;
    assert_eq!(r.json["truncated"], true);
    t.fake
        .kube
        .unreachable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = t
        .send(req("GET", "/api/v1/envs/k/k8s/pods", None, Some(&admin)))
        .await;
    assert_eq!(r.status, 503);
}

#[tokio::test]
async fn ordinary_users_can_read_and_act() {
    let (t, admin) = kube_app().await;
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let user = session_from(&t.login("bob", "bob-password-1").await);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments",
            None,
            Some(&user),
        ))
        .await;
    assert_eq!(r.status, 200);
    let s = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/restart",
            None,
            Some(&user),
        ))
        .await;
    assert_eq!(s.status, 204);
}
