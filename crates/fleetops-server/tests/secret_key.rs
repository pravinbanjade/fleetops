use clap::Parser;
use fleetops_server::config::Config;

#[tokio::test]
async fn the_key_file_is_created_once_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from(["fleetops", "--data", dir.path().to_str().unwrap()]);
    let a = fleetops_server::open_store(&config).await.unwrap();
    let rec = a
        .create_environment_with_credential(
            "k",
            "kube-config",
            "https://k",
            "kubeconfig",
            "tok",
            &serde_json::json!({}),
            1,
        )
        .await
        .unwrap();
    drop(a);
    assert!(dir.path().join("secret.key").exists());
    let b = fleetops_server::open_store(&config).await.unwrap();
    assert_eq!(
        b.env_credential(rec.id).await.unwrap(),
        Some(fleetops_core::env::Credential::Kubeconfig("tok".into()))
    );
}

#[tokio::test]
async fn a_malformed_secret_key_stops_startup() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from([
        "fleetops",
        "--data",
        dir.path().to_str().unwrap(),
        "--secret-key",
        "short",
    ]);
    let Err(e) = fleetops_server::open_store(&config).await else {
        panic!("a malformed key must stop startup")
    };
    let e = e.to_string();
    assert!(e.contains("FLEETOPS_SECRET_KEY must be 32 bytes"), "{e}");
}
