mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use fleetops_core::{
    external::{ExternalAuth, LoginProvider},
    Result, Role, User,
};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

/// A stand-in for the Enterprise hook: `managed` users are SSO users, and
/// `directory` is what a directory login for "dirk" returns.
#[derive(Default)]
struct Stub {
    managed: HashMap<i64, String>,
    directory: Option<(User, Option<i64>)>,
    /// Panic if a directory login is attempted.
    no_directory: bool,
}

#[async_trait]
impl ExternalAuth for Stub {
    async fn providers(&self) -> Result<Vec<LoginProvider>> {
        Ok(vec![LoginProvider {
            id: 3,
            name: "Acme Okta".into(),
            kind: "oidc".into(),
        }])
    }
    async fn password_allowed(&self, user: &User) -> Result<bool> {
        Ok(!self.managed.contains_key(&user.id))
    }
    async fn directory_login(&self, _: &str, _: &str) -> Result<Option<(User, Option<i64>)>> {
        assert!(!self.no_directory, "directory login attempted");
        Ok(self.directory.clone())
    }
    async fn managed_users(&self) -> Result<HashMap<i64, String>> {
        Ok(self.managed.clone())
    }
}

fn user(id: i64, name: &str) -> User {
    User {
        id,
        username: name.into(),
        role: Role::User,
        created_at: 0,
        updated_at: 0,
        sso: None,
    }
}

#[tokio::test]
async fn providers_route_lists_the_hook_and_is_public() {
    let app = app_with_external(Arc::new(Stub::default())).await;
    let r = app
        .send(req("GET", "/api/v1/auth/providers", None, None))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json,
        json!([{"id": 3, "name": "Acme Okta", "kind": "oidc"}])
    );
}

#[tokio::test]
async fn community_providers_is_empty() {
    let app = app().await;
    let r = app
        .send(req("GET", "/api/v1/auth/providers", None, None))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json, json!([]));
}

#[tokio::test]
async fn password_login_refused_when_hook_disallows_it() {
    // User 2 is created second, after the admin.
    let stub = Stub {
        managed: HashMap::from([(2, "Acme Okta".to_string())]),
        ..Stub::default()
    };
    let app = app_with_external(Arc::new(stub)).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "sam", "sam-password", "user").await;
    let refused = app.login("sam", "sam-password").await;
    let wrong = app.login("admin", "not-the-password").await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused.status, wrong.status);
    for k in ["code", "message"] {
        assert_eq!(refused.json[k], wrong.json[k]);
    }
}

#[tokio::test]
async fn directory_login_runs_when_local_check_fails() {
    let app = app().await;
    app.setup_admin().await;
    let created = app
        .store
        .create_user("dirk", "!sso", Role::User, 0)
        .await
        .unwrap();
    let stub = Stub {
        directory: Some((user(created.id, "dirk"), Some(12))),
        ..Stub::default()
    };
    let state = app.state.clone().with_external_auth(Arc::new(stub));
    let router = fleetops_api::app(state.clone(), axum::Router::new());
    let app = TestApp {
        router,
        store: app.store.clone(),
        state,
        fake: app.fake.clone(),
    };
    let r = app.login("dirk", "directory-password").await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let cookie = r.headers["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("Max-Age=43200"), "{cookie}");
}

#[tokio::test]
async fn local_password_wins_over_directory() {
    let stub = Stub {
        no_directory: true,
        ..Stub::default()
    };
    let app = app_with_external(Arc::new(stub)).await;
    app.setup_admin().await;
    let r = app.login(ADMIN, ADMIN_PW).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn users_route_fills_sso_name() {
    let stub = Stub {
        managed: HashMap::from([(2, "Acme Okta".to_string())]),
        ..Stub::default()
    };
    let app = app_with_external(Arc::new(stub)).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "sam", "sam-password", "user").await;
    let r = app
        .send(req("GET", "/api/v1/users", None, Some(&admin)))
        .await;
    let users = r.json.as_array().unwrap();
    assert_eq!(users[0].get("sso"), None);
    assert_eq!(users[1]["sso"], "Acme Okta");
}

#[tokio::test]
async fn update_refuses_role_or_password_for_managed_user() {
    let stub = Stub {
        managed: HashMap::from([(2, "Acme Okta".to_string())]),
        ..Stub::default()
    };
    let app = app_with_external(Arc::new(stub)).await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "sam", "sam-password", "user").await;
    for body in [
        json!({"role": "admin"}),
        json!({"password": "new-password"}),
    ] {
        let r = app
            .send(req("PATCH", "/api/v1/users/2", Some(body), Some(&admin)))
            .await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.json);
    }
}
