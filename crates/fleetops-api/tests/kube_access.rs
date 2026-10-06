mod common;

use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::json;

async fn limited() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let k = &t.fake.kube;
    k.object(KubeKind::Deployments, "team-a", "web");
    k.object(KubeKind::Deployments, "team-b", "api");
    k.object(KubeKind::Namespaces, "-", "team-a");
    k.object(KubeKind::Namespaces, "-", "team-b");
    k.object(KubeKind::Nodes, "-", "n1");
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["team-a"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    (t, bob)
}

fn names(r: &Resp) -> Vec<String> {
    r.json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn all_namespace_lists_are_filtered_and_other_namespaces_refused() {
    let (t, bob) = limited().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(names(&r), ["web"]);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments?namespace=team-b",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        r.json["message"],
        "namespace 'team-b' is not in your access"
    );
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/namespaces",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(names(&r), ["team-a"]);
    let r = t
        .send(req("GET", "/api/v1/envs/k/k8s/nodes", None, Some(&bob)))
        .await;
    assert_eq!(names(&r), ["n1"]);
}

#[tokio::test]
async fn objects_outside_the_set_are_refused_for_every_route() {
    let (t, bob) = limited().await;
    let base = "/api/v1/envs/k/k8s/deployments/team-b/api";
    for (m, p, body) in [
        ("GET", base.to_string(), None),
        ("GET", format!("{base}/events"), None),
        ("GET", format!("{base}/edit"), None),
        ("DELETE", base.to_string(), None),
        ("POST", format!("{base}/restart"), None),
        (
            "POST",
            format!("{base}/scale"),
            Some(json!({"replicas": 1})),
        ),
        (
            "PUT",
            base.to_string(),
            Some(json!({"yaml": "x", "resource_version": "1"})),
        ),
    ] {
        let r = t.send(req(m, &p, body, Some(&bob))).await;
        assert_eq!(r.status, 403, "{m} {p}: {:?}", r.json);
    }
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments/team-a/web",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/nodes/-/n1",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200);
    let r = t
        .send(req(
            "DELETE",
            "/api/v1/envs/k/k8s/namespaces/-/team-a",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(
        r.json["message"],
        "cluster-scoped objects are outside your access"
    );
    assert!(t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn apply_passes_the_set_and_refuses_a_default_namespace_outside_it() {
    let (t, bob) = limited().await;
    let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n  namespace: team-a\n---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: b\n  namespace: team-b\n";
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": yaml, "namespace": "team-a", "dry_run": true})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert!(r.json[0]["error"].is_null());
    assert_eq!(r.json[1]["error"]["code"], "forbidden");
    assert_eq!(
        r.json[1]["error"]["message"],
        "namespace 'team-b' is not in your access"
    );
    let limited_to = t.fake.kube.applied.lock().unwrap()[0].3.clone();
    assert_eq!(
        limited_to.unwrap().into_iter().collect::<Vec<_>>(),
        ["team-a"]
    );
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": yaml, "namespace": "team-b"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn a_whole_cluster_grant_passes_no_set() {
    let (t, _bob) = limited().await;
    let admin = session_from(&t.login(ADMIN, ADMIN_PW).await);
    t.send(req("POST", "/api/v1/envs/k/k8s/apply", Some(json!({"yaml": "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n", "namespace": "x"})), Some(&admin)))
        .await;
    assert!(t.fake.kube.applied.lock().unwrap()[0].3.is_none());
}
