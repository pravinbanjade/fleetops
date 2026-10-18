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
        .install(&admin, &license(&k, now_unix() + 30 * DAY, None, &["audit_export"]))
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
    let id = create(&ee, &admin, oidc("Acme")).await.json()["id"].as_i64().unwrap();
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
    ee.install(&admin, &license(&k, now_unix() + DAY, None, &[])).await;
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
    let r = create(&ee, &admin, json!({"name": "AD", "settings": {"kind": "ldap"}})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text.contains("later release"), "{}", r.text);
}

#[tokio::test]
async fn unlicensed_create_refused_but_disable_allowed() {
    let (ee, admin) = licensed().await;
    let mut v = oidc("Acme");
    v["enabled"] = json!(true);
    let id = create(&ee, &admin, v).await.json()["id"].as_i64().unwrap();
    let r = ee.send("DELETE", "/api/v1/ee/license", None, Some(&admin)).await;
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
    let conn = create(&ee, &admin, oidc("Acme")).await.json()["id"].as_i64().unwrap();
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
    let conn = create(&ee, &admin, oidc("Acme")).await.json()["id"].as_i64().unwrap();
    let id = ee.user(&admin, "sam").await.json()["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO ee_sso_identities (user_id, connection_id, subject, last_login_at) VALUES (?, ?, 'sam', 0)")
        .bind(id)
        .bind(conn)
        .execute(ee.store.pool())
        .await
        .unwrap();
    let wrong = ee
        .send("POST", "/api/v1/auth/login", body(json!({"username": "admin", "password": "nope-nope"})), None)
        .await;
    for pw in [PW, "!sso"] {
        let r = ee
            .send("POST", "/api/v1/auth/login", body(json!({"username": "sam", "password": pw})), None)
            .await;
        assert_eq!(r.status, wrong.status);
        assert_eq!(r.text, wrong.text);
    }
}
