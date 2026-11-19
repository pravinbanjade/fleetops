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

fn demo_config() -> fleetops_api::ApiConfig {
    fleetops_api::ApiConfig {
        demo: fleetops_api::DemoLogin::parse("demo:demo-password"),
        ..fleetops_api::ApiConfig::default()
    }
}

#[tokio::test]
async fn status_shows_the_demo_login() {
    let app = app_with(demo_config()).await;
    let r = app
        .send(req("GET", "/api/v1/auth/status", None, None))
        .await;
    assert_eq!(r.json["demo"]["username"], "demo");
    assert_eq!(r.json["demo"]["password"], "demo-password");

    let plain = common::app().await;
    let r = plain
        .send(req("GET", "/api/v1/auth/status", None, None))
        .await;
    assert!(r.json["demo"].is_null());
}

#[tokio::test]
async fn demo_visitors_see_only_their_own_session() {
    let app = app_with(demo_config()).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "demo", "demo-password", "user")
        .await;
    let a = session_from(&app.login("demo", "demo-password").await);
    let b = session_from(&app.login("demo", "demo-password").await);

    let list = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&a)))
        .await
        .json;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["current"], true);
    let own = list[0]["id"].as_i64().unwrap();

    // Visitor A cannot end visitor B's session, even knowing its id.
    let b_id = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&b)))
        .await
        .json[0]["id"]
        .as_i64()
        .unwrap();
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/auth/sessions/{b_id}"),
            None,
            Some(&a),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&b)))
            .await
            .status,
        StatusCode::OK
    );

    // Their own session they can end.
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/auth/sessions/{own}"),
            None,
            Some(&a),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn demo_password_cannot_change() {
    let app = app_with(demo_config()).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "demo", "demo-password", "user")
        .await;
    let a = session_from(&app.login("demo", "demo-password").await);
    let b = session_from(&app.login("demo", "demo-password").await);
    let r = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": "demo-password", "new_password": "taken-over-now"})),
            Some(&a),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(req("GET", "/api/v1/auth/me", None, Some(&b)))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.login("demo", "demo-password").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn other_users_keep_their_session_controls_on_the_demo() {
    let app = app_with(demo_config()).await;
    let s1 = app.setup_admin().await;
    let _s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);
    let r = app
        .send(req("GET", "/api/v1/auth/sessions", None, Some(&s1)))
        .await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
}

#[test]
fn demo_login_parses() {
    let d = fleetops_api::DemoLogin::parse("demo:a:b").unwrap();
    assert_eq!((d.username.as_str(), d.password.as_str()), ("demo", "a:b"));
    assert!(fleetops_api::DemoLogin::parse("demo").is_none());
    assert!(fleetops_api::DemoLogin::parse(":pw").is_none());
    assert!(fleetops_api::DemoLogin::parse("demo:").is_none());
}

#[tokio::test]
async fn wrong_demo_passwords_do_not_lock_out_the_demo() {
    let app = app_with(demo_config()).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "demo", "demo-password", "user")
        .await;
    for _ in 0..10 {
        let _ = app.login("demo", "not-the-password").await;
    }
    assert_eq!(
        app.login("demo", "not-the-password").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.login("demo", "demo-password").await.status,
        StatusCode::OK
    );
}
