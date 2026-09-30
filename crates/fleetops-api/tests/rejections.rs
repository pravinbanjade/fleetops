//! Requests axum's own extractors reject still get the JSON error body the
//! OpenAPI document promises for every error. `TestApp::send` checks the
//! body against that schema.

mod common;

use axum::{body::Body, http::Request};
use common::*;

fn assert_invalid(r: &Resp, status: u16) {
    assert_eq!(r.status, status, "{:?}", r.json);
    assert_eq!(r.json["code"], "invalid", "{:?}", r.json);
    assert!(r.json["message"].is_string());
}

#[tokio::test]
async fn missing_query_parameter_is_a_json_error() {
    let t = app().await;
    let s = t.with_local().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/files",
            None,
            Some(&s),
        ))
        .await;
    assert_invalid(&r, 400);
}

#[tokio::test]
async fn malformed_query_parameter_is_a_json_error() {
    let t = app().await;
    let s = t.setup_admin().await;
    let r = t
        .send(req("GET", "/api/v1/audit?limit=many", None, Some(&s)))
        .await;
    assert_invalid(&r, 400);
    let r = t
        .send(req(
            "DELETE",
            "/api/v1/envs/local/volumes/v1?force=maybe",
            None,
            Some(&s),
        ))
        .await;
    assert_invalid(&r, 400);
}

#[tokio::test]
async fn build_without_a_multipart_body_is_a_json_error() {
    let t = app().await;
    let s = t.with_local().await;
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/images/build",
            Some(serde_json::json!({"tag": "app"})),
            Some(&s),
        ))
        .await;
    assert_invalid(&r, 400);
}

#[tokio::test]
async fn plain_get_on_the_websocket_is_a_json_error() {
    let t = app().await;
    let s = t.setup_admin().await;
    let mut r: Request<Body> = req("GET", "/api/v1/ws", None, Some(&s));
    r.headers_mut().remove("content-type");
    let r = t.send(r).await;
    assert_invalid(&r, 400);
}
