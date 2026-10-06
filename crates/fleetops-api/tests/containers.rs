mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;

#[tokio::test]
async fn create_start_list_and_remove() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"name": "web", "image": "nginx:1.27"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let id = r.json["id"].as_str().unwrap().to_string();
    assert_eq!(app.fake.container(&id).unwrap().state, "running");

    let list = app
        .send(req("GET", "/api/v1/envs/local/containers", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["name"], "web");

    let running = app
        .send(req(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(
        running.status,
        StatusCode::CONFLICT,
        "docker's 409 passes through"
    );
    assert_eq!(running.json["message"], "cannot remove a running container");
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}?force=true"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn create_without_start_and_validation() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine", "start": false})),
            Some(&s),
        ))
        .await;
    assert_eq!(
        app.fake
            .container(r.json["id"].as_str().unwrap())
            .unwrap()
            .state,
        "created"
    );
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": ""})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "a", "name": "bad name"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lifecycle_actions_and_unknown_action() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c9", "db", "exited", None));
    for (action, state) in [
        ("start", "running"),
        ("pause", "paused"),
        ("unpause", "running"),
        ("restart", "running"),
        ("stop", "exited"),
        ("kill", "exited"),
    ] {
        let r = app
            .send(req(
                "POST",
                &format!("/api/v1/envs/local/containers/c9/{action}"),
                None,
                Some(&s),
            ))
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{action}");
        assert_eq!(app.fake.container("c9").unwrap().state, state, "{action}");
    }
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c9/explode",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/missing/start",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json["code"], "upstream");
}

#[tokio::test]
async fn mutations_are_audited_with_environment_and_failures() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "exited", None));
    app.send(req(
        "POST",
        "/api/v1/envs/local/containers/c1/start",
        None,
        Some(&s),
    ))
    .await;
    app.send(req(
        "POST",
        "/api/v1/envs/local/containers/missing/stop",
        None,
        Some(&s),
    ))
    .await;
    let rows = app.store.list_audit(2, None).await.unwrap();
    assert_eq!(
        (rows[0].action.as_str(), rows[0].result.as_str()),
        ("container.stop", "failure")
    );
    assert!(rows[0].detail.as_ref().unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("No such container"));
    assert_eq!(
        (rows[1].action.as_str(), rows[1].result.as_str()),
        ("container.start", "success")
    );
    assert!(rows[1].environment_id.is_some());
    assert_eq!(rows[1].target_id.as_deref(), Some("c1"));
}

#[tokio::test]
async fn inspect_spec_rename() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["Name"], "/web");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/spec",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["name"], "web");
    assert_eq!(r.json["image"], "alpine:3.20");
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/rename",
            Some(json!({"name": "web2"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.fake.container("c1").unwrap().name, "web2");
}

#[tokio::test]
async fn recreate_replaces_container() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/recreate",
            Some(json!({"image": "nginx:2"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let new_id = r.json["id"].as_str().unwrap();
    assert_eq!(r.json["old_removed"], true);
    let new = app.fake.container(new_id).unwrap();
    assert_eq!(
        (new.name.as_str(), new.image.as_str(), new.state.as_str()),
        ("web", "nginx:2", "running")
    );
    assert!(app.fake.container("c1").is_none());
}

#[tokio::test]
async fn duplicate_creates_and_starts_copy() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/duplicate",
            Some(json!({"name": "web-copy"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let copy = app.fake.container(r.json["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        (copy.name.as_str(), copy.state.as_str()),
        ("web-copy", "running")
    );
    let dup = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/duplicate",
            Some(json!({"name": "web-copy"})),
            Some(&s),
        ))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn prune_removes_stopped() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "a", "exited", None));
    app.fake.add(summary("c2", "b", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/prune",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["deleted"], json!(["c1"]));
    assert!(app.fake.container("c2").is_some());
}

#[tokio::test]
async fn files_list_and_download() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/files?path=/etc",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["entries"][0]["name"], "hosts");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/files/download?path=/etc/ho%22sts",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json, json!("hello"));
    assert_eq!(
        r.headers.get("content-disposition").unwrap(),
        "attachment; filename=\"hosts\""
    );
    assert_eq!(
        r.headers.get("content-type").unwrap(),
        "application/octet-stream"
    );
}

#[tokio::test]
async fn users_can_manage_containers_unauthenticated_cannot() {
    let app = app().await;
    let admin = app.with_local().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    app.grant("bob", "local", "operator", None).await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = app
        .send(req("GET", "/api/v1/envs/local/containers", None, None))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let no_csrf = Session {
        csrf: String::new(),
        ..bob
    };
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&no_csrf),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unknown_environment_is_404() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req("GET", "/api/v1/envs/nope/containers", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.json["message"].as_str().unwrap().contains("nope"));
}
