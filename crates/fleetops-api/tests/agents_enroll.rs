mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

const ORIGIN: &str = "https://fleet.example.com";

fn with_origin(
    mut r: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    r.headers_mut().insert("origin", ORIGIN.parse().unwrap());
    r
}

async fn add_agent(t: &TestApp, admin: &Session, name: &str) -> Resp {
    t.send(with_origin(req(
        "POST",
        "/api/v1/envs",
        Some(json!({"name": name, "kind": "docker-agent"})),
        Some(admin),
    )))
    .await
}

#[tokio::test]
async fn creating_an_agent_environment_returns_a_one_time_command() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = add_agent(&t, &admin, "web-01").await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    assert_eq!(r.json["kind"], "docker-agent");
    assert_eq!(r.json["url"], format!("agent://{}", r.json["id"]));
    let token = r.json["enrollment"]["join_token"].as_str().unwrap();
    assert!(token.starts_with("fj_") && token.len() == 67, "{token}");
    let cmd = r.json["enrollment"]["command"].as_str().unwrap();
    assert!(cmd.contains(&format!("--server {ORIGIN}")), "{cmd}");
    // Re-running the command on a host (after a rotate) replaces the old agent.
    assert!(cmd.starts_with("docker rm -f fleetops-agent"), "{cmd}");
    assert!(cmd.contains(&format!("--join-token {token}")), "{cmd}");
    assert!(
        cmd.contains(&format!(
            "docker.io/fleets/fleetops-agent:{}",
            env!("CARGO_PKG_VERSION")
        )),
        "{cmd}"
    );

    // The token is never shown again.
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let row = &list.json[0];
    assert_eq!(row["name"], "web-01");
    assert_eq!(row["agent"]["connected"], false);
    assert_eq!(row["agent"]["enrolled"], false);
    assert!(!list.json.to_string().contains(token));
}

#[tokio::test]
async fn agent_and_local_inputs_are_validated() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "a", "kind": "docker-agent", "url": "unix:///x"})),
            Some(&admin),
        )))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "agents take no URL");
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "b", "kind": "docker-local"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "local needs a URL");
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "c", "kind": "docker-local", "url": "agent://1"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "agent:// is server-made");
}

#[tokio::test]
async fn an_agent_url_cannot_be_edited() {
    let t = app().await;
    let admin = t.setup_admin().await;
    add_agent(&t, &admin, "web-01").await;
    let r = t
        .send(req(
            "PATCH",
            "/api/v1/envs/web-01",
            Some(json!({"url": "unix:///x.sock"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = t
        .send(req(
            "PATCH",
            "/api/v1/envs/web-01",
            Some(json!({"name": "web-1"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "renaming is fine: {:?}", r.json);
}

#[tokio::test]
async fn rotating_issues_a_new_token_and_is_admin_only() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let first = add_agent(&t, &admin, "web-01").await.json["enrollment"]["join_token"].clone();
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/web-01/agent/rotate",
            None,
            Some(&admin),
        )))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_ne!(r.json["join_token"], first);
    assert!(r.json["command"]
        .as_str()
        .unwrap()
        .contains("--join-token fj_"));

    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/web-01/agent/rotate",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/nope/agent/rotate",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rotating_a_local_environment_is_rejected() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/agent/rotate",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
