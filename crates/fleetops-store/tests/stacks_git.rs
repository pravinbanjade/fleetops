use fleetops_store::{GitSettings, Store, TokenChange};

async fn setup() -> (Store, i64) {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_environment("local", "docker-local", "unix:///var/run/docker.sock", 1)
        .await
        .unwrap();
    let st = s
        .create_stack(env.id, "shop", "", &["A=1".to_string()], None, 1)
        .await
        .unwrap();
    (s, st.id)
}

fn settings(sync: &str) -> GitSettings {
    GitSettings {
        url: "https://git.example/shop.git".into(),
        branch: "main".into(),
        path: "compose.yaml".into(),
        sync: sync.into(),
        poll_seconds: 300,
        auto_deploy: false,
    }
}

#[tokio::test]
async fn set_get_round_trip() {
    let (s, id) = setup().await;
    let rec = s
        .set_stack_git(
            id,
            &settings("poll"),
            TokenChange::Set("tok"),
            Some("sec"),
            None,
            5,
        )
        .await
        .unwrap();
    assert_eq!(rec.url, "https://git.example/shop.git");
    assert_eq!(rec.sync, "poll");
    assert!(rec.has_token);
    assert!(!rec.paused);
    assert_eq!(rec.updated_at, 5);
    assert_eq!(s.stack_git(id).await.unwrap(), Some(rec));
    assert_eq!(s.stack_git_token(id).await.unwrap().as_deref(), Some("tok"));
    assert_eq!(
        s.stack_git_secret(id).await.unwrap().as_deref(),
        Some("sec")
    );
}

#[tokio::test]
async fn source_becomes_git_then_inline_on_detach() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    assert_eq!(s.stack_by_id(id).await.unwrap().source, "git");
    s.detach_stack_git(id).await.unwrap();
    assert_eq!(s.stack_by_id(id).await.unwrap().source, "inline");
    assert!(s.stack_git(id).await.unwrap().is_none());
    assert!(s.draft(id).await.unwrap().is_some());
}

#[tokio::test]
async fn first_insert_requires_secret() {
    let (s, id) = setup().await;
    assert!(s
        .set_stack_git(id, &settings("manual"), TokenChange::Keep, None, None, 1)
        .await
        .is_err());
    assert_eq!(s.stack_by_id(id).await.unwrap().source, "inline");
}

#[tokio::test]
async fn token_keep_clear_set() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Set("a"),
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.set_stack_git(id, &settings("manual"), TokenChange::Keep, None, None, 2)
        .await
        .unwrap();
    assert_eq!(s.stack_git_token(id).await.unwrap().as_deref(), Some("a"));
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Set("b"),
        None,
        None,
        3,
    )
    .await
    .unwrap();
    assert_eq!(s.stack_git_token(id).await.unwrap().as_deref(), Some("b"));
    let rec = s
        .set_stack_git(id, &settings("manual"), TokenChange::Clear, None, None, 4)
        .await
        .unwrap();
    assert!(!rec.has_token);
    assert_eq!(s.stack_git_token(id).await.unwrap(), None);
    // The secret is kept when it is not given again.
    assert_eq!(
        s.stack_git_secret(id).await.unwrap().as_deref(),
        Some("sec")
    );
}

#[tokio::test]
async fn token_and_secret_sealed_at_rest() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Set("plain-token"),
        Some("plain-secret"),
        None,
        1,
    )
    .await
    .unwrap();
    let (token, secret): (String, String) =
        sqlx::query_as("SELECT token, webhook_secret FROM stack_git WHERE stack_id = ?")
            .bind(id)
            .fetch_one(s.pool())
            .await
            .unwrap();
    for (raw, plain) in [(&token, "plain-token"), (&secret, "plain-secret")] {
        assert!(raw.starts_with("v1:"), "{raw}");
        assert!(!raw.contains(plain));
    }
}

#[tokio::test]
async fn changing_source_resets_sync_state() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.record_git_fetch(id, "c1", "services: {}\n", &[], 2)
        .await
        .unwrap();
    s.set_git_pending(id, Some("c1")).await.unwrap();
    s.record_git_error(id, "boom", 3).await.unwrap();
    let mut changed = settings("manual");
    changed.auto_deploy = true;
    let rec = s
        .set_stack_git(id, &changed, TokenChange::Keep, None, None, 4)
        .await
        .unwrap();
    assert_eq!(rec.last_fetched_commit.as_deref(), Some("c1"));
    assert_eq!(rec.pending_commit.as_deref(), Some("c1"));
    assert_eq!(rec.last_sync_error.as_deref(), Some("boom"));
    changed.branch = "other".into();
    let rec = s
        .set_stack_git(id, &changed, TokenChange::Keep, None, None, 5)
        .await
        .unwrap();
    assert_eq!(rec.last_fetched_commit, None);
    assert_eq!(rec.pending_commit, None);
    assert_eq!(rec.last_sync_error, None);
}

#[tokio::test]
async fn record_fetch_updates_draft_compose_keeps_variables() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.record_git_error(id, "earlier", 2).await.unwrap();
    s.record_git_fetch(id, "c1", "services: {}\n", &[], 7)
        .await
        .unwrap();
    let d = s.draft(id).await.unwrap().unwrap();
    assert_eq!(d.compose, "services: {}\n");
    assert_eq!(d.variables, vec!["A=1".to_string()]);
    let rec = s.stack_git(id).await.unwrap().unwrap();
    assert_eq!(rec.last_fetched_commit.as_deref(), Some("c1"));
    assert_eq!(rec.last_sync_at, Some(7));
    assert_eq!(rec.last_sync_error, None);
}

