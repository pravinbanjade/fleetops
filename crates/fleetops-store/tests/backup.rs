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
    let sealed: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM {} WHERE {} = ?",
        col.column, col.table, col.id_column
    )))
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

// ---- archives (Task 3) ----

use fleetops_store::backup::{format, open_backup, BackupMeta, KdfParams};
use fleetops_store::{NewAudit, NewSession};
use std::{fs::File, path::Path};

const PW: &str = "correct horse battery";

/// A file-backed store, as in production: `VACUUM INTO` needs one.
pub async fn populated(key: &SecretKey, dir: &Path) -> Store {
    let store = Store::open(&dir.join("live.db"))
        .await
        .unwrap()
        .with_secret_key(key);
    let u = store.create_first_admin("admin", "hash", 1).await.unwrap();
    store
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
    for i in 0..3 {
        store
            .insert_audit(&NewAudit {
                ts: i,
                user_id: Some(u.id),
                environment_id: None,
                action: "test",
                target_type: None,
                target_id: None,
                result: "success",
                detail: None,
            })
            .await
            .unwrap();
    }
    store
        .create_session(&NewSession {
            user_id: u.id,
            token_hash: "t",
            csrf_token: "c",
            now: 1,
            expires_at: i64::MAX,
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();
    store
}

pub fn meta() -> BackupMeta {
    BackupMeta {
        now: 1_700_000_000,
        edition: "community".into(),
        source: None,
        sealed: community_sealed(),
    }
}

pub async fn backup_file(store: &Store, dir: &Path) -> std::path::PathBuf {
    let out = dir.join("b.fob");
    store
        .write_backup(meta(), PW, KdfParams::TEST, &dir.join("scratch"), &out)
        .await
        .unwrap();
    out
}

#[tokio::test]
async fn backup_round_trip_keeps_data_drops_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(&SecretKey::random(), dir.path()).await;
    let file = backup_file(&store, dir.path()).await;
    let opened = open_backup(File::open(&file).unwrap(), PW, &dir.path().join("o")).unwrap();
    let c = &opened.manifest.counts;
    assert_eq!(c["users"], 1);
    assert_eq!(c["audit_log"], 3);
    assert!(!c.contains_key("sessions"));
    let restored = Store::open(&opened.db).await.unwrap();
    assert_eq!(restored.count_users().await.unwrap(), 1);
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(restored.pool())
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    // The scratch directory is left empty.
    assert_eq!(
        std::fs::read_dir(dir.path().join("scratch"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn manifest_carries_key_and_sealed() {
    let dir = tempfile::tempdir().unwrap();
    let key = SecretKey::random();
    let store = populated(&key, dir.path()).await;
    let file = backup_file(&store, dir.path()).await;
    let m = open_backup(File::open(&file).unwrap(), PW, &dir.path().join("o"))
        .unwrap()
        .manifest;
    assert_eq!(m.secret_key, key.to_base64());
    assert_eq!(m.sealed, community_sealed());
    assert!(m.community_migration >= 6);
    assert_eq!(m.enterprise_migration, None);
    assert!(!m.summary().has_enterprise_data);
}

#[tokio::test]
async fn ee_tables_come_along() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(&SecretKey::random(), dir.path()).await;
    for sql in [
        "CREATE TABLE ee_sso_states (state TEXT)",
        "INSERT INTO ee_sso_states VALUES ('s')",
        "CREATE TABLE ee_foo (id INTEGER)",
        "INSERT INTO ee_foo VALUES (1), (2)",
    ] {
        sqlx::query(sql).execute(store.pool()).await.unwrap();
    }
    let file = backup_file(&store, dir.path()).await;
    let c = open_backup(File::open(&file).unwrap(), PW, &dir.path().join("o"))
        .unwrap()
        .manifest
        .counts;
    assert_eq!(c["ee_foo"], 2);
    assert!(!c.contains_key("ee_sso_states"));
}

/// Seals a hand-built tar.gz the way a backup is sealed.
fn sealed_tar(dir: &Path, build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> std::path::PathBuf {
    let mut b = tar::Builder::new(Vec::new());
    build(&mut b);
    let raw = b.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &raw).unwrap();
    let tgz = gz.finish().unwrap();
    let out = dir.join(format!("x{}.fob", rand_suffix()));
    format::encrypt(
        tgz.as_slice(),
        File::create(&out).unwrap(),
        PW,
        KdfParams::TEST,
    )
    .unwrap();
    out
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn file_entry(b: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]) {
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o600);
    // Bypasses the builder's own path checks, as a hostile file would.
    let raw = &mut h.as_old_mut().name;
    raw[..name.len()].copy_from_slice(name.as_bytes());
    h.set_cksum();
    b.append(&h, data).unwrap();
}

fn manifest_json() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "format": 1, "fleetops_version": "0.2.0", "created_at": 1, "edition": "community",
        "source": null, "community_migration": 6, "enterprise_migration": null,
        "counts": {}, "secret_key": SecretKey::random().to_base64(), "sealed": []
    }))
    .unwrap()
}

