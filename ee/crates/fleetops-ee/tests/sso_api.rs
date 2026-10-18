mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_api::ApiConfig;
use fleetops_core::now_unix;
use serde_json::{json, Value};

const DAY: i64 = 86_400;

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

fn oidc(name: &str) -> Value {
    json!({
        "name": name,
        "settings": {"kind": "oidc", "issuer": "https://idp.acme.io", "client_id": "fleet"},
        "secrets": {"client_secret": "s3cr3t-value"}
    })
}

/// A licensed server with a public URL, and its admin.
async fn licensed() -> (Ee, Session) {
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
    let r = ee
        .install(
            &admin,
            &license(&k, now_unix() + 30 * DAY, None, &["audit_export"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    (ee, admin)
}

async fn create(ee: &Ee, admin: &Session, v: Value) -> Resp {
    ee.send("POST", "/api/v1/ee/sso/connections", body(v), Some(admin))
        .await
}

#[tokio::test]
async fn license_listing_only_audit_export_has_sso() {
    let (ee, admin) = licensed().await;
    let r = ee.send("GET", "/api/v1/edition", None, Some(&admin)).await;
    let features = r.json()["features"].to_string();
    assert!(features.contains("\"sso\""), "{features}");
}

#[tokio::test]
async fn create_connection_hides_secrets() {
    let (ee, admin) = licensed().await;
    let r = create(&ee, &admin, oidc("Acme Okta")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert!(!r.text.contains("s3cr3t-value"), "{}", r.text);
    let v = r.json();
    assert_eq!(v["secrets_set"], json!(["client_secret"]));
    assert_eq!(v["kind"], "oidc");
    assert_eq!(
        v["callback_url"],
        format!("{PUBLIC_URL}/api/v1/ee/sso/{}/callback", v["id"])
    );
    let list = ee
        .send("GET", "/api/v1/ee/sso/connections", None, Some(&admin))
        .await;
    assert!(!list.text.contains("s3cr3t-value"));
}

#[tokio::test]
async fn secret_survives_patch_without_secret() {
    let (ee, admin) = licensed().await;
    let id = create(&ee, &admin, oidc("Acme")).await.json()["id"]
        .as_i64()
        .unwrap();
    let r = ee
        .send(
            "PATCH",
            &format!("/api/v1/ee/sso/connections/{id}"),
            body(json!({"name": "Acme Okta", "enabled": true})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["secrets_set"], json!(["client_secret"]));
    let s = fleetops_ee::sso::db::secrets(&ee.store, id).await.unwrap();
    assert_eq!(s.client_secret.as_deref(), Some("s3cr3t-value"));
}

#[tokio::test]
async fn enable_needs_public_url() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.install(&admin, &license(&k, now_unix() + DAY, None, &[]))
        .await;
    let mut v = oidc("Acme");
    v["enabled"] = json!(true);
    let r = create(&ee, &admin, v).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text.contains("FLEETOPS_PUBLIC_URL"), "{}", r.text);
}

#[tokio::test]
async fn rejects_common_tenant() {
    let (ee, admin) = licensed().await;
    let r = create(
        &ee,
        &admin,
        json!({"name": "MS", "settings": {"kind": "microsoft", "tenant": "common", "client_id": "x"}}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text);
}

#[tokio::test]
async fn ldap_kind_refused_for_now() {
    let (ee, admin) = licensed().await;
    let r = create(
        &ee,
        &admin,
        json!({"name": "AD", "settings": {"kind": "ldap"}}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text.contains("later release"), "{}", r.text);
}

#[tokio::test]
async fn unlicensed_create_refused_but_disable_allowed() {
    let (ee, admin) = licensed().await;
    let mut v = oidc("Acme");
    v["enabled"] = json!(true);
    let id = create(&ee, &admin, v).await.json()["id"].as_i64().unwrap();
    let r = ee
        .send("DELETE", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert!(r.status.is_success(), "{}", r.text);
    let r = create(&ee, &admin, oidc("Other")).await;
    assert_eq!(r.json()["code"], "license_required", "{}", r.text);
    let rename = ee
        .send(
            "PATCH",
            &format!("/api/v1/ee/sso/connections/{id}"),
            body(json!({"name": "Renamed"})),
            Some(&admin),
        )
        .await;
    assert_eq!(rename.json()["code"], "license_required");
    let off = ee
        .send(
            "PATCH",
            &format!("/api/v1/ee/sso/connections/{id}"),
            body(json!({"enabled": false})),
            Some(&admin),
        )
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text);
    assert_eq!(off.json()["enabled"], false);
}

#[tokio::test]
async fn providers_lists_only_enabled_redirect_connections() {
    let (ee, admin) = licensed().await;
    let mut on = oidc("On");
    on["enabled"] = json!(true);
    create(&ee, &admin, on).await;
    create(&ee, &admin, oidc("Off")).await;
    let r = ee.send("GET", "/api/v1/auth/providers", None, None).await;
    assert_eq!(r.json().as_array().unwrap().len(), 1, "{}", r.text);
    assert_eq!(r.json()[0]["name"], "On");
}

#[tokio::test]
async fn password_mode_local_admins_blocks_local_user_but_not_local_admin() {
    let (ee, admin) = licensed().await;
    ee.user(&admin, "bob").await;
    let r = ee
        .send(
            "PUT",
            "/api/v1/ee/sso/settings",
            body(json!({"password_login": "local_admins"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let bob = ee
        .send(
            "POST",
            "/api/v1/auth/login",
            body(json!({"username": "bob", "password": PW})),
            None,
        )
        .await;
    assert_eq!(bob.status, StatusCode::UNAUTHORIZED);
    ee.login("admin").await;
}

#[tokio::test]
async fn local_admins_mode_refused_without_a_local_admin() {
    let (ee, admin) = licensed().await;
    // Make the only admin an SSO user.
    let conn = create(&ee, &admin, oidc("Acme")).await.json()["id"]
        .as_i64()
        .unwrap();
    sqlx::query("INSERT INTO ee_sso_identities (user_id, connection_id, subject, last_login_at) VALUES (1, ?, 'x', 0)")
        .bind(conn)
        .execute(ee.store.pool())
        .await
        .unwrap();
    let r = ee
        .send(
            "PUT",
            "/api/v1/ee/sso/settings",
            body(json!({"password_login": "local_admins"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
}

#[tokio::test]
async fn sso_user_password_login_looks_like_a_wrong_password() {
    let (ee, admin) = licensed().await;
    let conn = create(&ee, &admin, oidc("Acme")).await.json()["id"]
        .as_i64()
        .unwrap();
    let id = ee.user(&admin, "sam").await.json()["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO ee_sso_identities (user_id, connection_id, subject, last_login_at) VALUES (?, ?, 'sam', 0)")
        .bind(id)
        .bind(conn)
        .execute(ee.store.pool())
        .await
        .unwrap();
    let wrong = ee
        .send(
            "POST",
            "/api/v1/auth/login",
            body(json!({"username": "admin", "password": "nope-nope"})),
            None,
        )
        .await;
    for pw in [PW, "!sso"] {
        let r = ee
            .send(
                "POST",
                "/api/v1/auth/login",
                body(json!({"username": "sam", "password": pw})),
                None,
            )
            .await;
        assert_eq!(r.status, wrong.status);
        for k in ["code", "message"] {
            assert_eq!(r.json()[k], wrong.json()[k]);
        }
    }
}

// ---- mappings and connection lifecycle (Task 4) ----------------------------

use fleetops_ee::sso::provision::{self, Claims};

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

async fn enabled(ee: &Ee, admin: &Session) -> i64 {
    let mut v = oidc("Acme");
    v["enabled"] = json!(true);
    create(ee, admin, v).await.json()["id"].as_i64().unwrap()
}

async fn map(ee: &Ee, admin: &Session, conn: i64, v: Value) -> Resp {
    ee.send(
        "POST",
        &format!("/api/v1/ee/sso/connections/{conn}/mappings"),
        body(v),
        Some(admin),
    )
    .await
}

async fn jane(ee: &Ee, conn: i64, groups: &[&str]) -> fleetops_core::User {
    let c = fleetops_ee::sso::db::get(ee.store.pool(), conn)
        .await
        .unwrap();
    provision::login(
        &ee.state,
        &c,
        Claims {
            subject: "jane-sub".into(),
            email: Some("jane@acme.io".into()),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            ..Claims::default()
        },
    )
    .await
    .unwrap()
    .unwrap()
}

async fn team_members(ee: &Ee, team: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT user_id FROM ee_team_members WHERE team_id = ?")
        .bind(team)
        .fetch_all(ee.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn mapping_needs_exactly_one_target() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let ops = team(&ee, &admin, "Ops").await;
    for v in [
        json!({"group": "ops"}),
        json!({"group": "ops", "team_id": ops, "admin": true}),
    ] {
        assert_eq!(
            map(&ee, &admin, conn, v).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    let r = map(&ee, &admin, conn, json!({"group": "ops", "team_id": ops})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    assert_eq!(r.json()["team_name"], "Ops");
    let r = map(&ee, &admin, conn, json!({"group": "admins", "admin": true})).await;
    assert_eq!(r.json()["admin"], true);
}

#[tokio::test]
async fn deleting_mapping_removes_sync_membership_now() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let ops = team(&ee, &admin, "Ops").await;
    let mid = map(&ee, &admin, conn, json!({"group": "ops", "team_id": ops}))
        .await
        .json()["id"]
        .as_i64()
        .unwrap();
    let u = jane(&ee, conn, &["ops"]).await;
    assert_eq!(team_members(&ee, ops).await, vec![u.id]);
    let r = ee
        .send(
            "DELETE",
            &format!("/api/v1/ee/sso/connections/{conn}/mappings/{mid}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(team_members(&ee, ops).await.is_empty());
}

#[tokio::test]
async fn disabling_connection_ends_sessions_and_removes_sync_rows() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let ops = team(&ee, &admin, "Ops").await;
    map(&ee, &admin, conn, json!({"group": "ops", "team_id": ops})).await;
    let u = jane(&ee, conn, &["ops"]).await;
    ee.store
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
    let r = ee
        .send(
            "PATCH",
            &format!("/api/v1/ee/sso/connections/{conn}"),
            body(json!({"enabled": false})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert!(team_members(&ee, ops).await.is_empty());
    assert!(ee.store.list_sessions(u.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_connection_keeps_users_and_reports_count() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let u = jane(&ee, conn, &[]).await;
    let r = ee
        .send(
            "DELETE",
            &format!("/api/v1/ee/sso/connections/{conn}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["users_left"], 1);
    assert_eq!(ee.store.get_user(u.id).await.unwrap().username, "jane");
    let pw = ee
        .send(
            "POST",
            "/api/v1/auth/login",
            body(json!({"username": "jane", "password": "!sso"})),
            None,
        )
        .await;
    assert_eq!(pw.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unlicensed_add_mapping_refused_delete_allowed() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let ops = team(&ee, &admin, "Ops").await;
    let mid = map(&ee, &admin, conn, json!({"group": "ops", "team_id": ops}))
        .await
        .json()["id"]
        .as_i64()
        .unwrap();
    ee.send("DELETE", "/api/v1/ee/license", None, Some(&admin))
        .await;
    let r = map(&ee, &admin, conn, json!({"group": "dev", "team_id": ops})).await;
    assert_eq!(r.json()["code"], "license_required");
    let r = ee
        .send(
            "DELETE",
            &format!("/api/v1/ee/sso/connections/{conn}/mappings/{mid}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn local_admin_logs_in_while_connection_issuer_unreachable() {
    let (ee, admin) = licensed().await;
    let mut v = oidc("Dead");
    v["settings"]["issuer"] = json!("http://127.0.0.1:1");
    v["enabled"] = json!(true);
    assert_eq!(create(&ee, &admin, v).await.status, StatusCode::CREATED);
    ee.send(
        "PUT",
        "/api/v1/ee/sso/settings",
        body(json!({"password_login": "local_admins"})),
        Some(&admin),
    )
    .await;
    ee.login("admin").await;
}

#[tokio::test]
async fn team_member_list_marks_sync_rows() {
    let (ee, admin) = licensed().await;
    let conn = enabled(&ee, &admin).await;
    let ops = team(&ee, &admin, "Ops").await;
    map(&ee, &admin, conn, json!({"group": "ops", "team_id": ops})).await;
    let u = jane(&ee, conn, &["ops"]).await;
    let r = ee
        .send(
            "GET",
            &format!("/api/v1/ee/teams/{ops}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(
        r.json()["synced"],
        json!([{"user_id": u.id, "connection": "Acme"}])
    );
    let rm = ee
        .send(
            "DELETE",
            &format!("/api/v1/ee/teams/{ops}/members/{}", u.id),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(rm.status, StatusCode::CONFLICT, "{}", rm.text);
}
