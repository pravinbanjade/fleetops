use fleetops_store::Store;

#[tokio::test]
async fn community_store_opens_a_database_enterprise_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fleetops.db");
    let store = Store::open(&path).await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    fleetops_ee::db::put(store.pool(), "x.y", 1, 1)
        .await
        .unwrap();
    drop(store);
    // A Community binary: its migrator meets version 1000000 it does not know.
    let again = Store::open(&path).await.unwrap();
    // And Enterprise again, meeting the Community versions.
    fleetops_ee::db::migrate(again.pool()).await.unwrap();
    assert_eq!(
        fleetops_ee::db::get(again.pool()).await.unwrap().as_deref(),
        Some("x.y")
    );
}

#[tokio::test]
async fn sso_tables_exist_and_members_carry_a_source() {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    for table in [
        "ee_sso_connections",
        "ee_sso_identities",
        "ee_sso_mappings",
        "ee_sso_states",
    ] {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = ?")
            .bind(table)
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }
    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('ee_team_members')")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert!(
        cols.contains(&"source_connection_id".to_string()),
        "{cols:?}"
    );
}

#[tokio::test]
async fn stack_verb_migration() {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    for (name, perms) in [
        ("ops", r#"["stacks.view","stacks.operate"]"#),
        ("peek", r#"["containers.view","containers.reveal"]"#),
        ("none", r#"["images.view"]"#),
        (
            "both",
            r#"["stacks.operate","stacks.create","containers.reveal","stacks.reveal"]"#,
        ),
    ] {
        sqlx::query(
            "INSERT INTO ee_roles (name, permissions, created_at, updated_at) VALUES (?, ?, 1, 1)",
        )
        .bind(name)
        .bind(perms)
        .execute(store.pool())
        .await
        .unwrap();
    }
    // Re-run the migration's statements over roles that predate it.
    sqlx::raw_sql(include_str!("../migrations/1000005_stack_verbs.sql"))
        .execute(store.pool())
        .await
        .unwrap();
    let get = |name: &'static str| {
        let pool = store.pool().clone();
        async move {
            let s: String = sqlx::query_scalar("SELECT permissions FROM ee_roles WHERE name = ?")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
            let mut v: Vec<String> = serde_json::from_str(&s).unwrap();
            v.sort();
            v
        }
    };
    assert_eq!(
        get("ops").await,
        ["stacks.create", "stacks.operate", "stacks.view"]
    );
    assert_eq!(
        get("peek").await,
        ["containers.reveal", "containers.view", "stacks.reveal"]
    );
    assert_eq!(get("none").await, ["images.view"]);
    assert_eq!(
        get("both").await,
        [
            "containers.reveal",
            "stacks.create",
            "stacks.operate",
            "stacks.reveal"
        ]
    );
}
