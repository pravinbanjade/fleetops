mod common;

use common::*;
use fleetops_core::env::{Credential, EnvKind};
use serde_json::json;

fn post(path: &str, body: serde_json::Value, s: &Session) -> axum::http::Request<axum::body::Body> {
    let mut r = req("POST", path, Some(body), Some(s));
    r.headers_mut()
        .insert("origin", "https://fleet.example".parse().unwrap());
    r
}

#[tokio::test]
async fn a_kubeconfig_environment_is_stored_encrypted_and_never_returned() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post("/api/v1/envs", json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true}), &admin))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(r.json["url"], "https://127.0.0.1:1");
    assert!(!r.json.to_string().contains("very-secret-token"));
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert!(!list.json.to_string().contains("very-secret-token"));
    let k = list
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "k")
        .unwrap();
    assert_eq!(
        k["kube"],
        json!({"server": "https://127.0.0.1:1", "context": "dev", "user_kind": "token"})
    );
    let spec = t
        .fake
        .built()
        .into_iter()
        .rev()
        .find(|s| s.name == "k")
        .unwrap();
    assert_eq!(spec.kind, EnvKind::KubeConfig);
    assert!(
        matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("very-secret-token") && !y.contains("other"))
    );
}

#[tokio::test]
async fn an_unreachable_cluster_is_refused_unless_the_check_is_skipped() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG}),
            &admin,
        ))
        .await;
    assert_eq!(r.status, 400);
    assert!(
        r.json["message"]
            .as_str()
            .unwrap()
            .contains("cannot reach cluster"),
        "{:?}",
        r.json
    );
    assert!(
        t.store.list_environments().await.unwrap().is_empty(),
        "nothing stored"
    );
}

#[tokio::test]
async fn bad_kubeconfigs_are_refused_with_the_reason() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let exec = KUBECONFIG.replace(
        "{token: very-secret-token}",
        "{exec: {apiVersion: v1, command: aws}}",
    );
    for (body, needle) in [
        (
            json!({"name": "k", "kind": "kube-config", "kubeconfig": exec, "skip_check": true}),
            "exec plugins are off on this server",
        ),
        (
            json!({"name": "k", "kind": "kube-config", "skip_check": true}),
            "a kubeconfig is required",
        ),
        (
            json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "url": "https://x", "skip_check": true}),
            "takes no URL",
        ),
        (
            json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "context": "nope", "skip_check": true}),
            "unknown context 'nope'",
        ),
    ] {
        let r = t.send(post("/api/v1/envs", body, &admin)).await;
        assert_eq!(r.status, 400);
        assert!(
            r.json["message"].as_str().unwrap().contains(needle),
            "{needle}: {:?}",
            r.json
        );
    }
}

#[tokio::test]
async fn contexts_lists_a_kubeconfigs_contexts_for_admins_only() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post(
            "/api/v1/kube/contexts",
            json!({"kubeconfig": KUBECONFIG}),
            &admin,
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(
        r.json,
        json!({"contexts": ["dev", "other"], "current": "dev"})
    );
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let user = session_from(&t.login("bob", "bob-password-1").await);
    let r = t
        .send(post(
            "/api/v1/kube/contexts",
            json!({"kubeconfig": KUBECONFIG}),
            &user,
        ))
        .await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn a_kube_agent_environment_gets_a_kubectl_manifest() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "k8s-01", "kind": "kube-agent"}),
            &admin,
        ))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    let token = r.json["enrollment"]["join_token"].as_str().unwrap();
    let cmd = r.json["enrollment"]["command"].as_str().unwrap();
    assert!(cmd.starts_with("kubectl apply -f - <<'EOF'"), "{cmd}");
    assert!(cmd.contains(token) && cmd.contains("- https://fleet.example"));
    assert_eq!(r.json["kind"], "kube-agent");
    let rot = t
        .send(post("/api/v1/envs/k8s-01/agent/rotate", json!({}), &admin))
        .await;
    assert_eq!(rot.status, 200, "{:?}", rot.json);
    assert!(rot.json["command"]
        .as_str()
        .unwrap()
        .starts_with("kubectl apply"));
}

