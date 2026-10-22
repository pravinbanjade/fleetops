use fleetops_core::compose::files::*;
use fleetops_core::compose::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;

const KEY: [u8; 32] = [7; 32];

fn project(yaml: &str, vars: &[&str]) -> Project {
    let vars: Vec<String> = vars.iter().map(|v| v.to_string()).collect();
    parse(yaml, &vars, "shop").expect("valid").project
}

fn values(items: &[(FileKind, &str, &str)]) -> FileValues {
    items
        .iter()
        .map(|(k, n, v)| ((*k, n.to_string()), v.to_string()))
        .collect()
}

const YAML: &str = r#"
services:
  db:
    image: postgres
    secrets: [db_password, api_key]
    configs: [pg_conf]
  web:
    image: nginx
secrets:
  db_password: { file: ./pw.txt }
  api_key: { environment: API_KEY }
  unused: { file: ./nope }
configs:
  pg_conf: { content: "max_connections=10" }
"#;

#[test]
fn resolve_fills_digests_and_returns_contents() {
    let mut p = project(YAML, &[]);
    let vars = vec!["API_KEY=k3y".to_string()];
    let got = resolve(
        &mut p,
        &vars,
        &values(&[(FileKind::Secret, "db_password", "s3cr3t")]),
        &KEY,
    )
    .expect("resolves");
    assert_eq!(
        got,
        values(&[
            (FileKind::Secret, "db_password", "s3cr3t"),
            (FileKind::Secret, "api_key", "k3y"),
            (FileKind::Config, "pg_conf", "max_connections=10"),
        ])
    );
    let files = &p.services["db"].files;
    let pw = files.iter().find(|f| f.source == "db_password").unwrap();
    assert_eq!(
        pw.digest,
        digest(&KEY, FileKind::Secret, "db_password", "s3cr3t")
    );
    assert!(files.iter().all(|f| !f.digest.is_empty()));
}

#[test]
fn missing_value_is_an_issue_with_exact_copy() {
    let mut p = project(YAML, &[]);
    let vars = vec!["API_KEY=k".to_string()];
    let issues = resolve(&mut p, &vars, &FileValues::new(), &KEY).expect_err("missing");
    assert_eq!(
        issues,
        vec![ComposeIssue::new(
            "secrets.db_password",
            "secret db_password has no value: set it in the Secrets tab"
        )]
    );
    let mut p = project(
        "services:\n  web: { image: x, configs: [c] }\nconfigs:\n  c: { file: ./c.conf }\n",
        &[],
    );
    let issues = resolve(&mut p, &[], &FileValues::new(), &KEY).expect_err("missing");
    assert_eq!(
        issues[0].message,
        "config c has no value: set it in the Secrets tab"
    );
}

#[test]
fn unset_variable_is_an_issue() {
    let mut p = project(YAML, &[]);
    let issues = resolve(
        &mut p,
        &[],
        &values(&[(FileKind::Secret, "db_password", "x")]),
        &KEY,
    )
    .expect_err("unset");
    assert_eq!(
        issues,
        vec![ComposeIssue::new(
            "secrets.api_key",
            "secret api_key: variable API_KEY is not set"
        )]
    );
}

#[test]
fn limits() {
    let yaml = "services:\n  web: { image: x, secrets: [a] }\nsecrets:\n  a: { file: ./a }\n";
    let mut p = project(yaml, &[]);
    let big = "x".repeat(MAX_VALUE + 1);
    let issues =
        resolve(&mut p, &[], &values(&[(FileKind::Secret, "a", &big)]), &KEY).expect_err("too big");
    assert!(issues[0].message.contains("500 KiB"), "{issues:?}");

    let mut yaml = String::from("services:\n  web:\n    image: x\n    secrets: [");
    let names: Vec<String> = (0..MAX_FILES + 1).map(|i| format!("s{i}")).collect();
    yaml.push_str(&names.join(", "));
    yaml.push_str("]\nsecrets:\n");
    for n in &names {
        yaml.push_str(&format!("  {n}: {{ file: ./{n} }}\n"));
    }
    let value = "y".repeat(MAX_TOTAL / MAX_FILES + 1);
    let vals: FileValues = names
        .iter()
        .map(|n| ((FileKind::Secret, n.clone()), value.clone()))
        .collect();
    let mut p = project(&yaml, &[]);
    let issues = resolve(&mut p, &[], &vals, &KEY).expect_err("over limits");
    assert!(
        issues.iter().any(|i| i.message.contains("64 files")),
        "{issues:?}"
    );
    assert!(
        issues.iter().any(|i| i.message.contains("4 MiB")),
        "{issues:?}"
    );
}