type Build = Box<dyn FnOnce(&mut tar::Builder<Vec<u8>>)>;

#[test]
fn rejects_unexpected_tar_entries() {
    let dir = tempfile::tempdir().unwrap();
    let m = manifest_json();
    let cases: Vec<(&str, Build)> = vec![
        ("traversal", Box::new(|b| file_entry(b, "../x", b"x"))),
        (
            "absolute",
            Box::new(|b| file_entry(b, "/tmp/fleetops-x", b"x")),
        ),
        ("symlink", {
            let m = m.clone();
            Box::new(move |b| {
                file_entry(b, "manifest.json", &m);
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                b.append_link(&mut h, "fleetops.db", "/etc/passwd").unwrap();
            })
        }),
        ("manifest twice", {
            let m = m.clone();
            Box::new(move |b| {
                file_entry(b, "manifest.json", &m);
                file_entry(b, "manifest.json", &m);
            })
        }),
        ("extra file", {
            let m = m.clone();
            Box::new(move |b| {
                file_entry(b, "manifest.json", &m);
                file_entry(b, "fleetops.db", b"db");
                file_entry(b, "extra", b"x");
            })
        }),
        ("db before manifest", {
            let m = m.clone();
            Box::new(move |b| {
                file_entry(b, "fleetops.db", b"db");
                file_entry(b, "manifest.json", &m);
            })
        }),
        (
            "no db",
            Box::new(move |b| file_entry(b, "manifest.json", &m)),
        ),
    ];
    for (name, build) in cases {
        let file = sealed_tar(dir.path(), build);
        let out = dir.path().join(format!("o-{}", rand_suffix()));
        let e = open_backup(File::open(&file).unwrap(), PW, &out).unwrap_err();
        assert!(e.to_string().contains("damaged backup"), "{name}: {e}");
        for entry in std::fs::read_dir(&out).unwrap() {
            let p = entry.unwrap().path();
            assert!(p.starts_with(&out), "{name}: {p:?}");
        }
    }
    assert!(!dir.path().parent().unwrap().join("x").exists());
    assert!(!Path::new("/tmp/fleetops-x").exists());
}

#[test]
fn rejects_oversized_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b' '; 2 * 1024 * 1024];
    let file = sealed_tar(dir.path(), |b| file_entry(b, "manifest.json", &big));
    let e = open_backup(File::open(&file).unwrap(), PW, &dir.path().join("o")).unwrap_err();
    assert!(e.to_string().contains("damaged backup"), "{e}");
}

// ---- restore (Task 4) ----

use fleetops_core::env::Credential;
use fleetops_store::backup::{
    apply_pending, check_versions, mark_pending, reseal, Manifest, Pending, ServerVersions,
};

async fn opened_from(key: &SecretKey, dir: &Path) -> fleetops_store::backup::Opened {
    let store = populated(key, dir).await;
    let file = backup_file(&store, dir).await;
    open_backup(File::open(&file).unwrap(), PW, &dir.join("o")).unwrap()
}

#[tokio::test]
async fn restore_reseals_with_target_key() {
    let dir = tempfile::tempdir().unwrap();
    let (ka, kb) = (SecretKey::random(), SecretKey::random());
    let o = opened_from(&ka, dir.path()).await;
    assert_eq!(reseal(&o.db, &o.manifest, &kb).await.unwrap(), 1);
    let s = Store::open(&o.db).await.unwrap().with_secret_key(&kb);
    let env = s.list_environments().await.unwrap()[0].id;
    assert_eq!(
        s.env_credential(env).await.unwrap(),
        Some(Credential::Kubeconfig(KUBECONFIG.into()))
    );
    assert!(!dir.path().join("secret.key").exists());
}

