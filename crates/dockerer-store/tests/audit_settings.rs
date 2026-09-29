use dockerer_core::Role;
use dockerer_store::{NewAudit, Store, AUDIT_RETENTION_KEY};
use serde_json::json;

fn entry(ts: i64, user_id: Option<i64>, action: &str) -> NewAudit<'_> {
    NewAudit {
        ts,
        user_id,
        environment_id: None,
        action,
        target_type: Some("user"),
        target_id: Some("7"),
        result: "success",
        detail: None,
    }
}

#[tokio::test]
async fn list_audit_is_newest_first_with_cursor_and_username() {
    let s = Store::in_memory().await.unwrap();
    let uid = s
        .create_user("alice", "h", Role::Admin, 0)
        .await
        .unwrap()
        .id;
    let id1 = s.insert_audit(&entry(1, Some(uid), "a.one")).await.unwrap();
    let id2 = s.insert_audit(&entry(2, None, "a.two")).await.unwrap();
    let id3 = s
        .insert_audit(&NewAudit {
            detail: Some(json!({"k": 1})),
            ..entry(3, Some(uid), "a.three")
        })
        .await
        .unwrap();

    let page = s.list_audit(2, None).await.unwrap();
    assert_eq!(
        page.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![id3, id2]
    );
    assert_eq!(page[0].username.as_deref(), Some("alice"));
    assert_eq!(page[0].detail, Some(json!({"k": 1})));
    assert_eq!(page[1].username, None);

    let next = s.list_audit(10, Some(id2)).await.unwrap();
    assert_eq!(next.iter().map(|e| e.id).collect::<Vec<_>>(), vec![id1]);
    assert_eq!(next[0].action, "a.one");
    assert_eq!(next[0].target_id.as_deref(), Some("7"));
}

#[tokio::test]
async fn prune_audit_removes_older_rows() {
    let s = Store::in_memory().await.unwrap();
    s.insert_audit(&entry(10, None, "old")).await.unwrap();
    s.insert_audit(&entry(20, None, "new")).await.unwrap();
    assert_eq!(s.prune_audit(15).await.unwrap(), 1);
    let left = s.list_audit(10, None).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].action, "new");
}

#[tokio::test]
async fn audit_survives_user_deletion() {
    let s = Store::in_memory().await.unwrap();
    s.create_user("admin", "h", Role::Admin, 0).await.unwrap();
    let uid = s.create_user("bob", "h", Role::User, 0).await.unwrap().id;
    s.insert_audit(&entry(1, Some(uid), "x")).await.unwrap();
    s.delete_user(uid).await.unwrap();
    let rows = s.list_audit(10, None).await.unwrap();
    assert_eq!(rows[0].user_id, None);
}

#[tokio::test]
async fn settings_roundtrip_and_retention_default() {
    let s = Store::in_memory().await.unwrap();
    assert_eq!(s.get_setting("x").await.unwrap(), None);
    s.set_setting("x", &json!({"a": 1})).await.unwrap();
    s.set_setting("x", &json!({"a": 2})).await.unwrap();
    assert_eq!(s.get_setting("x").await.unwrap(), Some(json!({"a": 2})));

    assert_eq!(s.audit_retention_days().await.unwrap(), 90);
    s.set_setting(AUDIT_RETENTION_KEY, &json!(30))
        .await
        .unwrap();
    assert_eq!(s.audit_retention_days().await.unwrap(), 30);
    s.set_setting(AUDIT_RETENTION_KEY, &json!("bogus"))
        .await
        .unwrap();
    assert_eq!(s.audit_retention_days().await.unwrap(), 90);
}
