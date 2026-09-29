mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn list_marks_current_session() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let _s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);
    let r = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&s1)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let list = r.json.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    assert_eq!(list[0]["ip"], "127.0.0.1");
}

#[tokio::test]
async fn revoke_other_session() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);
    let list = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&s1)))
        .await
        .json;
    let other = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == false)
        .unwrap()["id"]
        .as_i64()
        .unwrap();

    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/auth/sessions/{other}"),
            None,
            Some(&s1),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&s2)))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&s1)))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs Task 9"]
async fn cannot_revoke_someone_elses_session() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    let admin_sessions = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&admin)))
        .await
        .json;
    let admin_sid = admin_sessions[0]["id"].as_i64().unwrap();
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/auth/sessions/{admin_sid}"),
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn change_password_flow() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);

    let wrong = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": "nope-nope", "new_password": "brand-new-pw"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);

    let short = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": ADMIN_PW, "new_password": "short"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);

    let ok = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": ADMIN_PW, "new_password": "brand-new-pw"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&s1)))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&s2)))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.login(ADMIN, ADMIN_PW).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.login(ADMIN, "brand-new-pw").await.status,
        StatusCode::OK
    );
}
