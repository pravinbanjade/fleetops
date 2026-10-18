mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_api::ApiConfig;
use fleetops_core::{now_unix, Role};
use fleetops_ee::sso::{
    db,
    provision::{self, Claims, Refusal},
};
use serde_json::{json, Value};

const DAY: i64 = 86_400;

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

struct Fx {
    ee: Ee,
    admin: Session,
    conn: db::Connection,
    ops: i64,
    dev: i64,
}

async fn fx_with(max_users: Option<u32>, require_mapped_group: bool) -> Fx {
    let k = key();
    let ee = ee_app_with(
        &k,
        None,
        ApiConfig {
            public_url: Some(PUBLIC_URL.into()),
            ..ApiConfig::default()
        },
    )
    .await;
    let admin = ee.admin().await;
    ee.install(&admin, &license(&k, now_unix() + DAY, max_users, &[]))
        .await;
    let r = ee
        .send(
            "POST",
            "/api/v1/ee/sso/connections",
            body(json!({
                "name": "Acme",
                "settings": {"kind": "oidc", "issuer": "https://idp.acme.io", "client_id": "fleet"},
                "secrets": {"client_secret": "x"},
                "enabled": true,
                "require_mapped_group": require_mapped_group
            })),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let conn = db::get(ee.store.pool(), r.json()["id"].as_i64().unwrap())
        .await
        .unwrap();
    let team = |name: &str| {
        let ee = &ee;
        let admin = &admin;
        let name = name.to_string();
        async move {
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
    };
    let ops = team("Ops").await;
    let dev = team("Dev").await;
    let now = now_unix();
    db::add_mapping(ee.store.pool(), conn.id, "ops", Some(ops), now)
        .await
        .unwrap();
    db::add_mapping(ee.store.pool(), conn.id, "dev", Some(dev), now)
        .await
        .unwrap();
    db::add_mapping(ee.store.pool(), conn.id, "fleet-admins", None, now)
        .await
        .unwrap();
    Fx {
        ee,
        admin,
        conn,
        ops,
        dev,
    }
}

async fn fx() -> Fx {
    fx_with(None, false).await
}

fn claims(subject: &str, email: &str, groups: &[&str]) -> Claims {
    Claims {
        subject: subject.into(),
        email: Some(email.into()),
        groups: groups.iter().map(|g| g.to_string()).collect(),
        ..Claims::default()
    }
}

async fn teams_of(f: &Fx, user: i64) -> Vec<(i64, Option<i64>)> {
    sqlx::query_as("SELECT team_id, source_connection_id FROM ee_team_members WHERE user_id = ? ORDER BY team_id")
        .bind(user)
        .fetch_all(f.ee.store.pool())
        .await
        .unwrap()
}

async fn sign_in(f: &Fx, c: Claims) -> Result<fleetops_core::User, Refusal> {
    provision::login(&f.ee.state, &f.conn, c).await.unwrap()
}

#[tokio::test]
async fn first_login_creates_user_and_identity() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["ops"]))
        .await
        .unwrap();
    assert_eq!(u.username, "jane");
    assert_eq!(u.role, Role::User);
    assert_eq!(teams_of(&f, u.id).await, vec![(f.ops, Some(f.conn.id))]);
    assert!(f
        .ee
        .actions()
        .await
        .contains(&"sso.user_create".to_string()));
}

#[tokio::test]
async fn second_login_finds_by_subject_even_if_email_changed() {
    let f = fx().await;
    let a = sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    let b = sign_in(&f, claims("sub-1", "jane.doe@acme.io", &[]))
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(b.username, "jane");
}

#[tokio::test]
async fn username_clash_gets_a_suffix() {
    let f = fx().await;
    f.ee.user(&f.admin, "jane").await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    assert_eq!(u.username, "jane-2");
}

#[tokio::test]
async fn manual_membership_survives_leaving_group() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    let r =
        f.ee.send(
            "PUT",
            &format!("/api/v1/ee/teams/{}/members/{}", f.ops, u.id),
            None,
            Some(&f.admin),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.text);
    sign_in(&f, claims("sub-1", "jane@acme.io", &["ops"]))
        .await
        .unwrap();
    sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    assert_eq!(teams_of(&f, u.id).await, vec![(f.ops, None)]);
}