#[tokio::test]
async fn updating_a_kubeconfig_replaces_it_and_rebuilds_the_environment() {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.send(post(
        "/api/v1/envs",
        json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true}),
        &admin,
    ))
    .await;
    let newer = KUBECONFIG
        .replace("127.0.0.1:1", "127.0.0.1:2")
        .replace("very-secret-token", "newer-token");
    let r = t
        .send(req(
            "PATCH",
            "/api/v1/envs/k",
            Some(json!({"kubeconfig": newer, "skip_check": true})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["url"], "https://127.0.0.1:2");
    let spec = t
        .fake
        .built()
        .into_iter()
        .rev()
        .find(|s| s.name == "k")
        .unwrap();
    assert!(
        matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("newer-token"))
    );
    // A rename alone keeps the stored kubeconfig.
    t.send(req(
        "PATCH",
        "/api/v1/envs/k",
        Some(json!({"name": "k2"})),
        Some(&admin),
    ))
    .await;
    let spec = t
        .fake
        .built()
        .into_iter()
        .rev()
        .find(|s| s.name == "k2")
        .unwrap();
    assert!(
        matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("newer-token"))
    );
    let bad = t
        .send(req(
            "PATCH",
            "/api/v1/envs/k2",
            Some(json!({"url": "https://x"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn a_kube_agent_keeps_its_access_level_across_rotation() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let created = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "ka", "kind": "kube-agent", "access": "cluster-admin"}),
            &admin,
        ))
        .await;
    assert_eq!(created.status, 201, "{:?}", created.json);
    assert!(created.json["enrollment"]["command"]
        .as_str()
        .unwrap()
        .contains("name: cluster-admin"));
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let ka = list
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ka")
        .unwrap()
        .clone();
    assert_eq!(ka["agent"]["access"], "cluster-admin");
    let rotated = t
        .send(post("/api/v1/envs/ka/agent/rotate", json!({}), &admin))
        .await;
    assert!(rotated.json["command"]
        .as_str()
        .unwrap()
        .contains("name: cluster-admin"));
}

#[tokio::test]
async fn a_kube_agent_can_read_custom_resources_and_keeps_it_across_rotation() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let created = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "kc", "kind": "kube-agent", "access": "read-only", "custom_resources": true}),
            &admin,
        ))
        .await;
    assert_eq!(created.status, 201, "{:?}", created.json);
    let everything = r#"apiGroups: ["*"]"#;
    assert!(created.json["enrollment"]["command"]
        .as_str()
        .unwrap()
        .contains(everything));
    let rotated = t
        .send(post("/api/v1/envs/kc/agent/rotate", json!({}), &admin))
        .await;
    assert!(rotated.json["command"]
        .as_str()
        .unwrap()
        .contains(everything));
    let plain = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "kp", "kind": "kube-agent"}),
            &admin,
        ))
        .await;
    assert!(!plain.json["enrollment"]["command"]
        .as_str()
        .unwrap()
        .contains(everything));
    let docker = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "da", "kind": "docker-agent", "custom_resources": true}),
            &admin,
        ))
        .await;
    assert_eq!(docker.status, 400, "{:?}", docker.json);
}

#[tokio::test]
async fn a_kube_agent_defaults_to_standard_access() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let created = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "ks", "kind": "kube-agent"}),
            &admin,
        ))
        .await;
    let cmd = created.json["enrollment"]["command"].as_str().unwrap();
    assert!(cmd.contains("# Access level: standard") && !cmd.contains("name: cluster-admin\n"));
}

#[tokio::test]
async fn only_a_kube_agent_takes_an_access_level() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post(
            "/api/v1/envs",
            json!({"name": "d", "kind": "docker-local", "url": "unix:///x.sock", "access": "standard"}),
            &admin,
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
}
