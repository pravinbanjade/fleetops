//! LDAP sign-in against a real OpenLDAP (`make test-ldap`, which runs
//! scripts/ldap-up.sh and sets FLEETOPS_LDAP_IT). Each test builds its own
//! server; the directory is only read.

mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_core::now_unix;
use fleetops_ee::sso::{db, ldap};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const DAY: i64 = 86_400;
const OPS: &str = "cn=ops,ou=groups,dc=fleetops,dc=test";

fn url() -> Option<String> {
    let u = std::env::var("FLEETOPS_LDAP_IT").ok();
    if u.is_none() {
        eprintln!("FLEETOPS_LDAP_IT is not set; run `make test-ldap`");
    }
    u
}

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

fn settings(url: &str) -> Value {
    json!({
        "kind": "ldap", "preset": "openldap", "url": url,
        "bind_dn": "cn=admin,dc=fleetops,dc=test",
        "user_base_dn": "ou=people,dc=fleetops,dc=test",
        "group_base_dn": "ou=groups,dc=fleetops,dc=test"
    })
}

/// A licensed server with an enabled OpenLDAP connection.
async fn server(url: &str, extra: Value) -> (Ee, Session, i64) {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let r = ee
        .install(
            &admin,
            &license(&k, now_unix() + 30 * DAY, None, &["audit_export"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let mut v = json!({
        "name": "Corp directory",
        "enabled": true,
        "settings": settings(url),
        "secrets": {"bind_password": "admin"}
    });
    if let Value::Object(m) = extra {
        for (k, val) in m {
            v[k] = val;
        }
    }
    let r = ee
        .send("POST", "/api/v1/ee/sso/connections", body(v), Some(&admin))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let id = r.json()["id"].as_i64().unwrap();
    (ee, admin, id)
}

async fn login(ee: &Ee, user: &str, pw: &str) -> Resp {
    ee.send(
        "POST",
        "/api/v1/auth/login",
        body(json!({"username": user, "password": pw})),
        None,
    )
    .await
}

async fn team(ee: &Ee, admin: &Session, name: &str) -> i64 {
    ee.send(
        "POST",
        "/api/v1/ee/teams",
        body(json!({"name": name})),
        Some(admin),
    )
    .await
    .json()["id"]
        .as_i64()
        .unwrap()
}

async fn map(ee: &Ee, admin: &Session, conn: i64, v: Value) {
    let r = ee
        .send(
            "POST",
            &format!("/api/v1/ee/sso/connections/{conn}/mappings"),
            body(v),
            Some(admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
}

async fn team_members(ee: &Ee, team: i64) -> Vec<(i64, Option<i64>)> {
    sqlx::query_as("SELECT user_id, source_connection_id FROM ee_team_members WHERE team_id = ?")
        .bind(team)
        .fetch_all(ee.store.pool())
        .await
        .unwrap()
}

async fn user_id(ee: &Ee, name: &str) -> Option<i64> {
    ee.store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.username == name)
        .map(|u| u.id)
}

#[tokio::test]
async fn alice_signs_in_and_joins_mapped_team() {
    let Some(url) = url() else { return };
    let (ee, admin, conn) = server(&url, json!({})).await;
    let ops = team(&ee, &admin, "Ops").await;
    map(&ee, &admin, conn, json!({"group": OPS, "team_id": ops})).await;
    let r = login(&ee, "alice", "alice-pw").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let id = user_id(&ee, "alice").await.expect("alice created");
    assert!(db::is_managed(ee.store.pool(), id).await.unwrap());
    assert_eq!(team_members(&ee, ops).await, vec![(id, Some(conn))]);
    let (subject, email): (String, Option<String>) =
        sqlx::query_as("SELECT subject, email FROM ee_sso_identities WHERE user_id = ?")
            .bind(id)
            .fetch_one(ee.store.pool())
            .await
            .unwrap();
    assert_eq!(subject.len(), 36, "{subject}");
    assert_eq!(email.as_deref(), Some("alice@fleetops.test"));
    // The second sign-in finds the same account.
    assert_eq!(login(&ee, "alice", "alice-pw").await.status, StatusCode::OK);
    assert_eq!(ee.store.count_users().await.unwrap(), 2);
}

#[tokio::test]
async fn nested_group_is_not_expanded_for_openldap() {
    let Some(url) = url() else { return };
    let (ee, admin, conn) = server(&url, json!({})).await;
    let outer = team(&ee, &admin, "Outer").await;
    map(
        &ee,
        &admin,
        conn,
        json!({"group": "cn=outer,ou=groups,dc=fleetops,dc=test", "team_id": outer}),
    )
    .await;
    assert_eq!(login(&ee, "alice", "alice-pw").await.status, StatusCode::OK);
    assert!(team_members(&ee, outer).await.is_empty());
}

#[tokio::test]
async fn posix_group_by_member_uid() {
    let Some(url) = url() else { return };
    let (ee, admin, conn) = server(&url, json!({})).await;
    let devs = team(&ee, &admin, "Devs").await;
    map(
        &ee,
        &admin,
        conn,
        json!({"group": "CN=devs,ou=groups,dc=fleetops,dc=test", "team_id": devs}),
    )
    .await;
    assert_eq!(login(&ee, "carol", "carol-pw").await.status, StatusCode::OK);
    let carol = user_id(&ee, "carol").await.unwrap();
    assert_eq!(team_members(&ee, devs).await, vec![(carol, Some(conn))]);
}

#[tokio::test]
async fn admin_mapping_promotes() {
    let Some(url) = url() else { return };
    let (ee, admin, conn) = server(&url, json!({})).await;
    map(
        &ee,
        &admin,
        conn,
        json!({"group": "cn=admins,ou=groups,dc=fleetops,dc=test", "admin": true}),
    )
    .await;
    assert_eq!(login(&ee, "bob", "bob-pw").await.status, StatusCode::OK);
    let bob = user_id(&ee, "bob").await.unwrap();
    assert_eq!(
        ee.store.get_user(bob).await.unwrap().role,
        fleetops_core::Role::Admin
    );
}

#[tokio::test]
async fn wrong_password_is_401() {
    let Some(url) = url() else { return };
    let (ee, _, _) = server(&url, json!({})).await;
    let r = login(&ee, "alice", "not-it").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(user_id(&ee, "alice").await, None);
}

#[tokio::test]
async fn empty_password_is_refused() {
    let Some(url) = url() else { return };
    let (ee, _, _) = server(&url, json!({})).await;
    let r = login(&ee, "alice", "").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.text);
    assert_eq!(user_id(&ee, "alice").await, None);
}

#[tokio::test]
async fn unknown_user_is_401() {
    let Some(url) = url() else { return };
    let (ee, _, _) = server(&url, json!({})).await;
    assert_eq!(
        login(&ee, "mallory", "whatever").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn ambiguous_user_is_refused() {
    let Some(url) = url() else { return };
    let (ee, _, _) = server(&url, json!({})).await;
    assert_eq!(
        login(&ee, "dup", "dup-pw").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(user_id(&ee, "dup").await, None);
}

#[tokio::test]
async fn wildcard_username_matches_nobody() {
    let Some(url) = url() else { return };
    let s = fleetops_ee::sso::model::ConnectionSettings::parse(settings(&url))
        .unwrap()
        .validated()
        .unwrap();
    assert_eq!(ldap::lookup(&s, Some("admin"), "*").await.unwrap(), None);
    assert!(ldap::lookup(&s, Some("admin"), "alice")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn directory_alice_beside_local_alice() {
    let Some(url) = url() else { return };
    let (ee, admin, _) = server(&url, json!({})).await;
    let local = ee.user(&admin, "alice").await.json()["id"]
        .as_i64()
        .unwrap();
    let r = login(&ee, "alice", "alice-pw").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["user"]["username"], "alice-2", "{}", r.text);
    let r = login(&ee, "alice", PW).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["user"]["id"], local, "{}", r.text);
}

#[tokio::test]
async fn unreachable_server_keeps_local_admin() {
    let Some(_) = url() else { return };
    let (ee, _, _) = server("ldap://127.0.0.1:1", json!({})).await;
    let t = Instant::now();
    let r = login(&ee, "bob", "bob-pw").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.text);
    assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
    assert_eq!(login(&ee, "admin", PW).await.status, StatusCode::OK);
    let failed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'sso.login' AND result = 'failure'",
    )
    .fetch_one(ee.store.pool())
    .await
    .unwrap();
    assert_eq!(failed, 1);
}

#[tokio::test]
async fn session_lasts_connection_hours() {
    let Some(url) = url() else { return };
    let (ee, _, _) = server(&url, json!({"session_hours": 2})).await;
    assert_eq!(login(&ee, "alice", "alice-pw").await.status, StatusCode::OK);
    let id = user_id(&ee, "alice").await.unwrap();
    let s = ee.store.list_sessions(id).await.unwrap();
    assert_eq!(s.len(), 1);
    assert!(s[0].expires_at - s[0].created_at <= 7200, "{:?}", s[0]);
}

#[tokio::test]
async fn test_button_lists_groups() {
    let Some(url) = url() else { return };
    let (ee, admin, conn) = server(&url, json!({})).await;
    let r = ee
        .send(
            "POST",
            &format!("/api/v1/ee/sso/connections/{conn}/test"),
            body(json!({})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.json()["ok"], true, "{}", r.text);
    let r = ee
        .send(
            "POST",
            &format!("/api/v1/ee/sso/connections/{conn}/test"),
            body(json!({"username": "alice"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.json()["ok"], true, "{}", r.text);
    let groups = r.json()["groups"].to_string().to_lowercase();
    assert!(groups.contains(OPS), "{groups}");
}
