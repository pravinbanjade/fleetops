use fleetops_store::secrets::SecretBox;
use fleetops_store::{RevisionFile, Store, STACK_REVISIONS_KEPT};

async fn setup() -> (Store, i64) {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_environment("local", "docker-local", "unix:///var/run/docker.sock", 1)
        .await
        .unwrap();
    (s, env.id)
}

fn vars(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const COMPOSE: &str = "services:\n  web:\n    image: nginx\n";

#[tokio::test]
async fn create_list_get_delete() {
    let (s, env) = setup().await;
    let rec = s
        .create_stack(env, "shop", COMPOSE, &vars(&["A=1"]), None, 10)
        .await
        .unwrap();
    assert_eq!(rec.name, "shop");
    assert_eq!(rec.source, "inline");
    assert_eq!(rec.deployed_revision, None);
    assert_eq!(s.list_stacks(env).await.unwrap(), vec![rec.clone()]);
    assert_eq!(s.stack_by_name(env, "shop").await.unwrap(), rec);
    let d = s.draft(rec.id).await.unwrap().unwrap();
    assert_eq!(
        (d.compose.as_str(), d.variables.clone()),
        (COMPOSE, vars(&["A=1"]))
    );
    s.delete_stack(rec.id).await.unwrap();
    assert!(s.list_stacks(env).await.unwrap().is_empty());
    assert!(s.stack_by_name(env, "shop").await.is_err());
    assert!(s.draft(rec.id).await.unwrap().is_none());
}

#[tokio::test]
async fn duplicate_name_conflict() {
    let (s, env) = setup().await;
    s.create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    let e = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap_err();
    assert!(matches!(e, fleetops_core::Error::Conflict(_)), "{e:?}");
}

#[tokio::test]
async fn same_name_other_env_ok() {
    let (s, env) = setup().await;
    let other = s
        .create_environment("b", "docker-local", "unix:///b.sock", 1)
        .await
        .unwrap();
    s.create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.create_stack(other.id, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
}

#[tokio::test]
async fn variables_round_trip_exactly() {
    let (s, env) = setup().await;
    let v = vars(&["P=a$b \"c\" ", "K=x=y"]);
    let rec = s
        .create_stack(env, "shop", COMPOSE, &v, None, 1)
        .await
        .unwrap();
    assert_eq!(s.draft(rec.id).await.unwrap().unwrap().variables, v);
    s.save_draft(rec.id, "services: {}", &vars(&["Z=1"]), None, 2)
        .await
        .unwrap();
    let d = s.draft(rec.id).await.unwrap().unwrap();
    assert_eq!(
        (d.compose.as_str(), d.variables, d.updated_at),
        ("services: {}", vars(&["Z=1"]), 2)
    );
    let r = s
        .create_revision(rec.id, COMPOSE, &v, "deploy", None, None, None, 3, &[])
        .await
        .unwrap();
    assert_eq!(s.revision_variables(rec.id, r.number).await.unwrap(), v);
}

#[tokio::test]
async fn revision_numbers_increment() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    let a = s
        .create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[])
        .await
        .unwrap();
    let b = s
        .create_revision(st.id, COMPOSE, &[], "rollback", Some(1), None, None, 2, &[])
        .await
        .unwrap();
    assert_eq!((a.number, b.number), (1, 2));
    assert_eq!(a.status, "deploying");
    assert_eq!((b.kind.as_str(), b.rollback_of), ("rollback", Some(1)));
    let list = s.list_revisions(st.id).await.unwrap();
    assert_eq!(
        list.iter().map(|r| r.number).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[tokio::test]
async fn finish_success_supersedes_previous() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 2, None).await.unwrap();
    assert_eq!(s.revision(st.id, 1).await.unwrap().status, "superseded");
    assert_eq!(s.revision(st.id, 2).await.unwrap().status, "deployed");
    assert_eq!(
        s.stack_by_name(env, "shop")
            .await
            .unwrap()
            .deployed_revision,
        Some(2)
    );
}

#[tokio::test]
async fn finish_failure_keeps_deployed() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 2, Some("boom")).await.unwrap();
    let r2 = s.revision(st.id, 2).await.unwrap();
    assert_eq!(
        (r2.status.as_str(), r2.error.as_deref()),
        ("failed", Some("boom"))
    );
    assert_eq!(s.revision(st.id, 1).await.unwrap().status, "deployed");
    assert_eq!(
        s.stack_by_name(env, "shop")
            .await
            .unwrap()
            .deployed_revision,
        Some(1)
    );
}

#[tokio::test]
async fn deploying_revisions_lists_only_deploying() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2, &[])
        .await
        .unwrap();
    let d = s.deploying_revisions().await.unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].0.name.as_str(), d[0].1.number), ("shop", 2));
}

