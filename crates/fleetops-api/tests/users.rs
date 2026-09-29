mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn admin_creates_and_lists_users() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app.create_user(&admin, "bob", "bob-password", "user").await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["username"], "bob");
    assert_eq!(r.json["role"], "user");
    let list = app
        .send(req("GET", "/api/v1/users", None, Some(&admin)))
        .await;
    assert_eq!(list.json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn create_validates_and_rejects_duplicates() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let dup = app.create_user(&admin, "BOB", "bob-password", "user").await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert!(dup.json["message"]
        .as_str()
        .unwrap()
        .contains("already taken"));
    assert_eq!(
        app.create_user(&admin, "carol", "short", "user")
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.create_user(&admin, "carol", "carol-password", "root")
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn non_admin_is_forbidden() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    assert_eq!(
        app.send(req("GET", "/api/v1/users", None, Some(&bob)))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.create_user(&bob, "eve", "eve-password", "admin")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/audit", None, Some(&bob)))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn role_change_takes_effect_immediately() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app
        .create_user(&admin, "bob", "bob-password", "user")
        .await
        .json["id"]
        .as_i64()
        .unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req(
            "PATCH",
            &format!("/api/v1/users/{id}"),
            Some(json!({"role": "admin"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["role"], "admin");
    assert_eq!(
        app.send(req("GET", "/api/v1/users", None, Some(&bob)))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_password_reset_revokes_target_sessions() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app
        .create_user(&admin, "bob", "bob-password", "user")
        .await
        .json["id"]
        .as_i64()
        .unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req(
            "PATCH",
            &format!("/api/v1/users/{id}"),
            Some(json!({"password": "reset-password"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&bob)))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.login("bob", "reset-password").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_resetting_own_password_keeps_current_session() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app
        .send(req("GET", "/api/v1/auth/me", None, Some(&admin)))
        .await
        .json["user"]["id"]
        .as_i64()
        .unwrap();
    let r = app
        .send(req(
            "PATCH",
            &format!("/api/v1/users/{id}"),
            Some(json!({"password": "another-password"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&admin)))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn patch_validates() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app
        .send(req(
            "PATCH",
            "/api/v1/users/999",
            Some(json!({"role": "user"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app
        .send(req(
            "PATCH",
            "/api/v1/users/1",
            Some(json!({})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .send(req(
            "PATCH",
            "/api/v1/users/1",
            Some(json!({"password": "short"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn last_admin_is_protected() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app
        .send(req(
            "PATCH",
            "/api/v1/users/1",
            Some(json!({"role": "user"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app
        .send(req("DELETE", "/api/v1/users/1", None, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_user_kills_their_sessions() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app
        .create_user(&admin, "bob", "bob-password", "user")
        .await
        .json["id"]
        .as_i64()
        .unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/users/{id}"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&bob)))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/users/{id}"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audit_lists_admin_actions_with_paging() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    app.create_user(&admin, "carol", "carol-password", "user")
        .await;
    let r = app
        .send(req("GET", "/api/v1/audit?limit=2", None, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let page = r.json.as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["action"], "user.create");
    assert_eq!(page[0]["username"], ADMIN);
    let before = page[1]["id"].as_i64().unwrap();
    let rest = app
        .send(req(
            "GET",
            &format!("/api/v1/audit?before={before}"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(
        rest.json.as_array().unwrap().last().unwrap()["action"],
        "auth.setup"
    );
}