#[tokio::test]
async fn reseal_fails_whole_on_bad_value() {
    let dir = tempfile::tempdir().unwrap();
    let o = opened_from(&SecretKey::random(), dir.path()).await;
    {
        let s = Store::open(&o.db).await.unwrap();
        sqlx::query("UPDATE env_credentials SET ciphertext = 'v1:AAAA'")
            .execute(s.pool())
            .await
            .unwrap();
    }
    let e = reseal(&o.db, &o.manifest, &SecretKey::random())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("could not be opened"), "{e}");
}

fn versions(ce: i64, ee: Option<i64>) -> ServerVersions {
    ServerVersions {
        community_max: ce,
        enterprise_max: ee,
        version: "0.2.0".into(),
    }
}

fn manifest(ce: i64, ee: Option<i64>) -> Manifest {
    serde_json::from_value(json!({
        "format": 1, "fleetops_version": "9.9.9", "created_at": 1, "edition": "community",
        "source": null, "community_migration": ce, "enterprise_migration": ee,
        "counts": {}, "secret_key": "", "sealed": []
    }))
    .unwrap()
}

#[test]
fn newer_backup_refused() {
    let e = check_versions(&manifest(99, None), &versions(6, None)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "invalid input: this backup is from FleetOps 9.9.9; upgrade to at least 9.9.9 to restore it"
    );
    assert!(check_versions(&manifest(6, Some(1_000_009)), &versions(6, Some(1_000_004))).is_err());
}

#[test]
fn older_backup_accepted() {
    assert!(check_versions(&manifest(3, Some(1_000_001)), &versions(6, Some(1_000_004))).is_ok());
}

#[test]
fn enterprise_unknown_on_community_is_accepted() {
    assert!(check_versions(&manifest(6, Some(1_000_099)), &versions(6, None)).is_ok());
}

async fn live_db(data: &Path, marker: &str) {
    let s = Store::open(&data.join("fleetops.db")).await.unwrap();
    s.create_first_admin(marker, "h", 1).await.unwrap();
    s.pool().close().await;
}

#[tokio::test]
async fn apply_pending_swaps_and_keeps_safety_copy() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    live_db(&data, "old").await;
    std::fs::write(data.join("secret.key"), "k").unwrap();
    let o = opened_from(&SecretKey::random(), dir.path()).await;
    mark_pending(&o.db, &data).await.unwrap();
    let Pending::Applied(safety) = apply_pending(&data).await.unwrap() else {
        panic!("not applied")
    };
    assert!(safety.join("fleetops.db").exists());
    assert!(safety.join("secret.key").exists());
    assert!(!data.join("restore-pending").exists());
    let s = Store::open(&data.join("fleetops.db")).await.unwrap();
    assert_eq!(s.list_users().await.unwrap()[0].username, "admin");
    assert_eq!(apply_pending(&data).await.unwrap(), Pending::None);
}

#[tokio::test]
async fn apply_pending_prunes_to_three() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    live_db(&data, "old").await;
    let o = opened_from(&SecretKey::random(), dir.path()).await;
    for _ in 0..4 {
        let copy = dir.path().join("copy.db");
        std::fs::copy(&o.db, &copy).unwrap();
        mark_pending(&copy, &data).await.unwrap();
        assert!(matches!(
            apply_pending(&data).await.unwrap(),
            Pending::Applied(_)
        ));
    }
    assert_eq!(
        std::fs::read_dir(data.join("pre-restore")).unwrap().count(),
        3
    );
}

#[tokio::test]
async fn apply_pending_keeps_live_db_when_pending_is_bad() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(data.join("restore-pending")).unwrap();
    live_db(&data, "old").await;
    std::fs::write(data.join("restore-pending/fleetops.db"), vec![9u8; 5000]).unwrap();
    let Pending::Rejected(moved) = apply_pending(&data).await.unwrap() else {
        panic!("not rejected")
    };
    assert!(moved
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("restore-failed-"));
    let s = Store::open(&data.join("fleetops.db")).await.unwrap();
    assert_eq!(s.list_users().await.unwrap()[0].username, "old");
}

#[tokio::test]
async fn apply_pending_prunes_rejected_to_three() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    live_db(&data, "old").await;
    for _ in 0..5 {
        std::fs::create_dir_all(data.join("restore-pending")).unwrap();
        std::fs::write(data.join("restore-pending/fleetops.db"), vec![9u8; 5000]).unwrap();
        assert!(matches!(
            apply_pending(&data).await.unwrap(),
            Pending::Rejected(_)
        ));
    }
    let failed = std::fs::read_dir(&data)
        .unwrap()
        .filter(|e| {
            let n = e.as_ref().unwrap().file_name();
            n.to_string_lossy().starts_with("restore-failed-")
        })
        .count();
    assert_eq!(failed, 3);
    let s = Store::open(&data.join("fleetops.db")).await.unwrap();
    assert_eq!(s.list_users().await.unwrap()[0].username, "old");
}

