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
