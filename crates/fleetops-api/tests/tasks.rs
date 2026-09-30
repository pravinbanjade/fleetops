mod common;

use axum::{body::Body, extract::ConnectInfo, http::Request, http::StatusCode};
use common::*;
use fleetops_core::streams::BuildContext;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;

async fn pull(t: &TestApp, s: &Session, image: &str) -> Resp {
    t.send(req(
        "POST",
        "/api/v1/envs/local/images/pull",
        Some(json!({"image": image})),
        Some(s),
    ))
    .await
}

async fn wait_task(t: &TestApp, s: &Session, id: &str) -> Value {
    for _ in 0..200 {
        let r = t
            .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(s)))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
        if r.json["status"] != "running" {
            return r.json;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("task {id} did not finish");
}

/// A multipart/form-data request with text or file parts.
fn multipart(uri: &str, parts: &[(&str, &[u8])], s: &Session) -> Request<Body> {
    let boundary = "fleetopsboundary7";
    let mut body = Vec::new();
    for (name, data) in parts {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend(*data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").as_bytes());
    let mut r = Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", &s.cookie)
        .header("x-csrf-token", &s.csrf)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    r
}

#[tokio::test]
async fn pull_runs_as_a_task_and_is_audited() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = pull(&t, &admin, "nginx:1.27").await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    let id = r.json["task_id"].as_str().unwrap().to_string();
    let done = wait_task(&t, &admin, &id).await;
    assert_eq!(done["status"], "succeeded");
    assert_eq!(
        (done["kind"].as_str(), done["target"].as_str()),
        (Some("image.pull"), Some("nginx:1.27"))
    );
    assert_eq!(done["env"], "local");
    assert!(done["finished_at"].is_i64());
    assert_eq!(t.fake.streams.lock().unwrap().pulls, ["nginx:1.27"]);
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let row = audit
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "image.pull")
        .unwrap()
        .clone();
    assert_eq!(
        (row["result"].as_str(), row["target_id"].as_str()),
        (Some("success"), Some("nginx:1.27"))
    );
    assert!(row["environment_id"].is_i64());
}

#[tokio::test]
async fn failed_pull_fails_the_task_with_dockers_message() {
    let t = app().await;
    let admin = t.with_local().await;
    let id = pull(&t, &admin, "missing").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let done = wait_task(&t, &admin, &id).await;
    assert_eq!(done["status"], "failed");
    assert!(
        done["error"].as_str().unwrap().contains("manifest unknown"),
        "{done}"
    );
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let row = audit
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "image.pull")
        .unwrap()
        .clone();
    assert_eq!(row["result"], "failure");
    assert!(row["detail"]["error"]
        .as_str()
        .unwrap()
        .contains("manifest unknown"));
}

#[tokio::test]
async fn pull_input_is_validated() {
    let t = app().await;
    let admin = t.with_local().await;
    for bad in ["", "has space", "nginx:"] {
        let r = pull(&t, &admin, bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}: {:?}", r.json);
    }
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine", "tag": "3.20"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let id = r.json["task_id"].as_str().unwrap().to_string();
    wait_task(&t, &admin, &id).await;
    assert_eq!(t.fake.streams.lock().unwrap().pulls, ["alpine:3.20"]);
}

#[tokio::test]
async fn task_topic_replays_the_backlog_then_follows_to_the_end() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.hold_pulls.store(true, Ordering::SeqCst);
    let id = pull(&t, &admin, "alpine").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Let the first progress message land before subscribing.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "task", "id": id}))
        .await;
    let first = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(
        first["data"]["status"], "Pulling from alpine",
        "backlog first"
    );
    t.fake.release_pulls.notify_one();
    let mut statuses = Vec::new();
    let end = loop {
        let v = ws.json().await;
        if is(&v, 1, "end") {
            break v;
        }
        statuses.push(v["data"]["status"].as_str().unwrap().to_string());
    };
    assert_eq!(
        statuses.last().unwrap(),
        "Status: Downloaded newer image for alpine:latest"
    );
    assert_eq!(end["reason"], "succeeded");
    assert_eq!(end["task"]["id"], id.as_str());
    // A finished task still replays, then ends at once.
    ws.send(json!({"op": "sub", "sub": 2, "topic": "task", "id": id}))
        .await;
    let mut n = 0;
    loop {
        let v = ws.json().await;
        if is(&v, 2, "end") {
            break;
        }
        n += 1;
    }
    assert_eq!(n, 3);
}

