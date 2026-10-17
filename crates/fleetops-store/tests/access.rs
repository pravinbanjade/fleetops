use fleetops_core::access::{EnvRole, Grant};
use fleetops_core::{Error, Role};
use fleetops_store::Store;
use std::collections::BTreeSet;

async fn store() -> Store {
    Store::in_memory().await.unwrap()
}

fn set(ns: &[&str]) -> BTreeSet<String> {
    ns.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn a_user_has_no_grant_until_one_is_set() {
    let s = store().await;
    let u = s.create_user("bob", "h", Role::User, 0).await.unwrap();
    let e = s
        .create_environment("k", "kube-config", "kube://k", 0)
        .await
        .unwrap();
    assert_eq!(s.env_grant(u.id, e.id).await.unwrap(), None);
    s.set_binding(u.id, e.id, EnvRole::Viewer, Some(&set(&["a"])), 5)
        .await
        .unwrap();
    assert_eq!(
        s.env_grant(u.id, e.id).await.unwrap(),
        Some(Grant::builtin(EnvRole::Viewer, Some(set(&["a"]))))
    );
    // Upsert replaces role and namespaces.
    s.set_binding(u.id, e.id, EnvRole::Operator, None, 6)
        .await
        .unwrap();
    assert_eq!(
        s.env_grant(u.id, e.id).await.unwrap(),
        Some(Grant::full(EnvRole::Operator))
    );
    assert!(s.delete_binding(u.id, e.id).await.unwrap());
    assert!(!s.delete_binding(u.id, e.id).await.unwrap());
    assert_eq!(s.env_grant(u.id, e.id).await.unwrap(), None);
}

#[tokio::test]
async fn admins_have_full_access_everywhere_and_their_bindings_return_on_demotion() {
    let s = store().await;
    let a = s.create_user("root", "h", Role::Admin, 0).await.unwrap();
    s.create_user("root2", "h", Role::Admin, 0).await.unwrap();
    let e1 = s
        .create_environment("d", "docker-local", "unix:///d", 0)
        .await
        .unwrap();
    let e2 = s
        .create_environment("k", "kube-config", "kube://k", 0)
        .await
        .unwrap();
    assert_eq!(
        s.env_grant(a.id, e1.id).await.unwrap(),
        Some(Grant::admin())
    );
    assert_eq!(s.user_grants(a.id).await.unwrap().len(), 2);
    s.set_binding(a.id, e2.id, EnvRole::Viewer, None, 1)
        .await
        .unwrap();
    s.set_user_role(a.id, Role::User, 2).await.unwrap();
    let g = s.user_grants(a.id).await.unwrap();
    assert_eq!(g.len(), 1);
    assert_eq!(g[&e2.id], Grant::full(EnvRole::Viewer));
    assert_eq!(s.env_grant(a.id, e1.id).await.unwrap(), None);
}

#[tokio::test]
async fn namespaces_are_refused_on_docker_environments() {
    let s = store().await;
    let u = s.create_user("bob", "h", Role::User, 0).await.unwrap();
    let d = s
        .create_environment("d", "docker-local", "unix:///d", 0)
        .await
        .unwrap();
    let e = s
        .set_binding(u.id, d.id, EnvRole::Viewer, Some(&set(&["a"])), 0)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("Kubernetes")),
        "{e}"
    );
}

#[tokio::test]
async fn unknown_users_and_environments_are_not_found() {
    let s = store().await;
    let u = s.create_user("bob", "h", Role::User, 0).await.unwrap();
    let d = s
        .create_environment("d", "docker-local", "unix:///d", 0)
        .await
        .unwrap();
    assert!(matches!(
        s.set_binding(999, d.id, EnvRole::Viewer, None, 0).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.set_binding(u.id, 999, EnvRole::Viewer, None, 0).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn bindings_go_with_their_user_or_environment() {
    let s = store().await;
    let u = s.create_user("bob", "h", Role::User, 0).await.unwrap();
    let v = s.create_user("carol", "h", Role::User, 0).await.unwrap();
    let d = s
        .create_environment("d", "docker-local", "unix:///d", 0)
        .await
        .unwrap();
    let k = s
        .create_environment("k", "kube-config", "kube://k", 0)
        .await
        .unwrap();
    s.set_binding(u.id, d.id, EnvRole::Viewer, None, 0)
        .await
        .unwrap();
    s.set_binding(u.id, k.id, EnvRole::Operator, Some(&set(&["a"])), 0)
        .await
        .unwrap();
    s.set_binding(v.id, k.id, EnvRole::Viewer, None, 0)
        .await
        .unwrap();
    let mine = s.user_bindings(u.id).await.unwrap();
    assert_eq!(
        mine.iter().map(|b| b.env_name.as_str()).collect::<Vec<_>>(),
        ["d", "k"]
    );
    assert_eq!(mine[1].env_kind, "kube-config");
    let on_k = s.env_bindings(k.id).await.unwrap();
    assert_eq!(
        on_k.iter().map(|b| b.username.as_str()).collect::<Vec<_>>(),
        ["bob", "carol"]
    );
    s.delete_environment(k.id).await.unwrap();
    assert_eq!(s.user_bindings(u.id).await.unwrap().len(), 1);
    s.delete_user(u.id).await.unwrap();
    assert!(s.env_bindings(d.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn upgrading_gives_existing_users_operator_on_existing_environments() {
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::borrow::Cow;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new().connect_with(opts).await.unwrap();
        let full = sqlx::migrate!("./migrations");
        let old = Migrator {
            migrations: Cow::Owned(
                full.migrations
                    .iter()
                    .filter(|m| m.version <= 4)
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        old.run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO users (id, username, password_hash, role, created_at, updated_at) VALUES
               (1, 'root', 'h', 'admin', 0, 0), (2, 'bob', 'h', 'user', 0, 0);
             INSERT INTO role_bindings (user_id, environment_id, role) VALUES (1, NULL, 'admin'), (2, NULL, 'user');
             INSERT INTO environments (id, name, kind, url, created_at, updated_at) VALUES
               (10, 'local', 'docker-local', 'unix:///d', 0, 0), (11, 'k', 'kube-config', 'kube://k', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }
    let s = Store::open(&path).await.unwrap();
    let bob = s.user_grants(2).await.unwrap();
    assert_eq!(bob.len(), 2);
    assert!(bob.values().all(|g| *g == Grant::full(EnvRole::Operator)));
    assert!(
        s.user_bindings(1).await.unwrap().is_empty(),
        "admins get no rows"
    );
    // A new environment is private to admins.
    let e = s
        .create_environment("new", "docker-local", "unix:///n", 1)
        .await
        .unwrap();
    assert_eq!(s.env_grant(2, e.id).await.unwrap(), None);
}
