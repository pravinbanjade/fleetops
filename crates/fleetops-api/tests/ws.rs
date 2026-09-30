mod common;

use common::*;
use serde_json::json;

#[tokio::test]
async fn websocket_requires_a_session_and_a_matching_origin() {
    let t = app().await;
    let admin = t.with_local().await;
    let addr = t.serve().await;
    let err = ws_connect(addr, None, None).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    let err = ws_connect(addr, Some(&admin), Some("http://evil.example"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
    let ok = format!("http://{addr}");
    assert!(ws_connect(addr, Some(&admin), Some(&ok)).await.is_ok());
}

#[tokio::test]
async fn bad_frames_get_errors_without_closing_the_socket() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send_text("not json").await;
    let e = ws.json().await;
    assert_eq!(
        (e["type"].as_str(), e["code"].as_str()),
        (Some("error"), Some("invalid"))
    );
    assert!(e["sub"].is_null());
    ws.send_binary(vec![1, 2]).await;
    assert_eq!(ws.json().await["code"], "invalid");
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events"}))
        .await;
    let e = ws.json().await;
    assert!(
        is(&e, 1, "error") && e["code"] == "invalid",
        "env is required: {e}"
    );
    ws.send(json!({"op": "sub", "sub": 2, "topic": "events", "env": "nope"}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 2, "error") && e["code"] == "not_found", "{e}");
    // Still usable.
    ws.send(json!({"op": "sub", "sub": 5, "topic": "events", "env": "local"}))
        .await;
    ws.quiet(100).await;
}

#[tokio::test]
async fn sub_ids_are_unique_per_connection_and_reusable_after_unsub() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    let sub = json!({"op": "sub", "sub": 9, "topic": "events", "env": "local"});
    ws.send(sub.clone()).await;
    ws.send(sub.clone()).await;
    let e = ws.json().await;
    assert!(is(&e, 9, "error") && e["code"] == "conflict", "{e}");
    ws.send(json!({"op": "unsub", "sub": 9})).await;
    ws.send(sub).await;
    ws.quiet(100).await;
}

#[tokio::test]
async fn unreachable_environment_is_env_unavailable() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake
        .reachable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events", "env": "local"}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 1, "error") && e["code"] == "env_unavailable", "{e}");
}

#[tokio::test]
async fn logging_out_closes_open_websockets() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    let r = t
        .send(req("POST", "/api/v1/auth/logout", None, Some(&admin)))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    let e = ws.json().await;
    assert_eq!(
        (e["type"].as_str(), e["code"].as_str()),
        (Some("error"), Some("unauthorized"))
    );
    assert!(ws.frame().await.is_none(), "socket closes after the error");
}

#[tokio::test]
async fn deleted_user_loses_the_websocket() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let r = t.create_user(&admin, "bob", "bob-password", "user").await;
    let bob_id = r.json["id"].as_i64().unwrap();
    let bob = session_from(&t.login("bob", "bob-password").await);
    let mut ws = Ws::open(t.serve().await, &bob).await;
    let r = t
        .send(req(
            "DELETE",
            &format!("/api/v1/users/{bob_id}"),
            None,
            Some(&admin),
        ))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    assert_eq!(ws.json().await["code"], "unauthorized");
}

#[tokio::test]
async fn unresponsive_client_is_dropped_and_releases_its_streams() {
    // A laptop that slept on a network change leaves a half-open socket:
    // it never answers pings, so the server must close it.
    let t = app_with(fleetops_api::ApiConfig {
        ws_ping_interval: std::time::Duration::from_millis(50),
        ..ws_config()
    })
    .await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, serde_json::Value::Null))
        .await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    // Stop reading: no pongs go back.
    eventually("zombie socket closed", || t.fake.open_event_streams() == 0).await;
    drop(ws);
}

#[tokio::test]
async fn a_client_that_answers_pings_stays_connected() {
    let t = app_with(fleetops_api::ApiConfig {
        ws_ping_interval: std::time::Duration::from_millis(50),
        ..ws_config()
    })
    .await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, serde_json::Value::Null))
        .await;
    // Reading lets the client library answer each ping.
    ws.quiet(400).await;
    assert_eq!(t.fake.open_event_streams(), 1);
}
