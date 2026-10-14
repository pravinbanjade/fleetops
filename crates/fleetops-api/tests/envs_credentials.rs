mod common;

use fleetops_core::env::{Credential, EnvKind};
use serde_json::json;

/// The registry builds each environment with its decrypted credential.
#[tokio::test]
async fn load_hands_a_kube_environment_its_kubeconfig() {
    let t = common::app().await;
    let rec = t
        .store
        .create_environment_with_credential(
            "k",
            "kube-config",
            "https://k:6443",
            "kubeconfig",
            "token: x",
            &json!({}),
            1,
        )
        .await
        .unwrap();
    t.state.envs.load(&t.store).await.unwrap();
    let built = t.fake.built();
    let spec = built.iter().find(|s| s.id == rec.id).expect("built");
    assert_eq!(spec.kind, EnvKind::KubeConfig);
    assert_eq!(
        spec.credential,
        Some(Credential::Kubeconfig("token: x".into()))
    );
}

#[tokio::test]
async fn kube_environments_cannot_be_created_yet() {
    let t = common::app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "k", "kind": "kube-config"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
}

const EXEC_KUBECONFIG: &str = "apiVersion: v1\nkind: Config\ncurrent-context: c\nclusters:\n- name: c\n  cluster: {server: 'https://k.example:6443'}\ncontexts:\n- name: c\n  context: {cluster: c, user: u}\nusers:\n- name: u\n  user:\n    exec: {apiVersion: client.authentication.k8s.io/v1, command: aws, args: [eks, get-token, --cluster-name, secret-name]}\n";

fn exec_body() -> serde_json::Value {
    json!({"name": "eks", "kind": "kube-config", "kubeconfig": EXEC_KUBECONFIG, "skip_check": true})
}

#[tokio::test]
async fn an_exec_kubeconfig_is_refused_while_the_flag_is_off() {
    let t = common::app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs",
            Some(exec_body()),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
    assert!(
        r.json.to_string().contains("--kube-exec-plugins=default"),
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn with_the_flag_it_is_saved_and_shown_by_command_only() {
    let config = fleetops_api::ApiConfig {
        kube_exec: fleetops_kube::exec_auth::ExecPolicy::parse("default").unwrap(),
        ..Default::default()
    };
    let t = common::app_with(config).await;
    let admin = t.setup_admin().await;
    let r = t
        .send(common::req(
            "POST",
            "/api/v1/envs",
            Some(exec_body()),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    let list = t
        .send(common::req("GET", "/api/v1/envs", None, Some(&admin)))
        .await;
    let eks = list
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "eks")
        .unwrap()
        .clone();
    assert_eq!(eks["kube"]["user_kind"], "exec");
    assert_eq!(eks["kube"]["exec_command"], "aws");
    assert!(
        !list.json.to_string().contains("secret-name"),
        "arguments never leave the server"
    );
    let audit = t
        .send(common::req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let rows = audit.json.to_string();
    assert!(
        rows.contains("\"exec_command\":\"aws\"") && !rows.contains("secret-name"),
        "{rows}"
    );
}
