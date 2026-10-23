mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;

#[tokio::test]
async fn system_info_and_df() {
    let app = app().await;
    let s = app.with_local().await;
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/info", None, Some(&s)))
            .await
            .json["NCPU"],
        2
    );
    assert!(app
        .send(req("GET", "/api/v1/envs/local/df", None, Some(&s)))
        .await
        .json
        .is_object());
}

#[tokio::test]
async fn recent_events_come_from_docker_since_the_requested_time() {
    let app = app().await;
    let s = app.with_local().await;
    let now = fleetops_core::now_unix();
    app.fake.state.lock().unwrap().past_events = vec![
        fake_event("die", "c1", now - 7200),
        fake_event("die", "c2", now - 60),
        fake_event("start", "c2", now - 59),
    ];
    let r = app
        .send(req(
            "GET",
            &format!("/api/v1/envs/local/events?since={}", now - 120),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let got: Vec<(&str, &str)> = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["action"].as_str().unwrap(), e["id"].as_str().unwrap()))
        .collect();
    assert_eq!(got, [("die", "c2"), ("start", "c2")]);
}

#[tokio::test]
async fn recent_events_look_back_at_most_an_hour() {
    let app = app().await;
    let s = app.with_local().await;
    let now = fleetops_core::now_unix();
    app.fake.state.lock().unwrap().past_events = vec![
        fake_event("die", "old", now - 7200),
        fake_event("die", "recent", now - 3000),
    ];
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/events?since=0",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json.as_array().unwrap().len(), 1);
    assert_eq!(r.json[0]["id"], "recent");
}

#[tokio::test]
async fn recent_events_need_a_session() {
    let app = app().await;
    app.with_local().await;
    let r = app
        .send(req("GET", "/api/v1/envs/local/events?since=0", None, None))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

fn fake_event(action: &str, id: &str, time: i64) -> fleetops_core::streams::DockerEvent {
    fleetops_core::streams::DockerEvent {
        kind: "container".into(),
        action: action.into(),
        id: id.into(),
        name: Some(format!("{id}-name")),
        attributes: Default::default(),
        time,
        scope: None,
    }
}

#[tokio::test]
async fn images_routes() {
    let app = app().await;
    let s = app.with_local().await;
    let list = app
        .send(req("GET", "/api/v1/envs/local/images", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["repo_tags"], json!(["alpine:3.20"]));
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:nope",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert!(app
        .send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine/history",
            None,
            Some(&s)
        ))
        .await
        .json
        .is_array());
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/images/sha256:alpine/tag",
            Some(json!({"repo": "me/alpine"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = app
        .send(req(
            "DELETE",
            "/api/v1/envs/local/images/sha256:alpine?force=true",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["removed"], json!(["sha256:alpine"]));
    app.send(req(
        "POST",
        "/api/v1/envs/local/images/prune?all=true",
        None,
        Some(&s),
    ))
    .await;
    app.send(req(
        "POST",
        "/api/v1/envs/local/images/prune",
        None,
        Some(&s),
    ))
    .await;
    let calls = app.fake.calls();
    assert!(calls.contains(&"tag sha256:alpine me/alpine:latest".to_string()));
    assert!(calls.contains(&"prune images all=true".to_string()));
    assert!(calls.contains(&"prune images all=false".to_string()));
}

#[tokio::test]
async fn volumes_routes() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/volumes",
            Some(json!({"name": "data", "labels": {"a": "b"}})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    assert_eq!(r.json["driver"], "local");
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/volumes", None, Some(&s)))
            .await
            .json[0]["name"],
        "data"
    );
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .json["Name"],
        "data"
    );
    assert_eq!(
        app.send(req(
            "DELETE",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(req(
            "DELETE",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/volumes/prune",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn networks_routes() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/networks",
            Some(json!({"name": "backend", "subnet": "10.1.0.0/24", "attachable": true})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let id = r.json["id"].as_str().unwrap().to_string();
    let list = app
        .send(req("GET", "/api/v1/envs/local/networks", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["subnet"], "10.1.0.0/24");
    assert_eq!(
        app.send(req(
            "GET",
            &format!("/api/v1/envs/local/networks/{id}"),
            None,
            Some(&s)
        ))
        .await
        .json["Name"],
        "backend"
    );
    let r = app
        .send(req(
            "POST",
            &format!("/api/v1/envs/local/networks/{id}/connect"),
            Some(json!({"container": "c1"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = app
        .send(req(
            "POST",
            &format!("/api/v1/envs/local/networks/{id}/disconnect"),
            Some(json!({"container": "c1", "force": true})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(app
        .fake
        .calls()
        .contains(&format!("disconnect {id} c1 force=true")));
    assert_eq!(
        app.send(req(
            "DELETE",
            &format!("/api/v1/envs/local/networks/{id}"),
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/networks/prune",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    for a in [
        "network.create",
        "network.connect",
        "network.disconnect",
        "network.remove",
        "network.prune",
    ] {
        assert!(actions.contains(&a.to_string()), "{a} not audited");
    }
}