#[tokio::test]
async fn prune_keeps_25_and_deployed() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[])
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    for n in 2..=30 {
        s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, n, &[])
            .await
            .unwrap();
        s.finish_revision(st.id, n, Some("x")).await.unwrap();
    }
    assert_eq!(STACK_REVISIONS_KEPT, 25);
    let deleted = s.prune_stack_revisions(STACK_REVISIONS_KEPT).await.unwrap();
    assert_eq!(deleted, 4);
    let left: Vec<i64> = s
        .list_revisions(st.id)
        .await
        .unwrap()
        .iter()
        .map(|r| r.number)
        .collect();
    assert_eq!(left.len(), 26);
    assert!(left.contains(&1));
    assert!(!left.contains(&2));
}

#[tokio::test]
async fn variables_sealed_at_rest() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &vars(&["TOKEN=hunter2"]), None, 1)
        .await
        .unwrap();
    let raw: String = sqlx::query_scalar("SELECT variables FROM stack_drafts WHERE stack_id = ?")
        .bind(st.id)
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert!(raw.starts_with("v1:"));
    assert!(!raw.contains("hunter2"));
}

#[tokio::test]
async fn stack_files_round_trip_sealed() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.set_stack_file(st.id, "secret", "db_password", "hunter2", None, 5)
        .await
        .unwrap();
    let files = s.stack_files(st.id).await.unwrap();
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(
        (
            f.kind.as_str(),
            f.source.as_str(),
            f.name.as_str(),
            f.value.as_str(),
            f.updated_at
        ),
        ("secret", "fleetops", "db_password", "hunter2", 5)
    );
    let (id, raw): (i64, String) = sqlx::query_as("SELECT id, value FROM stack_files")
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert!(raw.starts_with("v1:") && !raw.contains("hunter2"));
    let b = SecretBox::new(s.secret_key());
    assert!(b.open(format!("stack-file:{id}").as_bytes(), &raw).is_ok());
    assert!(b
        .open(format!("stack-file:{}", id + 1).as_bytes(), &raw)
        .is_err());
}

#[tokio::test]
async fn set_stack_file_upserts_and_remove_reports() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.set_stack_file(st.id, "secret", "pw", "a", None, 1)
        .await
        .unwrap();
    s.set_stack_file(st.id, "secret", "pw", "b", None, 2)
        .await
        .unwrap();
    s.set_stack_file(st.id, "config", "pw", "c", None, 3)
        .await
        .unwrap();
    let files = s.stack_files(st.id).await.unwrap();
    let vals: Vec<(&str, &str, i64)> = files
        .iter()
        .map(|f| (f.kind.as_str(), f.value.as_str(), f.updated_at))
        .collect();
    assert_eq!(vals, vec![("config", "c", 3), ("secret", "b", 2)]);
    assert!(s.remove_stack_file(st.id, "secret", "pw").await.unwrap());
    assert!(!s.remove_stack_file(st.id, "secret", "pw").await.unwrap());
    assert_eq!(s.stack_files(st.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn revision_snapshot_round_trip() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    let files = vec![
        RevisionFile {
            kind: "secret".into(),
            name: "pw".into(),
            value: "hunter2".into(),
        },
        RevisionFile {
            kind: "config".into(),
            name: "app".into(),
            value: "x=1".into(),
        },
    ];
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &files)
        .await
        .unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2, &[])
        .await
        .unwrap();
    let mut got = s.revision_files(st.id, 1).await.unwrap();
    got.sort_by(|a, b| a.kind.cmp(&b.kind));
    assert_eq!(got, vec![files[1].clone(), files[0].clone()]);
    assert!(s.revision_files(st.id, 2).await.unwrap().is_empty());
    let raw: Vec<String> = sqlx::query_scalar("SELECT value FROM stack_revision_files")
        .fetch_all(s.pool())
        .await
        .unwrap();
    assert!(raw
        .iter()
        .all(|r| r.starts_with("v1:") && !r.contains("hunter2")));
}

#[tokio::test]
async fn stack_files_cascade_on_delete() {
    let (s, env) = setup().await;
    let st = s
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    s.set_stack_file(st.id, "secret", "pw", "a", None, 1)
        .await
        .unwrap();
    let f = RevisionFile {
        kind: "secret".into(),
        name: "pw".into(),
        value: "a".into(),
    };
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1, &[f])
        .await
        .unwrap();
    s.delete_stack(st.id).await.unwrap();
    let n: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM stack_files), (SELECT COUNT(*) FROM stack_revision_files)",
    )
    .fetch_one(s.pool())
    .await
    .unwrap();
    assert_eq!(n, (0, 0));
}

#[tokio::test]
async fn file_digest_key_is_stable_and_differs_from_raw_key() {
    let (s, _) = setup().await;
    let k = s.file_digest_key();
    assert_eq!(k, s.file_digest_key());
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        s.secret_key().to_base64(),
    )
    .unwrap();
    assert_ne!(k.as_slice(), raw.as_slice());
    let other = Store::in_memory().await.unwrap();
    assert_ne!(k, other.file_digest_key());
}
