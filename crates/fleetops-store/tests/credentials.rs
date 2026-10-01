use fleetops_core::env::Credential;
use fleetops_store::{secrets::SecretKey, Store};
use serde_json::json;

const KUBECONFIG: &str = "apiVersion: v1\nusers: [{name: u, user: {token: s3cret}}]\n";

#[tokio::test]
async fn a_credential_round_trips_and_is_stored_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("f.db");
    let key = SecretKey::random();
    let store = Store::open(&db).await.unwrap().with_secret_key(&key);
    let summary = json!({"server": "https://k:6443", "context": "c", "user_kind": "token"});
    let rec = store
        .create_environment_with_credential(
            "k",
            "kube-config",
            "https://k:6443",
            "kubeconfig",
            KUBECONFIG,
            &summary,
            1,
        )
        .await
        .unwrap();
    assert_eq!(rec.kind, "kube-config");
    assert_eq!(
        store.env_credential(rec.id).await.unwrap(),
        Some(Credential::Kubeconfig(KUBECONFIG.into()))
    );
    assert_eq!(
        store.env_credential_summaries().await.unwrap()[&rec.id],
        summary
    );
    drop(store);

    // The file on disk never holds the plaintext.
    // Nor does the WAL beside it.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let raw = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"s3cret"));
    }

    // Another key cannot read it, and says so instead of failing.
    let other = Store::open(&db)
        .await
        .unwrap()
        .with_secret_key(&SecretKey::random());
    assert!(matches!(
        other.env_credential(rec.id).await.unwrap(),
        Some(Credential::Unreadable(_))
    ));
}

#[tokio::test]
async fn replacing_and_deleting_follow_the_environment() {
    let store = Store::in_memory().await.unwrap();
    let rec = store
        .create_environment_with_credential(
            "k",
            "kube-config",
            "https://a",
            "kubeconfig",
            "one",
            &json!({}),
            1,
        )
        .await
        .unwrap();
    store
        .put_env_credential(rec.id, "kubeconfig", "two", &json!({"v": 2}), 2)
        .await
        .unwrap();
    assert_eq!(
        store.env_credential(rec.id).await.unwrap(),
        Some(Credential::Kubeconfig("two".into()))
    );
    store.delete_environment(rec.id).await.unwrap();
    assert_eq!(store.env_credential(rec.id).await.unwrap(), None);
    assert!(store.env_credential_summaries().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_docker_environment_has_no_credential() {
    let store = Store::in_memory().await.unwrap();
    let rec = store
        .create_environment("d", "docker-local", "unix:///x.sock", 1)
        .await
        .unwrap();
    assert_eq!(store.env_credential(rec.id).await.unwrap(), None);
}
