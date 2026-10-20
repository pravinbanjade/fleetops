mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use fleetops_api::stacks::Timing;
use fleetops_api::ApiConfig;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::Duration;

const BASE: &str = "/api/v1/envs/local/stacks";
const WEB: &str =
    "services:\n  web:\n    image: nginx:1.27\n    volumes: ['data:/d']\nvolumes:\n  data:\n";

async fn fast_app() -> TestApp {
    app_with(ApiConfig {
        stack_timing: Timing {
            exit_check: Duration::from_millis(5),
            poll: Duration::from_millis(1),
            health_cap: Duration::from_millis(50),
        },
        ..Default::default()
    })
    .await
}

async fn create(app: &TestApp, s: &Session, compose: &str, variables: &[&str]) -> Resp {
    app.send(req(
        "POST",
        BASE,
        Some(json!({"name": "shop", "compose": compose, "variables": variables})),
        Some(s),
    ))
    .await
}

async fn plan(app: &TestApp, s: &Session, from: Value) -> Resp {
    app.send(req(
        "POST",
        &format!("{BASE}/shop/plan"),
        Some(from),
        Some(s),
    ))
    .await
}

async fn deploy(app: &TestApp, s: &Session, from: Value, token: &str) -> Resp {
    app.send(req(
        "POST",
        &format!("{BASE}/shop/deploy"),
        Some(json!({"from": from, "plan_token": token})),
        Some(s),
    ))
    .await
}

async fn wait_task(app: &TestApp, s: &Session, id: &str) -> Value {
    for _ in 0..500 {
        let r = app
            .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(s)))
            .await;
        if r.json["status"] != "running" {
            return r.json;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("task {id} never finished");
}

/// Plans `from` and deploys it, returning the finished task.
async fn plan_and_deploy(app: &TestApp, s: &Session, from: Value) -> Value {
    let p = plan(app, s, from.clone()).await;
    assert_eq!(p.status, StatusCode::OK, "{:?}", p.json);
    let token = p.json["plan"]["token"]
        .as_str()
        .expect("a plan")
        .to_string();
    let d = deploy(app, s, from, &token).await;
    assert_eq!(d.status, StatusCode::ACCEPTED, "{:?}", d.json);
    wait_task(app, s, d.json["task_id"].as_str().unwrap()).await
}

