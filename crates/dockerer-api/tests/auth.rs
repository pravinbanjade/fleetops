mod common;

use axum::http::StatusCode;
use common::*;
use dockerer_api::{auth::token::hash_token, ApiConfig, IsTls};
use dockerer_core::now_unix;
use dockerer_store::NewSession;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn status_reports_setup_required_then_done() {
    let app = app().await;
    let r = app
        .send(req("GET", "/api/v1/auth/status", None, None))
        .await;
    assert_eq!(r.json, json!({"setup_required": true, "setup_open": true}));
    app.setup_admin().await;
    let r = app
        .send(req("GET", "/api/v1/auth/status", None, None))
        .await;
    assert_eq!(
        r.json,
        json!({"setup_required": false, "setup_open": false})
    );
}

#[tokio::test]
async fn setup_creates_admin_and_logs_in() {
    let app = app().await;
    let s = app.setup_admin().await;
    let me = app
        .send(req("GET", "/api/v1/auth/me", None, Some(&s)))
        .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json["user"]["username"], ADMIN);
    assert_eq!(me.json["user"]["role"], "admin");
    assert_eq!(me.json["csrf_token"], s.csrf.as_str());
    assert!(me.json["user"].get("password_hash").is_none());
}

#[tokio::test]
async fn second_setup_conflicts() {
    let app = app().await;
    app.setup_admin().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/auth/setup",
            Some(json!({"username": "x", "password": "password1"})),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_locked_after_window() {
    let app = app_with(ApiConfig {
        setup_window: Duration::ZERO,
        ..ApiConfig::default()
    })
    .await;
    let r = app
        .send(req("GET", "/api/v1/auth/status", None, None))
        .await;
    assert_eq!(r.json["setup_open"], false);
    let r = app
        .send(req(
            "POST",
            "/api/v1/auth/setup",
            Some(json!({"username": ADMIN, "password": ADMIN_PW})),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.json["message"].as_str().unwrap().contains("restart"));
}

#[tokio::test]
async fn setup_validates_input() {
    let app = app().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/auth/setup",
            Some(json!({"username": ADMIN, "password": "short"})),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json["code"], "invalid");
    let r = app
        .send(req(
            "POST",
            "/api/v1/auth/setup",
            Some(json!({"username": "bad name", "password": ADMIN_PW})),
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn concurrent_setup_creates_exactly_one_admin() {
    let app = app().await;
    let mut handles = Vec::new();
    for i in 0..8 {
        let router = app.router.clone();
        handles.push(tokio::spawn(async move {
            let r = req(
                "POST",
                "/api/v1/auth/setup",
                Some(json!({"username": format!("admin{i}"), "password": ADMIN_PW})),
                None,
            );
            tower::ServiceExt::oneshot(router, r)
                .await
                .unwrap()
                .status()
        }));
    }
    let mut created = 0;
    for h in handles {
        match h.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => {}
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(app.store.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn login_success_and_failures() {
    let app = app().await;
    app.setup_admin().await;
    let ok = app.login(ADMIN, ADMIN_PW).await;
    assert_eq!(ok.status, StatusCode::OK);
    session_from(&ok);
    assert_eq!(
        app.login(ADMIN, "wrong-password").await.status,
        StatusCode::UNAUTHORIZED
    );
    let unknown = app.login("ghost", "whatever-pw").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.json["message"], "invalid username or password");
}

#[tokio::test]
async fn login_requires_json_content_type() {
    let app = app().await;
    app.setup_admin().await;
    let mut r = req(
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username": ADMIN, "password": ADMIN_PW})),
        None,
    );
    r.headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(app.send(r).await.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn login_is_case_insensitive_and_rate_limit_shares_key() {
    let app = app().await;
    app.setup_admin().await;
    assert_eq!(app.login("ADMIN", ADMIN_PW).await.status, StatusCode::OK);
    for i in 0..5 {
        let name = if i % 2 == 0 { "Admin" } else { "admin" };
        assert_eq!(
            app.login(name, "nope-nope").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let blocked = app.login(ADMIN, ADMIN_PW).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers.get("retry-after").is_some());

    let other_ip = app
        .send(req_from(
            [10, 0, 0, 9],
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": ADMIN, "password": ADMIN_PW})),
            None,
        ))
        .await;
    assert_eq!(other_ip.status, StatusCode::OK);
}

#[tokio::test]
async fn me_requires_session() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/auth/me", None, None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let bogus = Session {
        cookie: "dockerer_session=deadbeef".into(),
        csrf: String::new(),
    };
    let r = app
        .send(req("GET", "/api/v1/auth/me", None, Some(&bogus)))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutations_require_matching_csrf() {
    let app = app().await;
    let s = app.setup_admin().await;
    let missing = Session {
        csrf: String::new(),
        ..s.clone()
    };
    let r = app
        .send(req("POST", "/api/v1/auth/logout", None, Some(&missing)))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let wrong = Session {
        csrf: "0".repeat(64),
        ..s.clone()
    };
    let r = app
        .send(req("POST", "/api/v1/auth/logout", None, Some(&wrong)))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .send(req("POST", "/api/v1/auth/logout", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn logout_invalidates_session_and_clears_cookie() {
    let app = app().await;
    let s = app.setup_admin().await;
    let r = app
        .send(req("POST", "/api/v1/auth/logout", None, Some(&s)))
        .await;
    assert!(r
        .headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let r = app
        .send(req("GET", "/api/v1/auth/me", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_and_idle_sessions_are_rejected_and_deleted() {
    let app = app().await;
    app.setup_admin().await;
    let uid = app
        .store
        .find_credentials(ADMIN)
        .await
        .unwrap()
        .unwrap()
        .0
        .id;
    let now = now_unix();
    for (token, created, expires) in [
        ("expired-tok", now - 10, now - 1),
        ("idle-tok", now - 8 * 86_400, now + 86_400),
    ] {
        let hash = hash_token(token);
        app.store
            .create_session(&NewSession {
                user_id: uid,
                token_hash: &hash,
                csrf_token: "c",
                now: created,
                expires_at: expires,
                user_agent: None,
                ip: None,
            })
            .await
            .unwrap();
        let s = Session {
            cookie: format!("dockerer_session={token}"),
            csrf: "c".into(),
        };
        let r = app
            .send(req("GET", "/api/v1/auth/me", None, Some(&s)))
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{token}");
        assert!(
            app.store.find_session(&hash).await.unwrap().is_none(),
            "{token} not deleted"
        );
    }
}

#[tokio::test]
async fn cookie_is_secure_only_over_tls() {
    let app = app().await;
    app.setup_admin().await;
    let plain = app.login(ADMIN, ADMIN_PW).await;
    let cookie = plain.headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(!cookie.contains("Secure"));

    let mut r = req(
        "POST",
        "/api/v1/auth/login",
        Some(json!({"username": ADMIN, "password": ADMIN_PW})),
        None,
    );
    r.extensions_mut().insert(IsTls(true));
    let tls = app.send(r).await;
    assert!(tls
        .headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("; Secure"));
}

#[tokio::test]
async fn login_and_setup_are_audited() {
    let app = app().await;
    app.setup_admin().await;
    app.login(ADMIN, "wrong-password").await;
    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.action, e.result))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("auth.login".to_string(), "failure".to_string()),
            ("auth.setup".into(), "success".into())
        ]
    );
}

#[tokio::test]
async fn concurrent_wrong_logins_cannot_bypass_rate_limit() {
    let app = app().await;
    app.setup_admin().await;
    let mut handles = Vec::new();
    for _ in 0..20 {
        let router = app.router.clone();
        handles.push(tokio::spawn(async move {
            let r = req(
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username": ADMIN, "password": "wrong-password"})),
                None,
            );
            tower::ServiceExt::oneshot(router, r)
                .await
                .unwrap()
                .status()
        }));
    }
    let (mut unauthorized, mut limited) = (0, 0);
    for h in handles {
        match h.await.unwrap() {
            StatusCode::UNAUTHORIZED => unauthorized += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert!(unauthorized <= 5, "{unauthorized} attempts got through");
    assert_eq!(limited, 20 - unauthorized);
}

#[tokio::test]
async fn oversize_login_username_is_not_stored_verbatim() {
    let app = app().await;
    app.setup_admin().await;
    let r = app.login(&"a".repeat(10_000), "whatever-pw").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    for e in app.store.list_audit(10, None).await.unwrap() {
        assert!(e.target_id.unwrap_or_default().len() <= 64);
    }
}

#[tokio::test]
async fn malformed_json_gets_json_error_body() {
    let app = app().await;
    let mut r = req("POST", "/api/v1/auth/login", None, None);
    *r.body_mut() = axum::body::Body::from("{not json");
    r.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    let r = app.send(r).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json["code"], "invalid");
    assert!(r.json["request_id"].is_string());
}

#[tokio::test]
async fn bad_path_param_gets_json_error_body() {
    let app = app().await;
    let s = app.setup_admin().await;
    let r = app
        .send(req("DELETE", "/api/v1/users/abc", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json["code"], "invalid");
}
