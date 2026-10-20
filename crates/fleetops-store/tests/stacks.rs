use fleetops_store::{Store, STACK_REVISIONS_KEPT};

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
        .create_revision(rec.id, COMPOSE, &v, "deploy", None, None, None, 3)
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
        .create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1)
        .await
        .unwrap();
    let b = s
        .create_revision(st.id, COMPOSE, &[], "rollback", Some(1), None, None, 2)
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
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1)
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2)
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
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1)
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2)
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
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1)
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 2)
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
    s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, 1)
        .await
        .unwrap();
    s.finish_revision(st.id, 1, None).await.unwrap();
    for n in 2..=30 {
        s.create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, n)
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
