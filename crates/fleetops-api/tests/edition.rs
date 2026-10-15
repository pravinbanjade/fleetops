mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_core::edition::{Edition, Entitlements, Feature};
use std::sync::Arc;

struct Seats(u32);
impl Entitlements for Seats {
    fn edition(&self) -> Edition {
        Edition::Enterprise
    }
    fn has(&self, f: Feature) -> bool {
        f == Feature::AuditExport
    }
    fn max_users(&self) -> Option<u32> {
        Some(self.0)
    }
}

#[tokio::test]
async fn community_edition_has_no_features() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app
        .send(req("GET", "/api/v1/edition", None, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json,
        serde_json::json!({"edition": "community", "features": []})
    );
}

#[tokio::test]
async fn edition_needs_a_session() {
    let app = app().await;
    app.setup_admin().await;
    let r = app.send(req("GET", "/api/v1/edition", None, None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn edition_reports_entitled_features() {
    let app = app_entitled(Arc::new(Seats(10))).await;
    let admin = app.setup_admin().await;
    let r = app
        .send(req("GET", "/api/v1/edition", None, Some(&admin)))
        .await;
    assert_eq!(
        r.json,
        serde_json::json!({"edition": "enterprise", "features": ["audit_export"]})
    );
}

#[tokio::test]
async fn community_has_no_user_limit() {
    let app = app().await;
    let admin = app.setup_admin().await;
    for i in 0..5 {
        let r = app
            .create_user(&admin, &format!("u{i}"), "user-password", "user")
            .await;
        assert_eq!(r.status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn seat_limit_blocks_the_user_past_it() {
    let app = app_entitled(Arc::new(Seats(2))).await;
    let admin = app.setup_admin().await;
    assert_eq!(
        app.create_user(&admin, "bob", "bob-password", "user")
            .await
            .status,
        StatusCode::CREATED
    );
    let r = app
        .create_user(&admin, "carol", "carol-password", "user")
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json["code"], "seat_limit");
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("allows 2 users"));
    // Existing users keep working.
    assert_eq!(
        app.login("bob", "bob-password").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn require_feature_names_the_missing_feature() {
    let app = app().await;
    let err = fleetops_api::authz::require_feature(&app.state, Feature::AuditExport).unwrap_err();
    assert_eq!(err.code(), "license_required");
    assert!(err.to_string().contains("audit_export"));
}