#[tokio::test]
async fn create_save_get_round_trip() {
    let app = fast_app().await;
    let s = app.with_local().await;
    let r = create(&app, &s, WEB, &["A=1"]).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    assert_eq!(r.json["name"], "shop");
    assert_eq!(r.json["draft_differs"], true);
    let r = app
        .send(req(
            "PUT",
            &format!("{BASE}/shop/draft"),
            Some(json!({"compose": "services: {web: {image: x}}", "variables": ["B=2"]})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    let g = app
        .send(req("GET", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(g.json["draft"]["compose"], "services: {web: {image: x}}");
    assert_eq!(g.json["draft"]["variables"], json!(["B=2"]));
    assert!(g.json["deployed"].is_null());
    let l = app.send(req("GET", BASE, None, Some(&s))).await;
    assert_eq!(l.json.as_array().unwrap().len(), 1);
    let actions = app.audit_actions().await;
    assert!(
        actions.contains(&"stack.create".to_string())
            && actions.contains(&"stack.save".to_string())
    );
}

#[tokio::test]
async fn save_with_masked_values_keeps_real_ones() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &["TOKEN=hunter2"]).await;
    let masked = format!("TOKEN={}", fleetops_core::kube::MASK);
    app.send(req(
        "PUT",
        &format!("{BASE}/shop/draft"),
        Some(json!({"compose": WEB, "variables": [masked, "NEW=1"]})),
        Some(&s),
    ))
    .await;
    let g = app
        .send(req("GET", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(
        g.json["draft"]["variables"],
        json!(["TOKEN=hunter2", "NEW=1"])
    );
}

#[tokio::test]
async fn validate_reports_issues_with_paths() {
    let app = fast_app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            &format!("{BASE}/validate"),
            Some(json!({"name": "shop", "compose": "services:\n  web:\n    build: .\n"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let paths: Vec<&str> = r.json["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"services.web.build"), "{paths:?}");
    assert!(paths.contains(&"services.web.image"), "{paths:?}");
}

#[tokio::test]
async fn plan_then_deploy_runs_task_and_marks_deployed() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    let p = plan(&app, &s, json!({"source": "draft"})).await;
    assert_eq!(p.json["plan"]["services"][0]["action"], "create");
    assert!(p.json["compose_before"].is_null());
    let task = plan_and_deploy(&app, &s, json!({"source": "draft"})).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    let g = app
        .send(req("GET", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(g.json["stack"]["deployed_revision"], 1);
    assert_eq!(g.json["deployed"]["status"], "deployed");
    assert!(app.fake.container("shop-web-1").is_some());
    let l = app.send(req("GET", BASE, None, Some(&s))).await;
    assert_eq!(l.json[0]["draft_differs"], false);
    assert!(app
        .audit_actions()
        .await
        .contains(&"stack.deploy".to_string()));
}

#[tokio::test]
async fn stale_token_conflicts() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    let p = plan(&app, &s, json!({"source": "draft"})).await;
    let token = p.json["plan"]["token"].as_str().unwrap().to_string();
    app.fake
        .add(summary("cli1", "shop-web-1", "running", Some("shop")));
    let d = deploy(&app, &s, json!({"source": "draft"}), &token).await;
    assert_eq!(d.status, StatusCode::CONFLICT, "{:?}", d.json);
    assert_eq!(
        d.json["message"],
        "conflict: the stack changed since this plan was made; review again"
    );
}

#[tokio::test]
async fn concurrent_deploy_conflicts() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    app.fake.hold_pulls.store(true, Ordering::SeqCst);
    let p = plan(&app, &s, json!({"source": "draft"})).await;
    let token = p.json["plan"]["token"].as_str().unwrap().to_string();
    let first = deploy(&app, &s, json!({"source": "draft"}), &token).await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let second = deploy(&app, &s, json!({"source": "draft"}), &token).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{:?}", second.json);
    // Let the held pull go, nudging until the task has passed it.
    let id = first.json["task_id"].as_str().unwrap().to_string();
    for _ in 0..500 {
        app.fake.release_pulls.notify_waiters();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let r = app
            .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(&s)))
            .await;
        if r.json["status"] != "running" {
            assert_eq!(r.json["status"], "succeeded", "{:?}", r.json);
            return;
        }
    }
    panic!("the held deploy never finished");
}

#[tokio::test]
async fn rollback_creates_rollback_revision() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    plan_and_deploy(&app, &s, json!({"source": "draft"})).await;
    app.send(req(
        "PUT",
        &format!("{BASE}/shop/draft"),
        Some(json!({"compose": WEB.replace("1.27", "1.28")})),
        Some(&s),
    ))
    .await;
    let t = plan_and_deploy(&app, &s, json!({"source": "draft"})).await;
    assert_eq!(t["status"], "succeeded", "{t:?}");
    let p = plan(&app, &s, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(
        p.json["plan"]["services"][0]["reasons"][0],
        "image: nginx:1.28 → nginx:1.27"
    );
    let t = plan_and_deploy(&app, &s, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(t["status"], "succeeded", "{t:?}");
    let revs = app
        .send(req(
            "GET",
            &format!("{BASE}/shop/revisions"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(revs.json[0]["number"], 3);
    assert_eq!(revs.json[0]["kind"], "rollback");
    assert_eq!(revs.json[0]["rollback_of"], 1);
    assert_eq!(revs.json[0]["status"], "deployed");
    assert_eq!(revs.json[1]["status"], "superseded");
    assert!(app
        .audit_actions()
        .await
        .contains(&"stack.rollback".to_string()));
}

#[tokio::test]
async fn remove_keeps_volumes_by_default() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    plan_and_deploy(&app, &s, json!({"source": "draft"})).await;
    let r = app
        .send(req("DELETE", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(app.fake.container("shop-web-1").is_none());
    let st = app.fake.state.lock().unwrap();
    assert!(st.networks.iter().all(|n| n.name != "shop_default"));
    assert!(st.volumes.iter().any(|v| v.name == "shop_data"));
}

#[tokio::test]
async fn remove_with_volumes() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    plan_and_deploy(&app, &s, json!({"source": "draft"})).await;
    let r = app
        .send(req(
            "DELETE",
            &format!("{BASE}/shop?volumes=true"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(app
        .fake
        .state
        .lock()
        .unwrap()
        .volumes
        .iter()
        .all(|v| v.name != "shop_data"));
    let g = app
        .send(req("GET", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(g.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn oversize_compose_rejected() {
    let app = fast_app().await;
    let s = app.with_local().await;
    let big = format!("services: {{}}\n#{}", "x".repeat(1024 * 1024));
    let r = create(&app, &s, &big, &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json);
}

#[tokio::test]
async fn compose_list_marks_managed() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    app.fake
        .add(summary("a", "shop-web-1", "running", Some("shop")));
    app.fake
        .add(summary("b", "blog-web-1", "running", Some("blog")));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/compose-projects",
            None,
            Some(&s),
        ))
        .await;
    let managed: Vec<(String, bool)> = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_string(),
                p["managed"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(managed, vec![("blog".into(), false), ("shop".into(), true)]);
}

#[tokio::test]
async fn deploy_body_with_compose_rejected() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    let r = app
        .send(req(
            "POST",
            &format!("{BASE}/shop/deploy"),
            Some(
                json!({"from": {"source": "draft"}, "plan_token": "x", "compose": "services: {}"}),
            ),
            Some(&s),
        ))
        .await;
    assert!(
        r.status.is_client_error() && r.status != StatusCode::CONFLICT,
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn remove_and_actions_conflict_while_deploying() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &[]).await;
    app.fake.hold_pulls.store(true, Ordering::SeqCst);
    let p = plan(&app, &s, json!({"source": "draft"})).await;
    let token = p.json["plan"]["token"].as_str().unwrap().to_string();
    let d = deploy(&app, &s, json!({"source": "draft"}), &token).await;
    assert_eq!(d.status, StatusCode::ACCEPTED);
    let rm = app
        .send(req("DELETE", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(rm.status, StatusCode::CONFLICT, "{:?}", rm.json);
    let act = app
        .send(req("POST", &format!("{BASE}/shop/restart"), None, Some(&s)))
        .await;
    assert_eq!(act.status, StatusCode::CONFLICT, "{:?}", act.json);
    let id = d.json["task_id"].as_str().unwrap().to_string();
    for _ in 0..500 {
        app.fake.release_pulls.notify_waiters();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let r = app
            .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(&s)))
            .await;
        if r.json["status"] != "running" {
            return;
        }
    }
    panic!("the held deploy never finished");
}

#[tokio::test]
async fn save_rejects_renamed_masked_value() {
    let app = fast_app().await;
    let s = app.with_local().await;
    create(&app, &s, WEB, &["TOKEN=hunter2"]).await;
    let renamed = format!("NEW_TOKEN={}", fleetops_core::kube::MASK);
    let r = app
        .send(req(
            "PUT",
            &format!("{BASE}/shop/draft"),
            Some(json!({"compose": WEB, "variables": [renamed]})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json);
    let g = app
        .send(req("GET", &format!("{BASE}/shop"), None, Some(&s)))
        .await;
    assert_eq!(g.json["draft"]["variables"], json!(["TOKEN=hunter2"]));
}
