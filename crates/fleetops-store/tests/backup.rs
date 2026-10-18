use fleetops_store::{
    backup::{community_migration_max, community_sealed},
    secrets::{SecretBox, SecretKey},
    Store,
};
use serde_json::json;

const KUBECONFIG: &str = "apiVersion: v1\nusers: [{name: u, user: {token: s3cret}}]\n";

#[tokio::test]
async fn sealed_aad_matches_credentials() {
    let key = SecretKey::random();
    let store = Store::in_memory().await.unwrap().with_secret_key(&key);
    let rec = store
        .create_environment_with_credential(
            "k",
            "kube-config",
            "https://k:6443",
            "kubeconfig",
            KUBECONFIG,
            &json!({}),
            1,
        )
        .await
        .unwrap();
    let col = &community_sealed()[0];
    let sealed: String = sqlx::query_scalar(&format!(
        "SELECT {} FROM {} WHERE {} = ?",
        col.column, col.table, col.id_column
    ))
    .bind(rec.id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    let plain = SecretBox::new(&key)
        .open(&col.aad_for(rec.id), &sealed)
        .unwrap();
    assert_eq!(plain, KUBECONFIG.as_bytes());
}

#[test]
fn key_round_trips_base64() {
    let key = SecretKey::random();
    let back = SecretKey::from_base64(&key.to_base64()).unwrap();
    assert_eq!(back.to_base64(), key.to_base64());
}

#[test]
fn migration_max_is_latest() {
    assert!(community_migration_max() >= 6);
}
