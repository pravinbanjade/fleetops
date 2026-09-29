use fleetops_core::Role;
use fleetops_server::maintenance::prune_once;
use fleetops_store::{NewAudit, NewSession, Store};

#[tokio::test]
async fn prunes_expired_sessions_and_old_audit() {
    let s = Store::in_memory().await.unwrap();
    let uid = s.create_user("a", "h", Role::Admin, 0).await.unwrap().id;
    let now = 100 * 86_400;
    let base = NewSession {
        user_id: uid,
        token_hash: "old",
        csrf_token: "c",
        now: now - 10,
        expires_at: now - 1,
        user_agent: None,
        ip: None,
    };
    s.create_session(&base).await.unwrap();
    s.create_session(&NewSession {
        token_hash: "live",
        expires_at: now + 1000,
        ..base
    })
    .await
    .unwrap();
    let audit = |ts| NewAudit {
        ts,
        user_id: None,
        environment_id: None,
        action: "x",
        target_type: None,
        target_id: None,
        result: "success",
        detail: None,
    };
    s.insert_audit(&audit(now - 91 * 86_400)).await.unwrap();
    s.insert_audit(&audit(now - 89 * 86_400)).await.unwrap();

    assert_eq!(prune_once(&s, now).await.unwrap(), (1, 1));
    assert!(s.find_session("live").await.unwrap().is_some());
    assert_eq!(s.list_audit(10, None).await.unwrap().len(), 1);
}
