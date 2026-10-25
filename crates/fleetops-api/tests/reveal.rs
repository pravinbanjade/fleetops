//! Reveal (custom roles spec, "What Reveal hides when it's missing").

mod common;

use async_trait::async_trait;
use common::*;
use fleetops_core::access::{CustomRef, EnvRole, ExtraGrants, Grant};
use fleetops_core::kube::{KubeKind, MASK};
use fleetops_core::perm::PermSet;
use fleetops_core::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fixed(Mutex<Vec<Grant>>);

#[async_trait]
impl ExtraGrants for Fixed {
    async fn env_grants(&self, _: i64, _: i64) -> Result<Vec<Grant>> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn user_grants(&self, _: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        Ok(HashMap::new())
    }
}

struct Rig {
    t: TestApp,
    extra: Arc<Fixed>,
    bob: Session,
    admin: Session,
}

async fn rig() -> Rig {
    let extra = Arc::new(Fixed::default());
    let config = fleetops_api::ApiConfig {
        helm: Some(Arc::new(FakeRenderer::default())),
        ..Default::default()
    };
    let t = app_with_extra_and(extra.clone(), config).await;
    let admin = t.with_local().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Secrets, "apps", "creds");
    t.fake.kube.helm_release("apps", "demo", 1, "deployed");
    t.fake.kube.secret_data.lock().unwrap().insert(
        ("apps".into(), "creds".into()),
        vec![
            ("password".into(), b"s3cret".to_vec()),
            ("blob".into(), vec![0xff, 0x00]),
        ],
    );
    {
        let mut st = t.fake.state.lock().unwrap();
        st.containers
            .push(common::fake_docker::summary("c1", "web", "running", None));
        st.env.insert(
            "c1".into(),
            vec!["DB_PASSWORD=s3cret".into(), "MODE=prod".into()],
        );
    }
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    Rig {
        t,
        extra,
        bob,
        admin,
    }
}

impl Rig {
    fn give(&self, perms: &[&str]) {
        let set: PermSet = perms.iter().map(|p| p.parse().unwrap()).collect();
        *self.extra.0.lock().unwrap() = vec![Grant::custom(
            CustomRef {
                id: 1,
                name: "r".into(),
            },
            set,
            None,
        )];
    }
    fn give_builtin(&self, role: EnvRole) {
        *self.extra.0.lock().unwrap() = vec![Grant::full(role)];
    }
    async fn get(&self, path: &str) -> Resp {
        self.t.send(req("GET", path, None, Some(&self.bob))).await
    }
}

#[tokio::test]
async fn container_env_masked_without_reveal() {
    let r = rig().await;
    r.give(&["containers.view"]);
    let spec = r.get("/api/v1/envs/local/containers/c1/spec").await;
    assert_eq!(
        spec.json["env"],
        json!([format!("DB_PASSWORD={MASK}"), format!("MODE={MASK}")])
    );
    let inspect = r.get("/api/v1/envs/local/containers/c1").await;
    assert_eq!(
        inspect.json["Config"]["Env"][0],
        format!("DB_PASSWORD={MASK}")
    );
    r.give(&["containers.view", "containers.reveal"]);
    let spec = r.get("/api/v1/envs/local/containers/c1/spec").await;
    assert_eq!(spec.json["env"], json!(["DB_PASSWORD=s3cret", "MODE=prod"]));
}

#[tokio::test]
async fn recreate_with_masked_env_keeps_values() {
    let r = rig().await;
    r.give(&["containers.view", "containers.create", "containers.delete"]);
    let mut spec = r.get("/api/v1/envs/local/containers/c1/spec").await.json;
    spec["env"] = json!([format!("DB_PASSWORD={MASK}"), "MODE=dev"]);
    let resp =
        r.t.send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/recreate",
            Some(spec),
            Some(&r.bob),
        ))
        .await;
    assert_eq!(resp.status, 200, "{:?}", resp.json);
    let created =
        r.t.fake
            .state
            .lock()
            .unwrap()
            .created_specs
            .last()
            .cloned()
            .unwrap();
    assert_eq!(created.env, ["DB_PASSWORD=s3cret", "MODE=dev"]);
}