// ---- final-review fixes ----

#[tokio::test]
async fn reseal_rejects_hostile_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = opened_from(&SecretKey::random(), dir.path()).await;
    let target = dir.path().join("pwned.db");
    let evil = format!(
        "env_id\", 1 FROM env_credentials WHERE 0; COMMIT; VACUUM INTO '{}'; BEGIN; SELECT \"env_id",
        target.display()
    );
    for (t, c, id) in [
        ("env_credentials", "ciphertext", evil.as_str()),
        ("env_credentials", evil.as_str(), "env_id"),
        ("env_credentials", "nope", "env_id"),
    ] {
        o.manifest.sealed = vec![fleetops_store::backup::SealedColumn::new(t, c, id, "{id}")];
        let e = reseal(&o.db, &o.manifest, &SecretKey::random())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("damaged backup"), "{e}");
        assert!(!target.exists(), "the injected statement ran");
    }
}

#[tokio::test]
async fn backup_refuses_unreadable_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(&SecretKey::random(), dir.path()).await;
    sqlx::query("UPDATE env_credentials SET ciphertext = 'v1:AAAA'")
        .execute(store.pool())
        .await
        .unwrap();
    let e = store
        .write_backup(
            meta(),
            PW,
            KdfParams::TEST,
            &dir.path().join("scratch"),
            &dir.path().join("b.fob"),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("can't be decrypted"), "{e}");
    assert!(!dir.path().join("b.fob").exists());
}

#[tokio::test]
async fn apply_pending_sweeps_backup_tmp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("backup-tmp")).unwrap();
    std::fs::write(dir.path().join("backup-tmp/abandoned.upload"), "x").unwrap();
    apply_pending(dir.path()).await.unwrap();
    assert!(!dir.path().join("backup-tmp").exists());
}

#[tokio::test]
async fn open_backup_bounds_the_database_size() {
    let dir = tempfile::tempdir().unwrap();
    let store = populated(&SecretKey::random(), dir.path()).await;
    let file = backup_file(&store, dir.path()).await;
    let e = fleetops_store::backup::open_backup_limited(
        File::open(&file).unwrap(),
        PW,
        &dir.path().join("o"),
        1024,
    )
    .unwrap_err();
    assert!(e.to_string().contains("damaged backup"), "{e}");
}

#[test]
fn streaming_encryptor_matches_decrypt() {
    use std::io::Write;
    for len in [0usize, 1, 64 * 1024, 64 * 1024 + 1, 300_000] {
        let plain: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
        let mut out = Vec::new();
        let mut w = format::Encryptor::new(&mut out, PW, KdfParams::TEST).unwrap();
        for chunk in plain.chunks(7_000) {
            w.write_all(chunk).unwrap();
        }
        w.finish().unwrap();
        let mut back = Vec::new();
        format::decrypt(out.as_slice(), &mut back, PW).unwrap();
        assert_eq!(back, plain, "len {len}");
    }
}

#[tokio::test]
async fn restore_reseals_stack_variables() {
    let dir = tempfile::tempdir().unwrap();
    let (ka, kb) = (SecretKey::random(), SecretKey::random());
    let store = populated(&ka, dir.path()).await;
    let env = store.list_environments().await.unwrap()[0].id;
    let vars = vec!["TOKEN=s3cret".to_string()];
    let st = store
        .create_stack(env, "shop", "services: {}", &vars, None, 1)
        .await
        .unwrap();
    store
        .create_revision(st.id, "services: {}", &vars, "deploy", None, None, None, 1)
        .await
        .unwrap();
    let file = backup_file(&store, dir.path()).await;
    let o = open_backup(File::open(&file).unwrap(), PW, &dir.path().join("o")).unwrap();
    assert_eq!(reseal(&o.db, &o.manifest, &kb).await.unwrap(), 3);
    let s = Store::open(&o.db).await.unwrap().with_secret_key(&kb);
    let st = s.stack_by_name(env, "shop").await.unwrap();
    assert_eq!(s.draft(st.id).await.unwrap().unwrap().variables, vars);
    assert_eq!(s.revision_variables(st.id, 1).await.unwrap(), vars);
}
