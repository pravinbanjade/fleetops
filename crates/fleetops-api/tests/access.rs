mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

async fn two_envs() -> (TestApp, Session, Session) {
    let t = app().await;
    let admin = t.with_local().await;
    t.add_env(&admin, "other").await;
    t.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    (t, admin, bob)
}

#[tokio::test]
async fn a_user_sees_only_environments_they_are_granted() {
    let (t, admin, bob) = two_envs().await;
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&bob))).await;
    assert_eq!(r.json, json!([]));
    t.grant("bob", "local", "viewer", None).await;
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&bob))).await;
    let names: Vec<_> = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].clone())
        .collect();
    assert_eq!(names, [json!("local")]);
    assert_eq!(
        r.json[0]["access"],
        json!({"role": "viewer", "namespaces": null})
    );
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
    assert_eq!(r.json[0]["access"]["role"], "operator");
}

#[tokio::test]
async fn an_ungranted_environment_looks_missing() {
    let (t, _admin, bob) = two_envs().await;
    for path in ["/api/v1/envs/other/status", "/api/v1/envs/other/containers"] {
        let r = t.send(req("GET", path, None, Some(&bob))).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.json["message"], "not found: environment 'other'");
    }
    assert!(!t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn a_viewer_reads_but_cannot_change_and_the_refusal_is_audited() {
    let (t, _admin, bob) = two_envs().await;
    t.grant("bob", "local", "viewer", None).await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json["message"], "containers.create permission required");
    assert_eq!(t.audit_actions().await[0], "access.denied");
}

#[tokio::test]
async fn an_operator_can_change_things() {
    let (t, _admin, bob) = two_envs().await;
    t.grant("bob", "local", "operator", None).await;
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_demoted_admin_without_bindings_sees_nothing() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = t
        .create_user(&admin, "root2", "root2-password", "admin")
        .await;
    let id = r.json["id"].as_i64().unwrap();
    t.send(req(
        "PATCH",
        &format!("/api/v1/users/{id}"),
        Some(json!({"role": "user"})),
        Some(&admin),
    ))
    .await;
    let s = session_from(&t.login("root2", "root2-password").await);
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&s))).await;
    assert_eq!(r.json, json!([]));
}

#[tokio::test]
async fn env_status_frames_reach_only_users_with_access() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    t.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    let addr = t.serve().await;
    let mut ws_bob = Ws::open(addr, &bob).await;
    let mut ws_admin = Ws::open(addr, &admin).await;
    let _ = t
        .state
        .hubs
        .status()
        .send(fleetops_api::ws::hub::StatusChange {
            env: "local".into(),
            status: json!({"state": "down", "reason": "x"}),
        });
    assert_eq!(
        ws_admin.until(|m| m["type"] == "env_status").await["env"],
        "local"
    );
    // Bob hears nothing.
    ws_bob.quiet(200).await;
}
