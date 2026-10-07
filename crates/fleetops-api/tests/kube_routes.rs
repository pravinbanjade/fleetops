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
        .send(req("GET", "/api/v1/envs/k/k8s/widgets", None, Some(&admin)))
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
    t.grant("bob", "k", "operator", None).await;
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

#[tokio::test]
async fn rbac_objects_named_with_colons_can_be_opened() {
    let (t, admin) = kube_app().await;
    t.fake
        .kube
        .object(KubeKind::ClusterRoles, "-", "system:aggregate-to-admin");
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/clusterroles/-/system:aggregate-to-admin",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    // Colons are allowed; path tricks still are not.
    for path in [
        "/api/v1/envs/k/k8s/clusterroles/-/..",
        "/api/v1/envs/k/k8s/clusterroles/-/system:a%2F..%2Fb",
        "/api/v1/envs/k/k8s/clusterroles/-/system:a%3Fx",
    ] {
        let r = t.send(req("GET", path, None, Some(&admin))).await;
        assert_eq!(r.status, 400, "{path}: {:?}", r.json);
    }
}

#[tokio::test]
async fn names_and_namespaces_must_be_kubernetes_names() {
    let (t, admin) = kube_app().await;
    // An encoded slash or query would be spliced into the apiserver URL and
    // reach a different object (a Secret, read through the ConfigMap route).
    for path in [
        "/api/v1/envs/k/k8s/configmaps/default%2Fsecrets%2Fdb%3Fx%3D/y",
        "/api/v1/envs/k/k8s/configmaps/apps/web%2F..%2Fx",
        "/api/v1/envs/k/k8s/pods/Apps/web-1",
        "/api/v1/envs/k/k8s/pods?namespace=apps%2Fsecrets",
    ] {
        let r = t.send(req("GET", path, None, Some(&admin))).await;
        assert_eq!(r.status, 400, "{path}: {:?}", r.json);
    }
    let d = t
        .send(req(
            "DELETE",
            "/api/v1/envs/k/k8s/pods/apps%2Fx/web-1",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(d.status, 400);
    assert!(t.fake.kube.calls().iter().all(|c| !c.starts_with("delete")));
    // The cluster-scoped placeholder still works.
    let n = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/nodes/-/n1",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(n.status, 200);
}

#[tokio::test]
async fn edit_returns_yaml_and_a_resource_version() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/deployments/apps/web/edit",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["resource_version"], "1");
    assert!(r.json["yaml"].as_str().unwrap().contains("name: web"));
}

#[tokio::test]
async fn a_stale_resource_version_is_409() {
    let (t, admin) = kube_app().await;
    let body = json!({"yaml": "kind: Deployment\n", "resource_version": "0", "dry_run": false});
    let r = t
        .send(req(
            "PUT",
            "/api/v1/envs/k/k8s/deployments/apps/web",
            Some(body),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json["code"], "conflict");
    assert!(t.fake.kube.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_edit_is_audited_but_its_dry_run_is_not() {
    let (t, admin) = kube_app().await;
    for dry_run in [true, false] {
        let body =
            json!({"yaml": "kind: Deployment\n", "resource_version": "1", "dry_run": dry_run});
        let r = t
            .send(req(
                "PUT",
                "/api/v1/envs/k/k8s/deployments/apps/web",
                Some(body),
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, 200, "{:?}", r.json);
        assert_eq!(r.json["action"], "configured");
    }
    let audit = t.audit_actions().await;
    assert_eq!(
        audit.iter().filter(|a| *a == "k8s.edit").count(),
        1,
        "{audit:?}"
    );
}

#[tokio::test]
async fn apply_reports_each_document_and_audits_commits() {
    let (t, admin) = kube_app().await;
    let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\napiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: r\n";
    let dry = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": yaml, "namespace": "apps", "dry_run": true})),
            Some(&admin),
        ))
        .await;
    assert_eq!(dry.status, 200, "{:?}", dry.json);
    assert_eq!(
        t.audit_actions()
            .await
            .iter()
            .filter(|a| a.starts_with("k8s."))
            .count(),
        0
    );
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": yaml, "namespace": "apps"})),
            Some(&admin),
        ))
        .await;
    let docs = r.json.as_array().unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["action"], "created");
    assert_eq!(docs[1]["error"]["code"], "forbidden");
    let audit = t.audit_actions().await;
    assert!(audit.contains(&"k8s.create".to_string()), "{audit:?}");
    assert!(
        audit.contains(&"k8s.apply".to_string()),
        "the failure is audited: {audit:?}"
    );
}

