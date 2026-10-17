//! Every environment route asks for exactly one permission (custom roles
//! spec, "The grid"); built-in roles keep their old reach.

mod common;

use async_trait::async_trait;
use common::*;
use fleetops_core::access::{CustomRef, EnvRole, ExtraGrants, Grant};
use fleetops_core::kube::KubeKind;
use fleetops_core::perm::{admin_perms, PermSet, Permission};
use fleetops_core::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Grants set by the test, by (user id, environment id).
#[derive(Default)]
struct Fixed(Mutex<HashMap<(i64, i64), Vec<Grant>>>);

#[async_trait]
impl ExtraGrants for Fixed {
    async fn env_grants(&self, user_id: i64, env_id: i64) -> Result<Vec<Grant>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(user_id, env_id))
            .cloned()
            .unwrap_or_default())
    }
    async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|((u, _), _)| *u == user_id)
            .map(|((_, e), g)| (*e, g.clone()))
            .collect())
    }
}

struct Rig {
    t: TestApp,
    extra: Arc<Fixed>,
    bob: Session,
    bob_id: i64,
    envs: HashMap<&'static str, i64>,
}

impl Rig {
    async fn new() -> Rig {
        let extra = Arc::new(Fixed::default());
        let config = fleetops_api::ApiConfig {
            helm: Some(Arc::new(FakeRenderer::default())),
            ..Default::default()
        };
        let t = app_with_extra_and(extra.clone(), config).await;
        let admin = t.with_local().await;
        t.add_kube_env(&admin, "k").await;
        let k = &t.fake.kube;
        k.object(KubeKind::Deployments, "apps", "web");
        k.object(KubeKind::Pods, "apps", "web-1");
        k.object(KubeKind::Secrets, "apps", "creds");
        k.object(KubeKind::Namespaces, "-", "apps");
        k.object(KubeKind::Nodes, "-", "n1");
        t.create_user(&admin, "bob", "bob-password-1", "user").await;
        let bob = session_from(&t.login("bob", "bob-password-1").await);
        let (u, _) = t.store.find_credentials("bob").await.unwrap().unwrap();
        let envs = t
            .store
            .list_environments()
            .await
            .unwrap()
            .into_iter()
            .map(|e| (if e.name == "local" { "local" } else { "k" }, e.id))
            .collect();
        Rig {
            t,
            extra,
            bob,
            bob_id: u.id,
            envs,
        }
    }

    /// Bob holds exactly `grants` on both environments.
    fn give(&self, grants: Vec<Grant>) {
        let mut m = self.extra.0.lock().unwrap();
        m.clear();
        for id in self.envs.values() {
            m.insert((self.bob_id, *id), grants.clone());
        }
    }

    async fn send(&self, c: &Case) -> Resp {
        self.t
            .send(req(c.method, c.path, c.body.clone(), Some(&self.bob)))
            .await
    }
}

fn custom(perms: PermSet) -> Grant {
    Grant::custom(
        CustomRef {
            id: 1,
            name: "test".into(),
        },
        perms,
        None,
    )
}

struct Case {
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    need: &'static str,
}

fn c(method: &'static str, path: &'static str, body: Option<Value>, need: &'static str) -> Case {
    Case {
        method,
        path,
        body,
        need,
    }
}

