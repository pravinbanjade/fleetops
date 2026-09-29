use fleetops_core::{Error, Role};
use fleetops_store::Store;

async fn store() -> Store {
    Store::in_memory().await.unwrap()
}

#[tokio::test]
async fn create_and_find_user_case_insensitively() {
    let s = store().await;
    let u = s
        .create_user("Alice", "hash1", Role::User, 100)
        .await
        .unwrap();
    assert_eq!(u.username, "Alice");
    assert_eq!(u.role, Role::User);
    assert_eq!(u.created_at, 100);

    let (found, hash) = s.find_credentials("alice").await.unwrap().unwrap();
    assert_eq!(found.id, u.id);
    assert_eq!(hash, "hash1");
    assert!(s.find_credentials("bob").await.unwrap().is_none());
}

#[tokio::test]
async fn duplicate_username_conflicts_case_insensitively() {
    let s = store().await;
    s.create_user("alice", "h", Role::User, 0).await.unwrap();
    let err = s
        .create_user("ALICE", "h", Role::User, 0)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn first_admin_only_once() {
    let s = store().await;
    let a = s.create_first_admin("admin", "h", 1).await.unwrap();
    assert_eq!(a.role, Role::Admin);
    let err = s.create_first_admin("other", "h", 1).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    assert_eq!(s.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn first_admin_refused_when_any_user_exists() {
    let s = store().await;
    s.create_user("bob", "h", Role::User, 0).await.unwrap();
    assert!(matches!(
        s.create_first_admin("admin", "h", 1).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn cannot_demote_or_delete_last_admin() {
    let s = store().await;
    let a = s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    assert!(matches!(
        s.set_user_role(a.id, Role::User, 1).await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(s.delete_user(a.id).await, Err(Error::Conflict(_))));

    let b = s.create_user("b", "h", Role::Admin, 0).await.unwrap();
    let demoted = s.set_user_role(a.id, Role::User, 2).await.unwrap();
    assert_eq!(demoted.role, Role::User);
    assert_eq!(demoted.updated_at, 2);
    assert!(matches!(s.delete_user(b.id).await, Err(Error::Conflict(_))));
    s.delete_user(a.id).await.unwrap();
    assert_eq!(s.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn missing_user_is_not_found() {
    let s = store().await;
    assert!(matches!(s.get_user(999).await, Err(Error::NotFound(_))));
    assert!(matches!(
        s.set_user_role(999, Role::User, 0).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.set_password_hash(999, "h", 0).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(s.delete_user(999).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn set_password_hash_updates_hash_and_timestamp() {
    let s = store().await;
    let u = s.create_user("alice", "old", Role::User, 0).await.unwrap();
    s.set_password_hash(u.id, "new", 50).await.unwrap();
    let (found, hash) = s.find_credentials("alice").await.unwrap().unwrap();
    assert_eq!(hash, "new");
    assert_eq!(found.updated_at, 50);
}

#[tokio::test]
async fn list_users_ordered_by_id() {
    let s = store().await;
    s.create_user("b", "h", Role::User, 0).await.unwrap();
    s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    let names: Vec<_> = s
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(names, vec!["b", "a"]);
}

#[tokio::test]
async fn global_role_binding_mirrors_role() {
    let s = store().await;
    let a = s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    let u = s.create_user("u", "h", Role::User, 0).await.unwrap();
    assert_eq!(s.global_role_binding(u.id).await.unwrap(), Some(Role::User));
    s.set_user_role(u.id, Role::Admin, 1).await.unwrap();
    assert_eq!(
        s.global_role_binding(u.id).await.unwrap(),
        Some(Role::Admin)
    );
    let first = s.create_first_admin("x", "h", 0).await;
    assert!(first.is_err());
    assert_eq!(
        s.global_role_binding(a.id).await.unwrap(),
        Some(Role::Admin)
    );
}
