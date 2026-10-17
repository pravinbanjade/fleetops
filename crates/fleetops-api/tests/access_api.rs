mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

async fn setup() -> (TestApp, Session, i64) {
    let t = app().await;
    let admin = t.with_local().await;
    t.add_kube_env(&admin, "k").await;
    let r = t.create_user(&admin, "bob", "bob-password", "user").await;
    (t, admin, r.json["id"].as_i64().unwrap())
}

fn env_id(t: &TestApp, name: &str) -> i64 {
    t.state.envs.get(name).unwrap().id()
}

#[tokio::test]
async fn an_admin_grants_lists_and_revokes() {
    let (t, admin, bob) = setup().await;
    let k = env_id(&t, "k");
    let r = t
        .send(req(
            "PUT",
            &format!("/api/v1/users/{bob}/access/{k}"),
            Some(json!({"role": "operator", "namespaces": ["b", "a", "a"]})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(
        r.json,
        json!({"env_id": k, "env_name": "k", "env_kind": "kube-config", "role": "operator", "role_id": null, "role_name": null, "namespaces": ["a", "b"]})
    );
    let r = t
        .send(req(
            "GET",
            &format!("/api/v1/users/{bob}/access"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.json.as_array().unwrap().len(), 1);
    let r = t
        .send(req("GET", "/api/v1/envs/k/access", None, Some(&admin)))
        .await;
    assert_eq!(
        r.json,
        json!([{"user_id": bob, "username": "bob", "role": "operator", "role_id": null, "role_name": null, "namespaces": ["a", "b"]}])
    );
    let r = t
        .send(req(
            "DELETE",
            &format!("/api/v1/users/{bob}/access/{k}"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = t
        .send(req(
            "GET",
            &format!("/api/v1/users/{bob}/access"),
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.json, json!([]));
    let actions = t.audit_actions().await;
    assert_eq!(&actions[..2], ["access.revoke", "access.grant"]);
}

#[tokio::test]
async fn bad_grants_are_refused() {
    let (t, admin, bob) = setup().await;
    let (k, local) = (env_id(&t, "k"), env_id(&t, "local"));
    let cases = [
        (
            format!("/api/v1/users/{bob}/access/{local}"),
            json!({"role": "viewer", "namespaces": ["a"]}),
            400,
            "Kubernetes",
        ),
        (
            format!("/api/v1/users/{bob}/access/{k}"),
            json!({"role": "viewer", "namespaces": []}),
            400,
            "at least one",
        ),
        (
            format!("/api/v1/users/{bob}/access/{k}"),
            json!({"role": "viewer", "namespaces": ["Not_OK"]}),
            400,
            "Not_OK",
        ),
        (
            format!("/api/v1/users/{bob}/access/{k}"),
            json!({"role": "admin"}),
            422,
            "",
        ),
        (
            format!("/api/v1/users/1/access/{k}"),
            json!({"role": "viewer"}),
            400,
            "admins already have full access",
        ),
        (
            format!("/api/v1/users/999/access/{k}"),
            json!({"role": "viewer"}),
            404,
            "",
        ),
        (
            format!("/api/v1/users/{bob}/access/999"),
            json!({"role": "viewer"}),
            404,
            "",
        ),
    ];
    for (path, body, status, text) in cases {
        let r = t
            .send(req("PUT", &path, Some(body.clone()), Some(&admin)))
            .await;
        assert_eq!(r.status.as_u16(), status, "{path} {body}: {:?}", r.json);
        assert!(
            r.json["message"].as_str().unwrap_or("").contains(text),
            "{:?}",
            r.json
        );
    }
}

#[tokio::test]
async fn only_admins_manage_access() {
    let (t, _admin, bob) = setup().await;
    let s = session_from(&t.login("bob", "bob-password").await);
    let k = env_id(&t, "k");
    let r = t
        .send(req(
            "PUT",
            &format!("/api/v1/users/{bob}/access/{k}"),
            Some(json!({"role": "operator"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = t
        .send(req("GET", "/api/v1/envs/k/access", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn community_refuses_a_custom_role() {
    let (t, admin, bob) = setup().await;
    let k = env_id(&t, "k");
    let path = format!("/api/v1/users/{bob}/access/{k}");
    let r = t
        .send(req("PUT", &path, Some(json!({"role_id": 1})), Some(&admin)))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    assert_eq!(r.json["code"], "license_required");
    for body in [json!({}), json!({"role": "viewer", "role_id": 1})] {
        let r = t.send(req("PUT", &path, Some(body), Some(&admin))).await;
        assert_eq!(r.status, 400, "{:?}", r.json);
        assert_eq!(r.json["message"], "invalid input: give a role or a role_id");
    }
}
