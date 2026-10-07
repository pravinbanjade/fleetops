mod common;

use common::fake_kube::widgets;
use common::*;
use fleetops_core::kube::{ApiKind, Discovery, KindRef};
use serde_json::json;

const WIDGETS: &str = "/api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets";

fn definitions() -> ApiKind {
    ApiKind {
        api: KindRef {
            group: "apiextensions.k8s.io".into(),
            version: "v1".into(),
            resource: "customresourcedefinitions".into(),
        },
        kind: "CustomResourceDefinition".into(),
        namespaced: false,
        verbs: ["get", "list", "watch"].map(String::from).to_vec(),
        columns: vec![],
        crd: None,
    }
}

fn discovery_with_widgets() -> Discovery {
    Discovery {
        kinds: vec![definitions(), widgets(true)],
        failed_groups: vec!["metrics.k8s.io".into()],
    }
}

async fn kube_app() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    (t, admin)
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
async fn discovery_lists_generic_kinds_and_failed_groups() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_discovery(discovery_with_widgets());
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/discovery",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["kinds"].as_array().unwrap().len(), 2);
    assert_eq!(r.json["failed_groups"], json!(["metrics.k8s.io"]));
}

#[tokio::test]
async fn a_widget_lists_opens_edits_and_deletes() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.set_discovery(discovery_with_widgets());
    k.generic_object(&widgets(true), "apps", "w1");
    let list = t
        .send(req(
            "GET",
            &format!("{WIDGETS}?namespace=apps"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(list.status, 200, "{:?}", list.json);
    assert_eq!(list.json["rows"][0]["name"], "w1");
    assert_eq!(list.json["rows"][0]["columns"]["Size"], "3");
    let obj = t
        .send(req(
            "GET",
            &format!("{WIDGETS}/apps/w1"),
            None,
            Some(&admin),
        ))
        .await;
    assert!(obj.json["yaml"].as_str().unwrap().contains("kind: Widget"));
    let ed = t
        .send(req(
            "GET",
            &format!("{WIDGETS}/apps/w1/edit"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(ed.json["resource_version"], "1");
    let ev = t
        .send(req(
            "GET",
            &format!("{WIDGETS}/apps/w1/events"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(ev.status, 200, "{:?}", ev.json);
    let put = t
        .send(req(
            "PUT",
            &format!("{WIDGETS}/apps/w1"),
            Some(json!({"yaml": "kind: Widget", "resource_version": "1"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(put.status, 200, "{:?}", put.json);
    let del = t
        .send(req(
            "DELETE",
            &format!("{WIDGETS}/apps/w1"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(del.status, 204, "{:?}", del.json);
    assert!(k
        .calls()
        .iter()
        .any(|c| c == "gdelete example.fleetops.io/v1/widgets apps/w1"));
    let audit = t.store.list_audit(1000, None).await.unwrap();
    assert!(
        audit.iter().any(|e| e.action == "k8s.delete"
            && e.target_id.as_deref() == Some("example.fleetops.io/v1/widgets/apps/w1")),
        "{audit:?}"
    );
    assert!(audit.iter().any(|e| e.action == "k8s.edit"));
}

#[tokio::test]
async fn a_kind_installed_after_the_cache_filled_resolves_at_once() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.set_discovery(Discovery::default());
    // Fills the cache without widgets.
    t.send(req(
        "GET",
        "/api/v1/envs/k/k8s/discovery",
        None,
        Some(&admin),
    ))
    .await;
    k.set_discovery(discovery_with_widgets());
    k.generic_object(&widgets(true), "apps", "w1");
    let list = t.send(req("GET", WIDGETS, None, Some(&admin))).await;
    assert_eq!(list.status, 200, "{:?}", list.json);
    assert_eq!(names(&list), ["w1"]);
    assert!(k.calls().iter().any(|c| c == "discover true"));
}

#[tokio::test]
async fn an_unknown_resource_is_404() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_discovery(discovery_with_widgets());
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/r/nope.io/v1/things",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 404, "{:?}", r.json);
}

#[tokio::test]
async fn a_malformed_resource_path_is_refused() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/r/Bad_Group/v1/things",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
}

#[tokio::test]
async fn verbs_decide_what_can_be_changed() {
    let (t, admin) = kube_app().await;
    let mut ro = widgets(true);
    ro.verbs = vec!["get".into(), "list".into(), "watch".into()];
    t.fake.kube.set_discovery(Discovery {
        kinds: vec![ro.clone()],
        failed_groups: vec![],
    });
    t.fake.kube.generic_object(&ro, "apps", "w1");
    let del = t
        .send(req(
            "DELETE",
            &format!("{WIDGETS}/apps/w1"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(del.status, 400, "{:?}", del.json);
    let put = t
        .send(req(
            "PUT",
            &format!("{WIDGETS}/apps/w1"),
            Some(json!({"yaml": "x", "resource_version": "1"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(put.status, 400, "{:?}", put.json);
}

#[tokio::test]
async fn a_namespace_limited_user_sees_only_namespaced_kinds_in_their_namespaces() {
    let (t, admin) = kube_app().await;
    let k = &t.fake.kube;
    k.set_discovery(discovery_with_widgets());
    k.generic_object(&widgets(true), "apps", "w1");
    k.generic_object(&widgets(true), "other", "w2");
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["apps"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);

    let d = t
        .send(req("GET", "/api/v1/envs/k/k8s/discovery", None, Some(&bob)))
        .await;
    let kinds: Vec<&str> = d.json["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["Widget"]);
    let list = t.send(req("GET", WIDGETS, None, Some(&bob))).await;
    assert_eq!(names(&list), ["w1"]);
    let crds = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/r/apiextensions.k8s.io/v1/customresourcedefinitions",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(crds.status, 403, "{:?}", crds.json);
    let other = t
        .send(req("GET", &format!("{WIDGETS}/other/w2"), None, Some(&bob)))
        .await;
    assert_eq!(other.status, 403, "{:?}", other.json);
}
