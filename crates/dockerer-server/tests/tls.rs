use clap::Parser;
use dockerer_server::{config::Config, tls};

#[test]
fn generates_cert_and_private_key() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("tls/cert.pem");
    let key = dir.path().join("tls/key.pem");
    tls::generate_self_signed(&cert, &key).unwrap();
    assert!(std::fs::read_to_string(&cert)
        .unwrap()
        .contains("BEGIN CERTIFICATE"));
    assert!(std::fs::read_to_string(&key)
        .unwrap()
        .contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[tokio::test]
async fn load_or_generate_reuses_existing_self_signed_cert() {
    tls::install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from(["dockerer", "--data", dir.path().to_str().unwrap()]);
    tls::load_or_generate(&config).await.unwrap();
    let first = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
    tls::load_or_generate(&config).await.unwrap();
    let second = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn explicit_cert_paths_must_exist() {
    tls::install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from([
        "dockerer",
        "--data",
        dir.path().to_str().unwrap(),
        "--tls-cert",
        "/nonexistent/cert.pem",
        "--tls-key",
        "/nonexistent/key.pem",
    ]);
    assert!(tls::load_or_generate(&config).await.is_err());
}

#[test]
fn cert_without_key_is_rejected_by_cli() {
    assert!(Config::try_parse_from(["dockerer", "--tls-cert", "c.pem"]).is_err());
}
