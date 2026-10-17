use fleetops_core::access::{EnvRole, ExtraGrants, Grant};
use fleetops_core::{Error, Role};
use fleetops_ee::teams::{db, grants::TeamGrants, validate_name};
use fleetops_store::Store;
use std::collections::BTreeSet;

struct Fx {
    store: Store,
    bob: i64,
    carol: i64,
    env: i64,
    env2: i64,
}

async fn fx() -> Fx {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    let bob = store
        .create_user("bob", "x", Role::User, 0)
        .await
        .unwrap()
        .id;
    let carol = store
        .create_user("carol", "x", Role::User, 0)
        .await
        .unwrap()
        .id;
    let env = store
        .create_environment("e", "docker-local", "unix:///x", 0)
        .await
        .unwrap()
        .id;
    let env2 = store
        .create_environment("k", "kube-config", "", 0)
        .await
        .unwrap()
        .id;
    Fx {
        store,
        bob,
        carol,
        env,
        env2,
    }
}

fn ns(n: &[&str]) -> BTreeSet<String> {
    n.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn team_grants_reach_members_only() {
    let f = fx().await;
    let p = f.store.pool();
    let t = db::create(p, "ops", None, 0).await.unwrap();
    db::add_member(p, t.id, f.bob, 0).await.unwrap();
    db::set_binding(p, t.id, f.env, EnvRole::Viewer, None, 0)
        .await
        .unwrap();
    let g = TeamGrants::new(p.clone());
    assert_eq!(
        g.env_grants(f.bob, f.env).await.unwrap(),
        vec![Grant::full(EnvRole::Viewer)]
    );
    assert!(g.env_grants(f.carol, f.env).await.unwrap().is_empty());
    assert!(g.env_grants(f.bob, f.env2).await.unwrap().is_empty());
    let all = g.user_grants(f.bob).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[&f.env], vec![Grant::full(EnvRole::Viewer)]);
}

#[tokio::test]
async fn two_teams_both_contribute() {
    let f = fx().await;
    let p = f.store.pool();
    let a = db::create(p, "a", None, 0).await.unwrap();
    let b = db::create(p, "b", None, 0).await.unwrap();
    for t in [&a, &b] {
        db::add_member(p, t.id, f.bob, 0).await.unwrap();
    }
    db::set_binding(p, a.id, f.env2, EnvRole::Viewer, Some(&ns(&["x"])), 0)
        .await
        .unwrap();
    db::set_binding(p, b.id, f.env2, EnvRole::Operator, Some(&ns(&["y"])), 0)
        .await
        .unwrap();
    let mut got = TeamGrants::new(p.clone())
        .env_grants(f.bob, f.env2)
        .await
        .unwrap();
    got.sort_by_key(|g| g.role);
    assert_eq!(
        got,
        vec![
            Grant {
                role: EnvRole::Viewer,
                namespaces: Some(ns(&["x"]))
            },
            Grant {
                role: EnvRole::Operator,
                namespaces: Some(ns(&["y"]))
            },
        ]
    );
}

#[tokio::test]
async fn names_are_unique_ignoring_case() {
    let f = fx().await;
    db::create(f.store.pool(), "Payments", None, 0)
        .await
        .unwrap();
    let err = db::create(f.store.pool(), "payments", None, 0)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
}

#[test]
fn validate_name_trims_and_rejects() {
    assert_eq!(validate_name(" ops team ").unwrap(), "ops team");
    assert_eq!(validate_name("a_b-c.d").unwrap(), "a_b-c.d");
    for bad in ["", "   ", &"x".repeat(65), "a/b"] {
        assert!(
            matches!(validate_name(bad), Err(Error::Invalid(_))),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn cascades() {
    let f = fx().await;
    let p = f.store.pool();
    let t = db::create(p, "ops", None, 0).await.unwrap();
    db::add_member(p, t.id, f.bob, 0).await.unwrap();
    db::add_member(p, t.id, f.carol, 0).await.unwrap();
    db::set_binding(p, t.id, f.env, EnvRole::Viewer, None, 0)
        .await
        .unwrap();
    db::set_binding(p, t.id, f.env2, EnvRole::Viewer, None, 0)
        .await
        .unwrap();

    f.store.delete_user(f.carol).await.unwrap();
    let members: Vec<_> = db::members(p, t.id)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(members, ["bob"]);

    f.store.delete_environment(f.env).await.unwrap();
    let envs: Vec<_> = db::bindings(p, t.id)
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.env_name)
        .collect();
    assert_eq!(envs, ["k"]);

    assert!(db::delete(p, t.id).await.unwrap());
    assert!(matches!(db::get(p, t.id).await, Err(Error::NotFound(_))));
    assert!(db::user_teams(p, f.bob).await.unwrap().is_empty());
    assert!(TeamGrants::new(p.clone())
        .user_grants(f.bob)
        .await
        .unwrap()
        .is_empty());
}
