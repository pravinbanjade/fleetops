mod common;

use axum::http::StatusCode;
use common::*;
use dockerer_api::IsTls;

#[tokio::test]
async fn health_ok_with_request_id_and_security_headers() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/health", None, None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["status"], "ok");
    assert!(r.headers.get("x-request-id").is_some());
    let csp = r
        .headers
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert_eq!(r.headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(r.headers.get("cache-control").unwrap(), "no-store");
    assert!(r.headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn hsts_only_over_tls() {
    let app = app().await;
    let mut request = req("GET", "/api/v1/health", None, None);
    request.extensions_mut().insert(IsTls(true));
    let r = app.send(request).await;
    assert!(r.headers.get("strict-transport-security").is_some());
}

#[tokio::test]
async fn unknown_api_route_is_json_404_with_matching_request_id() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/nope", None, None)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json["code"], "not_found");
    assert_eq!(
        r.json["request_id"].as_str().unwrap(),
        r.headers.get("x-request-id").unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn client_request_id_is_propagated_when_safe() {
    let app = app().await;
    let mut request = req("GET", "/api/v1/health", None, None);
    request
        .headers_mut()
        .insert("x-request-id", "abc-123".parse().unwrap());
    let r = app.send(request).await;
    assert_eq!(r.headers.get("x-request-id").unwrap(), "abc-123");

    let mut request = req("GET", "/api/v1/health", None, None);
    request
        .headers_mut()
        .insert("x-request-id", "bad%20id".parse().unwrap());
    let r = app.send(request).await;
    assert_ne!(r.headers.get("x-request-id").unwrap(), "bad%20id");
}