/// A masked key the container lacks (renamed, say) has no value to keep:
/// refused, not dropped.
#[tokio::test]
async fn recreate_with_an_unknown_masked_key_is_refused() {
    let r = rig().await;
    r.give(&["containers.view", "containers.create", "containers.delete"]);
    let mut spec = r.get("/api/v1/envs/local/containers/c1/spec").await.json;
    spec["env"] = json!([format!("DB_PASS={MASK}"), "MODE=dev"]);
    let resp =
        r.t.send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/recreate",
            Some(spec),
            Some(&r.bob),
        ))
        .await;
    assert_eq!(resp.status, 400, "{:?}", resp.json);
    assert!(
        resp.json["message"]
            .as_str()
            .unwrap()
            .contains("DB_PASS has a hidden value"),
        "{:?}",
        resp.json
    );
}

#[tokio::test]
async fn helm_values_and_notes_hidden_without_reveal() {
    let r = rig().await;
    r.give(&["helm.view"]);
    let d = r.get("/api/v1/envs/k/k8s/helm/releases/apps/demo").await;
    assert_eq!(d.status, 200, "{:?}", d.json);
    assert_eq!(d.json["values"], "");
    assert_eq!(d.json["notes"], "");
    assert_eq!(d.json["hidden"], true);
    r.give_builtin(EnvRole::Viewer);
    let d = r.get("/api/v1/envs/k/k8s/helm/releases/apps/demo").await;
    assert_eq!(d.json["notes"], "hello");
    assert_eq!(d.json["hidden"], false);
}

#[tokio::test]
async fn secret_values_need_reveal() {
    let r = rig().await;
    let path = "/api/v1/envs/k/k8s/secrets/apps/creds/values";
    r.give(&["secrets.view"]);
    let resp = r.get(path).await;
    assert_eq!(resp.status, 403);
    assert_eq!(resp.json["message"], "secrets.reveal permission required");
    r.give_builtin(EnvRole::Operator);
    assert_eq!(r.get(path).await.status, 403);
    r.give(&["secrets.reveal"]);
    let resp = r.get(path).await;
    assert_eq!(resp.status, 200, "{:?}", resp.json);
    assert_eq!(
        resp.json,
        json!([
            {"key": "password", "value": "s3cret", "binary": false},
            {"key": "blob", "value": "/wA=", "binary": true}
        ])
    );
    let audit = r.t.store.list_audit(10, None).await.unwrap();
    let row = audit.iter().find(|a| a.action == "secret.reveal").unwrap();
    assert!(
        row.target_id.as_deref().unwrap().contains("apps/creds"),
        "{row:?}"
    );
    assert!(!format!("{row:?}").contains("s3cret"));
    let admin = r.t.send(req("GET", path, None, Some(&r.admin))).await;
    assert_eq!(admin.status, 200);
}

#[tokio::test]
async fn an_admin_reveals_secret_values_with_no_grant() {
    let r = rig().await;
    *r.extra.0.lock().unwrap() = vec![];
    let path = "/api/v1/envs/k/k8s/secrets/apps/creds/values";
    let resp = r.t.send(req("GET", path, None, Some(&r.admin))).await;
    assert_eq!(resp.status, 200, "{:?}", resp.json);
    let envs =
        r.t.send(req("GET", "/api/v1/envs", None, Some(&r.admin)))
            .await;
    let k = envs
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "k")
        .unwrap();
    let perms = k["access"]["grants"][0]["permissions"]
        .as_array()
        .unwrap()
        .clone();
    assert!(perms.contains(&json!("secrets.reveal")), "{perms:?}");
}

#[tokio::test]
async fn helm_plans_hide_values_and_notes_without_reveal() {
    let r = rig().await;
    let body = json!({"chart": {"oci": "oci://r/demo", "version": "0.2.0"}, "values": "replicas: 2\n", "dry_run": true});
    let path = "/api/v1/envs/k/k8s/helm/releases/apps/demo/upgrade";
    r.give(&["helm.view", "helm.operate"]);
    let plan =
        r.t.send(req("POST", path, Some(body.clone()), Some(&r.bob)))
            .await;
    assert_eq!(plan.status, 200, "{:?}", plan.json);
    assert!(plan.json["values_before"].is_null(), "{:?}", plan.json);
    assert!(plan.json["values_after"].is_null());
    assert_eq!(plan.json["notes"], "");
    r.give(&["helm.view", "helm.operate", "helm.reveal"]);
    let plan = r.t.send(req("POST", path, Some(body), Some(&r.bob))).await;
    assert!(plan.json["values_before"].is_string(), "{:?}", plan.json);
}
