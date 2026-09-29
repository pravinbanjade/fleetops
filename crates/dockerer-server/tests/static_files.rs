use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use dockerer_api::{ApiConfig, AppState};
use dockerer_server::static_files;
use dockerer_store::Store;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(router: axum::Router, uri: &str) -> (StatusCode, String, String) {
    let res = router
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body =
        String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned();
    (status, ct, body)
}

#[tokio::test]
async fn root_and_client_routes_serve_index() {
    for uri in ["/", "/envs/local/containers/abc"] {
        let (status, ct, body) = get(static_files::router(), uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ct.starts_with("text/html"), "{uri}: {ct}");
        assert!(body.contains("Dockerer"), "{uri}");
    }
}

#[tokio::test]
async fn missing_asset_is_404_not_index() {
    let (status, _, _) = get(static_files::router(), "/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_api_path_outside_v1_is_json_404() {
    let (status, ct, body) = get(static_files::router(), "/api/v2/things").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(ct.starts_with("application/json"));
    assert!(body.contains("not_found"));
}

#[tokio::test]
async fn full_app_serves_api_and_spa_with_security_headers() {
    let store = Store::in_memory().await.unwrap();
    let app = dockerer_api::app(
        AppState::new(store, ApiConfig::default()),
        static_files::router(),
    );
    let res = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get("content-security-policy").is_some());
    let (status, _, body) = get(app, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ok"));
}
