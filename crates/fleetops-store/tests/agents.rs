use fleetops_store::Store;

const T0: i64 = 1_800_000_000;

#[tokio::test]
async fn an_agent_environment_is_created_with_a_pending_join_token() {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_agent_environment("web-01", "join-hash", T0 + 900, T0)
        .await
        .unwrap();
    assert_eq!(env.kind, "docker-agent");
    assert_eq!(env.url, format!("agent://{}", env.id));
    let a = s.get_agent(env.id).await.unwrap().unwrap();
    assert!(!a.enrolled);
    assert_eq!(a.join_expires_at, Some(T0 + 900));
}

#[tokio::test]
async fn join_tokens_expire_and_are_single_use() {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_agent_environment("web-01", "j1", T0 + 900, T0)
        .await
        .unwrap();
    assert_eq!(
        s.agent_by_join_token("j1", T0 + 899).await.unwrap(),
        Some(env.id)
    );
    assert_eq!(
        s.agent_by_join_token("j1", T0 + 900).await.unwrap(),
        None,
        "expired"
    );

    assert!(s.enroll_agent(env.id, "j1", "k1", T0 + 10).await.unwrap());
    assert!(
        !s.enroll_agent(env.id, "j1", "k2", T0 + 11).await.unwrap(),
        "second use"
    );
    assert_eq!(s.agent_by_join_token("j1", T0 + 12).await.unwrap(), None);
    assert_eq!(s.agent_by_key("k1").await.unwrap(), Some(env.id));
    assert_eq!(s.agent_by_key("k2").await.unwrap(), None);
    assert!(s.get_agent(env.id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn rotating_revokes_the_key_and_issues_a_new_join_token() {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_agent_environment("web-01", "j1", T0 + 900, T0)
        .await
        .unwrap();
    s.enroll_agent(env.id, "j1", "k1", T0).await.unwrap();
    s.rotate_agent(env.id, "j2", T0 + 1800).await.unwrap();
    assert_eq!(s.agent_by_key("k1").await.unwrap(), None);
    assert_eq!(
        s.agent_by_join_token("j2", T0 + 1000).await.unwrap(),
        Some(env.id)
    );
    assert!(!s.get_agent(env.id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn seen_records_version_host_and_errors_and_delete_cascades() {
    let s = Store::in_memory().await.unwrap();
    let env = s
        .create_agent_environment("web-01", "j1", T0 + 900, T0)
        .await
        .unwrap();
    s.record_agent_seen(
        env.id,
        T0 + 5,
        Some("0.2.0"),
        Some(r#"{"hostname":"web-01"}"#),
        None,
    )
    .await
    .unwrap();
    s.record_agent_seen(
        env.id,
        T0 + 6,
        None,
        None,
        Some("protocol 2; server needs 1"),
    )
    .await
    .unwrap();
    let a = s.get_agent(env.id).await.unwrap().unwrap();
    assert_eq!(a.last_seen_at, Some(T0 + 6));
    assert_eq!(
        a.agent_version.as_deref(),
        Some("0.2.0"),
        "None keeps the previous value"
    );
    assert_eq!(a.last_error.as_deref(), Some("protocol 2; server needs 1"));
    assert_eq!(s.list_agents().await.unwrap().len(), 1);

    s.delete_environment(env.id).await.unwrap();
    assert!(s.get_agent(env.id).await.unwrap().is_none());
}

#[tokio::test]
async fn agent_names_share_the_environment_namespace() {
    let s = Store::in_memory().await.unwrap();
    s.create_environment("web-01", "docker-local", "unix:///x.sock", T0)
        .await
        .unwrap();
    assert!(matches!(
        s.create_agent_environment("web-01", "j", T0 + 900, T0)
            .await,
        Err(fleetops_core::Error::Conflict(_))
    ));
}
