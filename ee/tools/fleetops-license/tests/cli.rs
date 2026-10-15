use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fleetops-license"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn keygen_sign_inspect_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    assert!(run(&["keygen", "--out", d]).status.success());
    assert!(
        !run(&["keygen", "--out", d]).status.success(),
        "keygen must not overwrite a key"
    );
    let key = format!("{d}/license.key");
    let public = format!("{d}/license.pub");
    let out = run(&[
        "sign",
        "--key",
        &key,
        "--customer",
        "Acme",
        "--expires",
        "2099-01-01",
        "--max-users",
        "5",
        "--features",
        "audit_export",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lic = format!("{d}/acme.lic");
    std::fs::write(&lic, &out.stdout).unwrap();
    let out = run(&["inspect", &lic, "--pub", &public]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("\"customer\": \"Acme\"") && text.contains("status: valid"),
        "{text}"
    );
    // Wrong key: rejected.
    let other = tempfile::tempdir().unwrap();
    run(&["keygen", "--out", other.path().to_str().unwrap()]);
    let wrong = format!("{}/license.pub", other.path().display());
    assert!(!run(&["inspect", &lic, "--pub", &wrong]).status.success());
    // Unknown feature: refused at signing.
    assert!(!run(&[
        "sign",
        "--key",
        &key,
        "--customer",
        "A",
        "--expires",
        "2099-01-01",
        "--features",
        "audit_exprot",
    ])
    .status
    .success());
}
