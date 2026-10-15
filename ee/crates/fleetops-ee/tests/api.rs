mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_core::now_unix;
use fleetops_ee::license::GRACE_SECS;
use fleetops_store::NewAudit;
use serde_json::json;

const DAY: i64 = 86_400;

fn row<'a>(ts: i64, target_id: &'a str, result: &'a str) -> NewAudit<'a> {
    NewAudit {
        ts,
        user_id: None,
        environment_id: None,
        action: "container.stop",
        target_type: Some("container"),
        target_id: Some(target_id),
        result,
        detail: None,
    }
}

#[tokio::test]
async fn unlicensed_enterprise_has_no_features() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let r = ee.send("GET", "/api/v1/edition", None, Some(&admin)).await;
    assert_eq!(r.json(), json!({"edition": "enterprise", "features": []}));
    let r = ee
        .send("GET", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "none");
    let r = ee
        .send("GET", "/api/v1/ee/audit/export", None, Some(&admin))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["code"], "license_required");
}

#[tokio::test]
async fn admin_installs_a_license_and_features_turn_on() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let r = ee
        .install(
            &admin,
            &license(&k, now_unix() + 30 * DAY, Some(5), &["audit_export"]),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let info = r.json();
    assert_eq!(info["status"], "valid");
    assert_eq!(info["customer"], "Acme");
    assert_eq!(info["max_users"], 5);
    assert_eq!(info["users_in_use"], 1);
    let r = ee.send("GET", "/api/v1/edition", None, Some(&admin)).await;
    assert_eq!(r.json()["features"], json!(["audit_export"]));
    assert!(ee.actions().await.contains(&"license.update".to_string()));
}

#[tokio::test]
async fn upload_trims_whitespace() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let text = format!("\r\n  {}  \n\n", license(&k, now_unix() + DAY, None, &[]));
    assert_eq!(ee.install(&admin, &text).await.status, StatusCode::OK);
}

#[tokio::test]
async fn bad_or_expired_uploads_are_refused_and_keep_the_stored_license() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.install(
        &admin,
        &license(&k, now_unix() + DAY, None, &["audit_export"]),
    )
    .await;
    let r = ee
        .install(&admin, &license(&key(), now_unix() + DAY, None, &[]))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["code"], "invalid");
    assert!(r.json()["message"]
        .as_str()
        .unwrap()
        .contains("license: signature does not verify"));
    let r = ee
        .install(&admin, &license(&k, now_unix() - DAY, None, &[]))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["message"]
        .as_str()
        .unwrap()
        .contains("license: expired on"));
    let r = ee
        .send("GET", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "valid");
}

