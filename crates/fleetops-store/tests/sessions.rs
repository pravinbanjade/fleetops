use fleetops_core::Role;
use fleetops_store::{NewSession, SessionRecord, Store};

async fn setup() -> (Store, i64) {
    let s = Store::in_memory().await.unwrap();
    let id = s.create_user("alice", "h", Role::User, 0).await.unwrap().id;
    (s, id)
}

fn new(user_id: i64, hash: &str, now: i64) -> NewSession<'_> {
    NewSession {
        user_id,
        token_hash: hash,
        csrf_token: "csrf",
        now,
        expires_at: now + 1000,
        user_agent: Some("ua"),
        ip: Some("1.2.3.4"),
    }
}

#[tokio::test]
async fn create_find_touch() {
    let (s, uid) = setup().await;
    let id = s.create_session(&new(uid, "h1", 10)).await.unwrap();
    let rec = s.find_session("h1").await.unwrap().unwrap();
    assert_eq!(
        rec,
        SessionRecord {
            id,
            user_id: uid,
            csrf_token: "csrf".into(),
            created_at: 10,
            expires_at: 1010,
            last_seen_at: 10,
            user_agent: Some("ua".into()),
            ip: Some("1.2.3.4".into()),
        }
    );
    assert!(s.find_session("nope").await.unwrap().is_none());
    s.touch_session(id, 50).await.unwrap();
    assert_eq!(
        s.find_session("h1").await.unwrap().unwrap().last_seen_at,
        50
    );
}

#[test]
fn is_active_respects_absolute_and_idle_expiry() {
    let rec = SessionRecord {
        id: 1,
        user_id: 1,
        csrf_token: String::new(),
        created_at: 0,
        expires_at: 1000,
        last_seen_at: 0,
        user_agent: None,
        ip: None,
    };
    assert!(rec.is_active(10, 100));
    assert!(
        !rec.is_active(100, 100),
        "idle exactly at timeout is expired"
    );
    assert!(!rec.is_active(1000, 5000), "absolute expiry");
}

#[tokio::test]
async fn delete_user_session_is_scoped_to_owner() {
    let (s, a) = setup().await;
    let b = s.create_user("bob", "h", Role::User, 0).await.unwrap().id;
    let sid = s.create_session(&new(a, "ha", 0)).await.unwrap();
    assert!(!s.delete_user_session(b, sid).await.unwrap());
    assert!(s.delete_user_session(a, sid).await.unwrap());
    assert!(s.find_session("ha").await.unwrap().is_none());
}

#[tokio::test]
async fn delete_user_sessions_except_current() {
    let (s, uid) = setup().await;
    let keep = s.create_session(&new(uid, "h1", 0)).await.unwrap();
    s.create_session(&new(uid, "h2", 0)).await.unwrap();
    s.create_session(&new(uid, "h3", 0)).await.unwrap();
    assert_eq!(s.delete_user_sessions(uid, Some(keep)).await.unwrap(), 2);
    let left: Vec<_> = s
        .list_sessions(uid)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(left, vec![keep]);
    assert_eq!(s.delete_user_sessions(uid, None).await.unwrap(), 1);
}

#[tokio::test]
async fn delete_expired_sessions_removes_absolute_and_idle() {
    let (s, uid) = setup().await;
    s.create_session(&NewSession {
        expires_at: 1000,
        ..new(uid, "expired", 0)
    })
    .await
    .unwrap();
    let idle = s
        .create_session(&NewSession {
            expires_at: 9000,
            ..new(uid, "idle", 0)
        })
        .await
        .unwrap();
    let fresh = s
        .create_session(&NewSession {
            expires_at: 9000,
            ..new(uid, "fresh", 0)
        })
        .await
        .unwrap();
    s.touch_session(idle, 400).await.unwrap();
    s.touch_session(fresh, 900).await.unwrap();
    assert_eq!(s.delete_expired_sessions(1000, 500).await.unwrap(), 2);
    assert!(s.find_session("fresh").await.unwrap().is_some());
}

#[tokio::test]
async fn deleting_user_cascades_sessions() {
    let (s, uid) = setup().await;
    s.create_session(&new(uid, "h1", 0)).await.unwrap();
    s.delete_user(uid).await.unwrap();
    assert!(s.find_session("h1").await.unwrap().is_none());
}