#[tokio::test]
async fn tasks_are_private_to_their_owner_and_admins() {
    let t = app().await;
    let admin = t.with_local().await;
    t.create_user(&admin, "bob", "bob-password", "user").await;
    t.create_user(&admin, "eve", "eve-password", "user").await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    let eve = session_from(&t.login("eve", "eve-password").await);
    let id = pull(&t, &bob, "alpine").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_task(&t, &bob, &id).await;
    wait_task(&t, &admin, &id).await;
    let r = t
        .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(&eve)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let mut ws = Ws::open(t.serve().await, &eve).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "task", "id": id}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 1, "error") && e["code"] == "not_found", "{e}");
    let r = t
        .send(req("GET", "/api/v1/tasks/nope", None, Some(&bob)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn too_many_running_tasks_is_a_conflict() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.hold_pulls.store(true, Ordering::SeqCst);
    for _ in 0..fleetops_api::tasks::MAX_RUNNING_TASKS {
        assert_eq!(
            pull(&t, &admin, "alpine").await.status,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        pull(&t, &admin, "alpine").await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn build_from_an_uploaded_dockerfile_or_context() {
    let t = app().await;
    let admin = t.with_local().await;
    let uri = "/api/v1/envs/local/images/build";
    let r = t
        .send(multipart(
            uri,
            &[("dockerfile", b"FROM alpine\n"), ("tag", b"app:1")],
            &admin,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    let done = wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    assert_eq!(
        (done["status"].as_str(), done["target"].as_str()),
        (Some("succeeded"), Some("app:1"))
    );
    let r = t
        .send(multipart(
            uri,
            &[
                ("context", b"tar-bytes"),
                ("dockerfile_path", b"docker/Dockerfile"),
            ],
            &admin,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    let builds = t.fake.streams.lock().unwrap().builds.clone();
    assert_eq!(
        builds[0].context,
        BuildContext::Dockerfile(b"FROM alpine\n".to_vec())
    );
    assert_eq!(builds[0].tag.as_deref(), Some("app:1"));
    assert_eq!(builds[1].context, BuildContext::Tar(b"tar-bytes".to_vec()));
    assert_eq!(builds[1].dockerfile.as_deref(), Some("docker/Dockerfile"));
    assert_eq!(builds[1].tag, None);
}

#[tokio::test]
async fn failing_build_step_fails_the_task() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = t
        .send(multipart(
            "/api/v1/envs/local/images/build",
            &[("dockerfile", b"RUN fail")],
            &admin,
        ))
        .await;
    let done = wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert!(
        done["error"].as_str().unwrap().contains("non-zero code"),
        "{done}"
    );
}

#[tokio::test]
async fn build_upload_is_validated() {
    let t = app().await;
    let admin = t.with_local().await;
    let uri = "/api/v1/envs/local/images/build";
    for parts in [
        &[][..],
        &[("tag", &b"x:1"[..])][..],
        &[("dockerfile", &b"FROM a"[..]), ("context", &b"t"[..])][..],
        &[("dockerfile", &b"FROM a"[..]), ("tag", &b"bad tag"[..])][..],
        &[("surprise", &b"?"[..])][..],
    ] {
        let r = t.send(multipart(uri, parts, &admin)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json);
        assert_eq!(r.json["code"], "invalid");
    }
    // Mutations need CSRF like every other route.
    let mut r = multipart(uri, &[("dockerfile", b"FROM a")], &admin);
    r.headers_mut().remove("x-csrf-token");
    assert_eq!(t.send(r).await.status, StatusCode::FORBIDDEN);
}