#[tokio::test]
async fn due_selection() {
    let (s, poll) = setup().await;
    let env = s.stack_by_id(poll).await.unwrap().env_id;
    let manual = s.create_stack(env, "m", "", &[], None, 1).await.unwrap().id;
    let hook = s.create_stack(env, "h", "", &[], None, 1).await.unwrap().id;
    s.set_stack_git(
        poll,
        &settings("poll"),
        TokenChange::Keep,
        Some("s"),
        None,
        1,
    )
    .await
    .unwrap();
    s.set_stack_git(
        manual,
        &settings("manual"),
        TokenChange::Keep,
        Some("s"),
        None,
        1,
    )
    .await
    .unwrap();
    s.set_stack_git(
        hook,
        &settings("webhook"),
        TokenChange::Keep,
        Some("s"),
        None,
        1,
    )
    .await
    .unwrap();
    let ids = |v: Vec<fleetops_store::StackRecord>| v.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(s.git_stacks_due(1000).await.unwrap()), vec![poll]);
    s.record_git_fetch(poll, "c", "", &[], 1000).await.unwrap();
    assert!(s.git_stacks_due(1299).await.unwrap().is_empty());
    assert_eq!(ids(s.git_stacks_due(1300).await.unwrap()), vec![poll]);
}

#[tokio::test]
async fn cascade_on_stack_delete() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.delete_stack(id).await.unwrap();
    assert!(s.stack_git(id).await.unwrap().is_none());
    assert!(s.stack_by_id(id).await.is_err());
}

#[tokio::test]
async fn fetch_after_detach_leaves_draft_alone() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.save_draft(id, "mine\n", &[], None, 2).await.unwrap();
    s.detach_stack_git(id).await.unwrap();
    assert!(!s
        .record_git_fetch(id, "c1", "theirs\n", &[], 3)
        .await
        .unwrap());
    assert_eq!(s.draft(id).await.unwrap().unwrap().compose, "mine\n");
}

#[tokio::test]
async fn git_draft_reads_file_and_commit_together() {
    let (s, id) = setup().await;
    assert_eq!(s.git_draft(id).await.unwrap().unwrap().1, None);
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.record_git_fetch(id, "c1", "one\n", &[], 2).await.unwrap();
    let (d, commit) = s.git_draft(id).await.unwrap().unwrap();
    assert_eq!(d.compose, "one\n");
    assert_eq!(d.variables, vec!["A=1".to_string()]);
    assert_eq!(commit.as_deref(), Some("c1"));
}

#[tokio::test]
async fn saving_variables_keeps_the_fetched_file() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.record_git_fetch(id, "c2", "new\n", &[], 2).await.unwrap();
    s.save_draft_variables(id, &["A=2".to_string()], None, 3)
        .await
        .unwrap();
    let d = s.draft(id).await.unwrap().unwrap();
    assert_eq!(d.compose, "new\n");
    assert_eq!(d.variables, vec!["A=2".to_string()]);
    assert_eq!(d.updated_at, 3);
}

fn configs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn record_git_fetch_replaces_repo_configs_only() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.set_stack_file(id, "config", "app", "mine", None, 1)
        .await
        .unwrap();
    s.record_git_fetch(id, "c1", "x", &configs(&[("app", "one"), ("old", "o")]), 2)
        .await
        .unwrap();
    s.record_git_fetch(id, "c2", "x", &configs(&[("app", "two")]), 3)
        .await
        .unwrap();
    let mut got: Vec<(String, String, String)> = s
        .stack_files(id)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.source, f.name, f.value))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("fleetops".into(), "app".into(), "mine".into()),
            ("repo".into(), "app".into(), "two".into()),
        ]
    );
}

#[tokio::test]
async fn detach_turns_repo_configs_into_fleetops_values() {
    let (s, id) = setup().await;
    s.set_stack_git(
        id,
        &settings("manual"),
        TokenChange::Keep,
        Some("sec"),
        None,
        1,
    )
    .await
    .unwrap();
    s.set_stack_file(id, "config", "app", "mine", None, 1)
        .await
        .unwrap();
    s.set_stack_file(id, "secret", "pw", "p", None, 1)
        .await
        .unwrap();
    s.record_git_fetch(
        id,
        "c1",
        "x",
        &configs(&[("app", "repo-app"), ("b", "bb")]),
        2,
    )
    .await
    .unwrap();
    s.detach_stack_git(id).await.unwrap();
    let mut got: Vec<(String, String, String, String)> = s
        .stack_files(id)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.kind, f.source, f.name, f.value))
        .collect();
    got.sort();
    let row = |k: &str, n: &str, v: &str| {
        (
            k.to_string(),
            "fleetops".to_string(),
            n.to_string(),
            v.to_string(),
        )
    };
    assert_eq!(
        got,
        vec![
            row("config", "app", "repo-app"),
            row("config", "b", "bb"),
            row("secret", "pw", "p")
        ]
    );
}
