mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;

#[tokio::test]
async fn lists_projects() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "running", Some("shop")));
    app.fake
        .add(summary("b", "shop-db-1", "exited", Some("shop")));
    app.fake.add(summary("c", "lonely", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/compose-projects",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json.as_array().unwrap().len(), 1);
    assert_eq!(r.json[0]["name"], "shop");
    assert_eq!(r.json[0]["status"], "partial");
    assert_eq!(r.json[0]["containers"], 2);
}

#[tokio::test]
async fn project_actions_apply_to_members_only() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "exited", Some("shop")));
    app.fake
        .add(summary("b", "shop-db-1", "exited", Some("shop")));
    app.fake.add(summary("c", "other", "exited", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/start",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json, json!({"affected": 2, "errors": []}));
    assert_eq!(app.fake.container("a").unwrap().state, "running");
    assert_eq!(app.fake.container("c").unwrap().state, "exited");

    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/down",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["affected"], 2);
    assert!(app.fake.container("a").is_none() && app.fake.container("b").is_none());
    assert!(app.fake.container("c").is_some());
    let audit = app.store.list_audit(1, None).await.unwrap();
    assert_eq!(
        (audit[0].action.as_str(), audit[0].target_id.as_deref()),
        ("compose.down", Some("shop"))
    );
}

#[tokio::test]
async fn unknown_project_or_action_is_404() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "exited", Some("shop")));
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/nope/start",
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
            "/api/v1/envs/local/compose-projects/shop/explode",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