#[tokio::test]
async fn non_admins_cannot_touch_the_license() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.install(
        &admin,
        &license(&k, now_unix() + DAY, None, &["audit_export"]),
    )
    .await;
    ee.user(&admin, "bob").await;
    let bob = ee.login("bob").await;
    for (m, uri) in [
        ("GET", "/api/v1/ee/license"),
        ("DELETE", "/api/v1/ee/license"),
        ("GET", "/api/v1/ee/audit/export"),
    ] {
        let r = ee.send(m, uri, None, Some(&bob)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{m} {uri}");
        assert!(!r.text.contains("Acme"));
        assert_eq!(
            ee.send(m, uri, None, None).await.status,
            StatusCode::UNAUTHORIZED,
            "{m} {uri}"
        );
    }
    let r = ee
        .install(&bob, &license(&k, now_unix() + DAY, None, &[]))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_falls_back_to_file_then_none() {
    let k = key();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fleetops.lic");
    std::fs::write(&file, license(&k, now_unix() + DAY, None, &[])).unwrap();
    let ee = ee_app(&k, Some(file.clone())).await;
    let admin = ee.admin().await;
    ee.install(
        &admin,
        &license(&k, now_unix() + DAY, None, &["audit_export"]),
    )
    .await;
    let r = ee
        .send("DELETE", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["status"], "valid");
    assert_eq!(
        r.json()["features"],
        json!([]),
        "back on the file's license"
    );
    std::fs::remove_file(&file).unwrap();
    let r = ee
        .send("DELETE", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "none");
    assert!(ee.actions().await.contains(&"license.remove".to_string()));
}

#[tokio::test]
async fn invalid_file_starts_with_features_off() {
    let k = key();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fleetops.lic");
    std::fs::write(&file, "garbage").unwrap();
    let ee = ee_app(&k, Some(file)).await;
    let admin = ee.admin().await;
    let r = ee
        .send("GET", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "invalid");
    assert_eq!(r.json()["reason"], "not a FleetOps license");
}

#[tokio::test]
async fn grace_keeps_features_and_expiry_drops_them() {
    let k = key();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fleetops.lic");
    std::fs::write(
        &file,
        license(&k, now_unix() - DAY, None, &["audit_export"]),
    )
    .unwrap();
    let ee = ee_app(&k, Some(file.clone())).await;
    let admin = ee.admin().await;
    let r = ee
        .send("GET", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "grace");
    let r = ee
        .send("GET", "/api/v1/ee/audit/export", None, Some(&admin))
        .await;
    assert_eq!(r.status, StatusCode::OK);

    let expired = license(&k, now_unix() - GRACE_SECS - 1, None, &["audit_export"]);
    std::fs::write(&file, expired).unwrap();
    let ee = ee_app(&k, Some(file)).await;
    let admin = ee.admin().await;
    let r = ee
        .send("GET", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(r.json()["status"], "expired");
    let r = ee
        .send("GET", "/api/v1/ee/audit/export", None, Some(&admin))
        .await;
    assert_eq!(r.json()["code"], "license_required");
}

#[tokio::test]
async fn seat_limit_blocks_new_users_only() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.user(&admin, "bob").await;
    ee.user(&admin, "carol").await;
    ee.install(&admin, &license(&k, now_unix() + DAY, Some(2), &[]))
        .await;
    let r = ee.user(&admin, "dave").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["code"], "seat_limit");
    ee.login("carol").await; // over the limit, still works
    ee.send("DELETE", "/api/v1/ee/license", None, Some(&admin))
        .await;
    assert_eq!(ee.user(&admin, "dave").await.status, StatusCode::CREATED);
}

#[tokio::test]
async fn export_csv_and_ndjson_with_range() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.install(
        &admin,
        &license(&k, now_unix() + DAY, None, &["audit_export"]),
    )
    .await;
    for ts in [100, 200, 300] {
        ee.store
            .insert_audit(&row(ts, "web", "success"))
            .await
            .unwrap();
    }
    let r = ee
        .send(
            "GET",
            "/api/v1/ee/audit/export?format=csv&from=150&to=300",
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "text/csv; charset=utf-8");
    assert!(r.headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment; filename=\"fleetops-audit-"));
    let lines: Vec<&str> = r.text.lines().collect();
    assert_eq!(
        lines[0],
        "id,ts,user_id,username,environment_id,action,target_type,target_id,result,detail"
    );
    assert_eq!(lines.len(), 2, "{}", r.text);
    assert!(lines[1].contains(",200,,,,container.stop,container,web,success,"));
    let r = ee
        .send(
            "GET",
            "/api/v1/ee/audit/export?format=ndjson",
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.headers["content-type"], "application/x-ndjson");
    let rows: Vec<serde_json::Value> = r
        .text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(rows.iter().any(|v| v["ts"] == 300));
    assert!(ee.actions().await.contains(&"audit.export".to_string()));
}

#[tokio::test]
async fn export_csv_neutralises_formulas_and_quotes() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    ee.install(
        &admin,
        &license(&k, now_unix() + DAY, None, &["audit_export"]),
    )
    .await;
    ee.store
        .insert_audit(&row(1, "=HYPERLINK(\"x\")", "a,b"))
        .await
        .unwrap();
    let r = ee
        .send(
            "GET",
            "/api/v1/ee/audit/export?format=csv&to=2",
            None,
            Some(&admin),
        )
        .await;
    let line = r.text.lines().nth(1).unwrap();
    assert!(
        line.contains(",\"'=HYPERLINK(\"\"x\"\")\",\"a,b\","),
        "{line}"
    );
}

#[tokio::test]
async fn served_document_lists_enterprise_routes() {
    let k = key();
    let ee = ee_app(&k, None).await;
    let doc = ee
        .send("GET", "/api/v1/openapi.json", None, None)
        .await
        .json();
    assert!(doc["paths"]["/api/v1/ee/license"]["put"].is_object());
    assert!(doc["paths"]["/api/v1/ee/audit/export"]["get"].is_object());
}