/// One request per route, with the permission the route table names. HTTP
/// routes only; WebSocket topics are covered in `ws_*` tests.
fn cases() -> Vec<Case> {
    vec![
        c(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            "containers.view",
        ),
        c(
            "GET",
            "/api/v1/envs/local/containers/c1",
            None,
            "containers.view",
        ),
        c(
            "GET",
            "/api/v1/envs/local/containers/c1/spec",
            None,
            "containers.view",
        ),
        c(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"name": "web", "image": "nginx:1.27"})),
            "containers.create",
        ),
        c(
            "POST",
            "/api/v1/envs/local/containers/c1/duplicate",
            Some(json!({"name": "web-copy"})),
            "containers.create",
        ),
        c(
            "POST",
            "/api/v1/envs/local/containers/c1/start",
            None,
            "containers.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/local/containers/c1/rename",
            Some(json!({"name": "web2"})),
            "containers.operate",
        ),
        c(
            "DELETE",
            "/api/v1/envs/local/containers/c1",
            None,
            "containers.delete",
        ),
        c(
            "POST",
            "/api/v1/envs/local/containers/prune",
            None,
            "containers.delete",
        ),
        c(
            "GET",
            "/api/v1/envs/local/containers/c1/files?path=/",
            None,
            "containers.reveal",
        ),
        c(
            "GET",
            "/api/v1/envs/local/containers/c1/files/download?path=/etc/hosts",
            None,
            "containers.reveal",
        ),
        c("GET", "/api/v1/envs/local/images", None, "images.view"),
        c(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine",
            None,
            "images.view",
        ),
        c(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine/history",
            None,
            "images.view",
        ),
        c(
            "POST",
            "/api/v1/envs/local/images/sha256:alpine/tag",
            Some(json!({"repo": "me/alpine"})),
            "images.create",
        ),
        c(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine"})),
            "images.create",
        ),
        c(
            "DELETE",
            "/api/v1/envs/local/images/sha256:alpine",
            None,
            "images.delete",
        ),
        c(
            "POST",
            "/api/v1/envs/local/images/prune",
            None,
            "images.delete",
        ),
        c("GET", "/api/v1/envs/local/volumes", None, "volumes.view"),
        c(
            "GET",
            "/api/v1/envs/local/volumes/data",
            None,
            "volumes.view",
        ),
        c(
            "POST",
            "/api/v1/envs/local/volumes",
            Some(json!({"name": "data2"})),
            "volumes.create",
        ),
        c(
            "DELETE",
            "/api/v1/envs/local/volumes/data",
            None,
            "volumes.delete",
        ),
        c(
            "POST",
            "/api/v1/envs/local/volumes/prune",
            None,
            "volumes.delete",
        ),
        c("GET", "/api/v1/envs/local/networks", None, "networks.view"),
        c(
            "GET",
            "/api/v1/envs/local/networks/n1",
            None,
            "networks.view",
        ),
        c(
            "POST",
            "/api/v1/envs/local/networks",
            Some(json!({"name": "backend"})),
            "networks.create",
        ),
        c(
            "POST",
            "/api/v1/envs/local/networks/n1/connect",
            Some(json!({"container": "c1"})),
            "networks.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/local/networks/n1/disconnect",
            Some(json!({"container": "c1"})),
            "networks.operate",
        ),
        c(
            "DELETE",
            "/api/v1/envs/local/networks/n1",
            None,
            "networks.delete",
        ),
        c(
            "POST",
            "/api/v1/envs/local/networks/prune",
            None,
            "networks.delete",
        ),
        c(
            "GET",
            "/api/v1/envs/local/compose-projects",
            None,
            "stacks.view",
        ),
        c(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/restart",
            None,
            "stacks.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/down",
            None,
            "stacks.delete",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/deployments",
            None,
            "workloads.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/deployments/apps/web",
            None,
            "workloads.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/deployments/apps/web/events",
            None,
            "workloads.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/deployments/apps/web/pods",
            None,
            "workloads.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/deployments/apps/web/edit",
            None,
            "workloads.view",
        ),
        c(
            "PUT",
            "/api/v1/envs/k/k8s/deployments/apps/web",
            Some(json!({"yaml": "kind: Deployment", "resource_version": "1"})),
            "workloads.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/scale",
            Some(json!({"replicas": 3})),
            "workloads.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/k/k8s/deployments/apps/web/restart",
            None,
            "workloads.operate",
        ),
        c(
            "DELETE",
            "/api/v1/envs/k/k8s/deployments/apps/web",
            None,
            "workloads.delete",
        ),
        c("GET", "/api/v1/envs/k/k8s/pods", None, "pods.view"),
        c("GET", "/api/v1/envs/k/k8s/metrics/pods", None, "pods.view"),
        c(
            "DELETE",
            "/api/v1/envs/k/k8s/pods/apps/web-1",
            None,
            "pods.delete",
        ),
        c("GET", "/api/v1/envs/k/k8s/secrets", None, "secrets.view"),
        c(
            "GET",
            "/api/v1/envs/k/k8s/secrets/apps/creds",
            None,
            "secrets.view",
        ),
        c(
            "POST",
            "/api/v1/envs/k/k8s/nodes/-/n1/cordon",
            None,
            "k8s-other.operate",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/serviceaccounts",
            None,
            "k8s-other.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/r/apiextensions.k8s.io/v1/customresourcedefinitions",
            None,
            "k8s-other.view",
        ),
        c("GET", "/api/v1/envs/k/k8s/helm/releases", None, "helm.view"),
        c(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases/apps/demo",
            None,
            "helm.view",
        ),
        c(
            "GET",
            "/api/v1/envs/k/k8s/helm/releases/apps/demo/history",
            None,
            "helm.view",
        ),
        c(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps/demo/rollback",
            Some(json!({"revision": 1, "expected_current": 2, "dry_run": true})),
            "helm.operate",
        ),
        c(
            "POST",
            "/api/v1/envs/k/k8s/helm/releases/apps/demo/uninstall",
            Some(json!({"expected_current": 1, "dry_run": true})),
            "helm.delete",
        ),
    ]
}