#[tokio::test]
async fn leaving_group_removes_sync_membership_and_signals() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["ops", "dev"]))
        .await
        .unwrap();
    let mut rx = f.ee.state.hubs.access().subscribe();
    sign_in(&f, claims("sub-1", "jane@acme.io", &["dev"]))
        .await
        .unwrap();
    assert_eq!(teams_of(&f, u.id).await, vec![(f.dev, Some(f.conn.id))]);
    assert_eq!(rx.try_recv().unwrap(), u.id);
}

#[tokio::test]
async fn admin_mapping_promotes_then_demotes() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["Fleet-Admins"]))
        .await
        .unwrap();
    assert_eq!(u.role, Role::Admin);
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    assert_eq!(u.role, Role::User);
}

#[tokio::test]
async fn sync_keeps_last_admin() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["fleet-admins"]))
        .await
        .unwrap();
    f.ee.store.delete_user(1).await.unwrap();
    let u2 = sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    assert_eq!(u2.id, u.id);
    assert_eq!(u2.role, Role::Admin);
}

#[tokio::test]
async fn seat_limit_blocks_create_not_existing_login() {
    let f = fx_with(Some(2), false).await;
    sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
    let r = sign_in(&f, claims("sub-2", "joe@acme.io", &[])).await;
    assert_eq!(r.unwrap_err(), Refusal::SeatLimit);
    sign_in(&f, claims("sub-1", "jane@acme.io", &[]))
        .await
        .unwrap();
}

#[tokio::test]
async fn unlicensed_blocks_create_not_existing_login() {
    let f = fx().await;
    sign_in(&f, claims("sub-1", "jane@acme.io", &["ops"]))
        .await
        .unwrap();
    f.ee.send("DELETE", "/api/v1/ee/license", None, Some(&f.admin))
        .await;
    let r = sign_in(&f, claims("sub-2", "joe@acme.io", &[])).await;
    assert_eq!(r.unwrap_err(), Refusal::Unlicensed);
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["dev"]))
        .await
        .unwrap();
    assert_eq!(teams_of(&f, u.id).await, vec![(f.dev, Some(f.conn.id))]);
}

#[tokio::test]
async fn require_mapped_group_denies() {
    let f = fx_with(None, true).await;
    let r = sign_in(&f, claims("sub-1", "jane@acme.io", &["sales"])).await;
    assert_eq!(r.unwrap_err(), Refusal::Denied);
    sign_in(&f, claims("sub-1", "jane@acme.io", &["ops"]))
        .await
        .unwrap();
}

#[tokio::test]
async fn resync_after_mapping_delete_uses_stored_groups() {
    let f = fx().await;
    let u = sign_in(&f, claims("sub-1", "jane@acme.io", &["ops"]))
        .await
        .unwrap();
    let m = db::mappings(f.ee.store.pool(), f.conn.id).await.unwrap();
    let ops = m.iter().find(|m| m.team_id == Some(f.ops)).unwrap();
    db::delete_mapping(f.ee.store.pool(), f.conn.id, ops.id)
        .await
        .unwrap();
    provision::resync_connection(&f.ee.state, f.conn.id)
        .await
        .unwrap();
    assert!(teams_of(&f, u.id).await.is_empty());
}

#[tokio::test]
async fn detach_removes_sync_rows_demotes_and_ends_sessions() {
    let f = fx().await;
    let u = sign_in(
        &f,
        claims("sub-1", "jane@acme.io", &["ops", "fleet-admins"]),
    )
    .await
    .unwrap();
    f.ee.store
        .create_session(&fleetops_store::NewSession {
            user_id: u.id,
            token_hash: "h",
            csrf_token: "c",
            now: now_unix(),
            expires_at: now_unix() + DAY,
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();
    provision::detach_connection(&f.ee.state, f.conn.id)
        .await
        .unwrap();
    assert!(teams_of(&f, u.id).await.is_empty());
    assert_eq!(f.ee.store.get_user(u.id).await.unwrap().role, Role::User);
    assert!(f.ee.store.list_sessions(u.id).await.unwrap().is_empty());
}