#[test]
fn unused_declarations_are_not_required() {
    let mut p = project(
        "services:\n  web: { image: x }\nsecrets:\n  a: { file: ./a }\n  b: { environment: NOPE }\n",
        &[],
    );
    assert_eq!(
        resolve(&mut p, &[], &FileValues::new(), &KEY),
        Ok(FileValues::new())
    );
}

#[test]
fn digest_is_keyed() {
    let d = digest(&KEY, FileKind::Secret, "pw", "hunter2");
    assert_eq!(d.len(), 64);
    assert_ne!(d, digest(&[8; 32], FileKind::Secret, "pw", "hunter2"));
    assert_ne!(d, digest(&KEY, FileKind::Config, "pw", "hunter2"));
    assert_ne!(d, digest(&KEY, FileKind::Secret, "pw", "hunter3"));
    let plain: String = Sha256::digest(b"hunter2")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_ne!(d, plain);
}

#[test]
fn archive_has_only_file_entries_with_owner_and_mode() {
    let mut p = project(
        r#"
services:
  db:
    image: x
    secrets: [{ source: pw, uid: "999", gid: "998", mode: 0400 }]
    configs: [{ source: app, target: /etc/app/x.conf }]
secrets:
  pw: { file: ./pw }
configs:
  app: { content: "hello" }
"#,
        &[],
    );
    let contents = resolve(
        &mut p,
        &[],
        &values(&[(FileKind::Secret, "pw", "s3cr3t")]),
        &KEY,
    )
    .expect("resolves");
    let tar = archive(&p.services["db"], &contents);
    let mut a = tar::Archive::new(tar.as_slice());
    let mut seen = BTreeMap::new();
    for e in a.entries().unwrap() {
        let mut e = e.unwrap();
        let h = e.header().clone();
        assert!(h.entry_type().is_file(), "{:?}", h.entry_type());
        let path = e.path().unwrap().to_string_lossy().into_owned();
        let mut body = String::new();
        e.read_to_string(&mut body).unwrap();
        seen.insert(
            path,
            (h.uid().unwrap(), h.gid().unwrap(), h.mode().unwrap(), body),
        );
    }
    assert_eq!(
        seen,
        BTreeMap::from([
            (
                "run/secrets/pw".to_string(),
                (999, 998, 0o400, "s3cr3t".to_string())
            ),
            (
                "etc/app/x.conf".to_string(),
                (0, 0, 0o444, "hello".to_string())
            ),
        ])
    );
}

#[test]
fn secret_paths_reads_the_label() {
    let labels = BTreeMap::from([(
        LABEL_SECRETS.to_string(),
        r#"["/run/secrets/a","/x/b"]"#.to_string(),
    )]);
    assert_eq!(secret_paths(&labels), vec!["/run/secrets/a", "/x/b"]);
    assert!(secret_paths(&BTreeMap::new()).is_empty());
}

#[test]
fn repo_path_cases() {
    assert_eq!(
        repo_path("deploy/compose.yaml", "nginx.conf"),
        Ok("deploy/nginx.conf".into())
    );
    assert_eq!(
        repo_path("deploy/compose.yaml", "./conf/a.conf"),
        Ok("deploy/conf/a.conf".into())
    );
    assert_eq!(repo_path("deploy/compose.yaml", "../x"), Ok("x".into()));
    assert_eq!(repo_path("compose.yaml", "x"), Ok("x".into()));
    assert!(repo_path("deploy/compose.yaml", "../../x").is_err());
    assert!(repo_path("deploy/compose.yaml", "/etc/x").is_err());
    assert!(repo_path("compose.yaml", "conf/").is_err());
}