#[tokio::test]
async fn apply_refuses_oversized_yaml_and_a_missing_namespace() {
    let (t, admin) = kube_app().await;
    let big = "x".repeat(fleetops_core::kube::APPLY_MAX_BYTES + 1);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": big, "namespace": "apps"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/apply",
            Some(json!({"yaml": "a: b", "namespace": "-"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn templates_are_listed_by_kind() {
    let (t, admin) = kube_app().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/k/k8s/templates",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert!(r.json["deployments"]
        .as_str()
        .unwrap()
        .contains("kind: Deployment"));
    assert!(r.json.get("nodes").is_none());
}

#[tokio::test]
async fn writes_need_csrf() {
    let (t, admin) = kube_app().await;
    let mut rq = req(
        "POST",
        "/api/v1/envs/k/k8s/apply",
        Some(json!({"yaml": "a: b", "namespace": "apps"})),
        Some(&admin),
    );
    rq.headers_mut().remove("x-csrf-token");
    assert_eq!(t.send(rq).await.status, 403);
}

#[tokio::test]
async fn an_edit_without_a_resource_version_is_refused() {
    let (t, admin) = kube_app().await;
    let body = json!({"yaml": "kind: Deployment\n", "resource_version": "", "dry_run": false});
    let r = t
        .send(req(
            "PUT",
            "/api/v1/envs/k/k8s/deployments/apps/web",
            Some(body),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
    assert!(t.fake.kube.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn suspend_and_resume_act_on_suspendable_kinds_and_are_audited() {
    let (t, admin) = kube_app().await;
    for (verb, kind, path) in [
        ("suspend", "deployments", "apps/web"),
        ("resume", "deployments", "apps/web"),
    ] {
        let r = t
            .send(req(
                "POST",
                &format!("/api/v1/envs/k/k8s/{kind}/{path}/{verb}"),
                None,
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, 204, "{verb}: {:?}", r.json);
    }
    let calls = t.fake.kube.calls();
    assert!(
        calls.contains(&"suspend deployments apps/web".to_string()),
        "{calls:?}"
    );
    assert!(calls.contains(&"resume deployments apps/web".to_string()));
    let pods = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/pods/apps/web-1/suspend",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(pods.status, 400);
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await
        .json
        .to_string();
    assert!(
        audit.contains("k8s.suspend") && audit.contains("k8s.resume"),
        "{audit}"
    );
}

#[tokio::test]
async fn a_viewer_cannot_suspend() {
    let (t, admin) = kube_app().await;
    t.create_user(&admin, "vic", "vic-password-1", "user").await;
    t.grant("vic", "k", "viewer", None).await;
    let vic = session_from(&t.login("vic", "vic-password-1").await);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/suspend",
            None,
            Some(&vic),
        ))
        .await;
    assert_eq!(r.status, 403);
    assert!(t
        .fake
        .kube
        .calls()
        .iter()
        .all(|c| !c.starts_with("suspend")));
}

#[tokio::test]
async fn nodes_cordon_and_cronjobs_run_now_and_both_are_audited() {
    let (t, admin) = kube_app().await;
    t.fake.kube.object(KubeKind::CronJobs, "apps", "nightly");
    for verb in ["cordon", "uncordon"] {
        let r = t
            .send(req(
                "POST",
                &format!("/api/v1/envs/k/k8s/nodes/-/n1/{verb}"),
                None,
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, 204, "{verb}: {:?}", r.json);
    }
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/k/k8s/cronjobs/apps/nightly/trigger",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["job"], "nightly-manual-1");
    let calls = t.fake.kube.calls();
    for c in ["cordon n1", "uncordon n1", "trigger apps/nightly"] {
        assert!(calls.contains(&c.to_string()), "{c}: {calls:?}");
    }
    // Each action belongs to its kind.
    for path in [
        "deployments/apps/web/cordon",
        "deployments/apps/web/trigger",
    ] {
        let r = t
            .send(req(
                "POST",
                &format!("/api/v1/envs/k/k8s/{path}"),
                None,
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, 400, "{path}");
    }
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await
        .json
        .to_string();
    for a in ["k8s.cordon", "k8s.uncordon", "k8s.trigger"] {
        assert!(audit.contains(a), "{a}: {audit}");
    }
}

#[tokio::test]
async fn a_viewer_cannot_cordon_or_run_now() {
    let (t, admin) = kube_app().await;
    t.fake.kube.object(KubeKind::CronJobs, "apps", "nightly");
    t.create_user(&admin, "vic", "vic-password-1", "user").await;
    t.grant("vic", "k", "viewer", None).await;
    let vic = session_from(&t.login("vic", "vic-password-1").await);
    for path in ["nodes/-/n1/cordon", "cronjobs/apps/nightly/trigger"] {
        let r = t
            .send(req(
                "POST",
                &format!("/api/v1/envs/k/k8s/{path}"),
                None,
                Some(&vic),
            ))
            .await;
        assert_eq!(r.status, 403, "{path}");
    }
    assert!(!t
        .fake
        .kube
        .calls()
        .iter()
        .any(|c| c.starts_with("cordon") || c.starts_with("trigger")));
}
