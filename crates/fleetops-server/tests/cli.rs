//! Command-line behaviour of the `fleetops` binary.

use std::process::Command;

#[test]
fn openapi_flag_prints_the_document_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("never-created");
    let out = Command::new(env!("CARGO_BIN_EXE_fleetops"))
        .arg("--openapi")
        .env("FLEETOPS_DATA", &data)
        .env("FLEETOPS_BIND", "127.0.0.1:1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).unwrap();
    assert_eq!(printed.trim_end(), fleetops_server::openapi_json());
    let doc: serde_json::Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(
        !data.exists(),
        "--openapi must not touch the data directory"
    );
}

#[cfg(feature = "enterprise")]
#[test]
fn enterprise_binary_documents_its_routes_and_takes_a_license_file() {
    let doc: serde_json::Value = serde_json::from_str(&fleetops_server::openapi_json()).unwrap();
    assert!(doc["paths"]["/api/v1/ee/license"].is_object());
    let out = Command::new(env!("CARGO_BIN_EXE_fleetops"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--license-file"));
}

#[cfg(not(feature = "enterprise"))]
#[test]
fn community_binary_has_no_license_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_fleetops"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("--license-file"));
    let doc: serde_json::Value = serde_json::from_str(&fleetops_server::openapi_json()).unwrap();
    assert!(doc["paths"]["/api/v1/ee/license"].is_null());
}

#[test]
fn helm_discard_argument_reads_stdin_and_prints_nothing() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("never-created");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetops"))
        .arg(fleetops_kube::helm::render::DISCARD_ARG)
        .env("FLEETOPS_DATA", &data)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // More than a pipe buffer, so a program that never reads would block helm.
    let manifest = "apiVersion: v1\nkind: ConfigMap\n---\n".repeat(20_000);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(manifest.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert!(!data.exists(), "the discard must not start the server");
}