fn p(s: &str) -> Permission {
    s.parse().unwrap()
}

fn without(set: PermSet, gone: Permission) -> PermSet {
    set.iter().filter(|x| *x != gone).collect()
}

#[tokio::test]
async fn each_route_is_refused_without_exactly_its_permission() {
    let r = Rig::new().await;
    for c in cases() {
        r.give(vec![custom(without(admin_perms(), p(c.need)))]);
        let resp = r.send(&c).await;
        assert_eq!(
            resp.status, 403,
            "{} {} without {}: {:?}",
            c.method, c.path, c.need, resp.json
        );
        assert_eq!(
            resp.json["message"],
            format!("{} permission required", c.need),
            "{} {}",
            c.method,
            c.path
        );
    }
}

#[tokio::test]
async fn each_route_is_allowed_with_its_permission() {
    let r = Rig::new().await;
    for c in cases() {
        r.give(vec![custom(PermSet::default().with(p(c.need)))]);
        let resp = r.send(&c).await;
        assert_ne!(
            resp.status, 403,
            "{} {} with only {}: {:?}",
            c.method, c.path, c.need, resp.json
        );
    }
}

#[tokio::test]
async fn builtin_parity() {
    let r = Rig::new().await;
    for role in [EnvRole::Viewer, EnvRole::Operator] {
        r.give(vec![Grant::full(role)]);
        for c in cases() {
            let resp = r.send(&c).await;
            let allowed = role.perms().contains(p(c.need));
            assert_eq!(
                resp.status == 403,
                !allowed,
                "{role:?} {} {}: {:?}",
                c.method,
                c.path,
                resp.json
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_is_audited_with_the_permission() {
    let r = Rig::new().await;
    r.give(vec![custom(PermSet::default().with(p("volumes.view")))]);
    let resp = r
        .send(&c("POST", "/api/v1/envs/local/volumes/prune", None, ""))
        .await;
    assert_eq!(resp.status, 403);
    let rows = r.t.store.list_audit(50, None).await.unwrap();
    let denied = rows.iter().find(|a| a.action == "access.denied").unwrap();
    assert_eq!(
        denied.detail.as_ref().unwrap()["permission"],
        "volumes.delete"
    );
}

#[tokio::test]
async fn deployer_story() {
    let r = Rig::new().await;
    let perms: PermSet = [
        "containers.view",
        "containers.create",
        "containers.operate",
        "volumes.view",
        "volumes.create",
    ]
    .into_iter()
    .map(p)
    .collect();
    r.give(vec![custom(perms)]);
    let ok = r
        .send(&c(
            "POST",
            "/api/v1/envs/local/containers/c1/stop",
            None,
            "",
        ))
        .await;
    assert_ne!(ok.status, 403, "{:?}", ok.json);
    for (m, path) in [
        ("DELETE", "/api/v1/envs/local/containers/c1"),
        ("DELETE", "/api/v1/envs/local/volumes/data"),
        ("POST", "/api/v1/envs/local/volumes/prune"),
    ] {
        assert_eq!(
            r.send(&c(m, path, None, "")).await.status,
            403,
            "{m} {path}"
        );
    }
}

#[tokio::test]
async fn recreate_needs_create_and_delete() {
    let r = Rig::new().await;
    let body = Some(json!({"image": "nginx:2"}));
    let path = "/api/v1/envs/local/containers/c1/recreate";
    r.give(vec![custom(
        PermSet::default().with(p("containers.create")),
    )]);
    let resp = r.send(&c("POST", path, body.clone(), "")).await;
    assert_eq!(resp.status, 403);
    assert_eq!(
        resp.json["message"],
        "containers.delete permission required"
    );
    r.give(vec![custom(
        PermSet::default()
            .with(p("containers.create"))
            .with(p("containers.delete")),
    )]);
    assert_ne!(r.send(&c("POST", path, body, "")).await.status, 403);
}

#[tokio::test]
async fn narrow_operator_wide_viewer() {
    let r = Rig::new().await;
    r.t.fake.kube.object(KubeKind::Pods, "other", "db-1");
    r.give(vec![
        Grant::builtin(EnvRole::Operator, Some(["apps".to_string()].into())),
        Grant::full(EnvRole::Viewer),
    ]);
    let del = |path| c("DELETE", path, None, "");
    assert_eq!(
        r.send(&del("/api/v1/envs/k/k8s/pods/other/db-1"))
            .await
            .status,
        403
    );
    assert_ne!(
        r.send(&del("/api/v1/envs/k/k8s/pods/apps/web-1"))
            .await
            .status,
        403
    );
    let list = r
        .send(&c(
            "GET",
            "/api/v1/envs/k/k8s/pods?namespace=other",
            None,
            "",
        ))
        .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.json["rows"][0]["name"], "db-1");
}

#[tokio::test]
async fn counts_skip_kinds_the_role_cannot_view() {
    let r = Rig::new().await;
    r.give(vec![custom(PermSet::default().with(p("pods.view")))]);
    let resp = r
        .send(&c(
            "GET",
            "/api/v1/envs/k/k8s/counts?kinds=pods,secrets",
            None,
            "",
        ))
        .await;
    assert_eq!(resp.status, 200, "{:?}", resp.json);
    assert_eq!(resp.json["pods"]["count"], 1);
    assert_eq!(resp.json["secrets"]["error"], "forbidden");
}

const CM_THEN_SECRET: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: s\n";

async fn apply(r: &Rig, yaml: &str, dry_run: bool) -> Resp {
    r.t.send(req(
        "POST",
        "/api/v1/envs/k/k8s/apply",
        Some(json!({"yaml": yaml, "namespace": "apps", "dry_run": dry_run})),
        Some(&r.bob),
    ))
    .await
}

fn perms(v: &[&str]) -> PermSet {
    v.iter().map(|s| p(s)).collect()
}

#[tokio::test]
async fn apply_refusal_sends_nothing() {
    let r = Rig::new().await;
    r.give(vec![custom(perms(&[
        "configmaps.create",
        "configmaps.operate",
    ]))]);
    for dry_run in [true, false] {
        let resp = apply(&r, CM_THEN_SECRET, dry_run).await;
        assert_eq!(resp.status, 200, "{:?}", resp.json);
        assert_eq!(
            resp.json[0]["error"]["message"],
            "not sent: another document was refused"
        );
        assert_eq!(
            resp.json[1]["error"]["message"],
            "secrets.create permission required"
        );
        assert!(resp.json[0]["action"].is_null() && resp.json[1]["action"].is_null());
        assert!(
            r.t.fake.kube.applied.lock().unwrap().is_empty(),
            "dry_run {dry_run}"
        );
    }
}

#[tokio::test]
async fn apply_allows_with_create_and_operate() {
    let r = Rig::new().await;
    r.give(vec![custom(perms(&[
        "configmaps.create",
        "configmaps.operate",
        "secrets.create",
        "secrets.operate",
    ]))]);
    let resp = apply(&r, CM_THEN_SECRET, false).await;
    assert!(
        resp.json[0]["error"].is_null() && resp.json[1]["error"].is_null(),
        "{:?}",
        resp.json
    );
    assert_eq!(r.t.fake.kube.applied.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn apply_unknown_kind_maps_to_k8s_other() {
    let r = Rig::new().await;
    let yaml = "apiVersion: cert-manager.io/v1\nkind: Certificate\nmetadata:\n  name: c\n";
    r.give(vec![custom(perms(&[
        "workloads.create",
        "workloads.operate",
    ]))]);
    let resp = apply(&r, yaml, false).await;
    assert_eq!(
        resp.json[0]["error"]["message"],
        "k8s-other.create permission required"
    );
    r.give(vec![custom(perms(&[
        "k8s-other.create",
        "k8s-other.operate",
    ]))]);
    assert!(apply(&r, yaml, false).await.json[0]["error"].is_null());
}

#[tokio::test]
async fn apply_without_any_create_is_refused_whole() {
    let r = Rig::new().await;
    r.give(vec![custom(perms(&["pods.view"]))]);
    let resp = apply(&r, CM_THEN_SECRET, false).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json[0]["error"]["code"], "forbidden");
}

#[tokio::test]
async fn helm_install_needs_helm_create_only() {
    let r = Rig::new().await;
    let body = json!({"name": "demo", "chart": {"repo_url": "https://c.example", "chart": "demo"}, "values": "", "dry_run": true, "expected_current": 0});
    let install = c(
        "POST",
        "/api/v1/envs/k/k8s/helm/releases/apps",
        Some(body),
        "",
    );
    r.give(vec![custom(perms(&["helm.view", "helm.operate"]))]);
    let resp = r.send(&install).await;
    assert_eq!(resp.status, 403, "{:?}", resp.json);
    assert_eq!(resp.json["message"], "helm.create permission required");
    r.give(vec![custom(perms(&["helm.create"]))]);
    assert_ne!(r.send(&install).await.status, 403);
}
