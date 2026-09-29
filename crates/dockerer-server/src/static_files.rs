use axum::{
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde_json::json;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
struct Assets;

pub fn router() -> Router {
    Router::new().fallback(serve)
}

async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        let body = json!({"code": "not_found", "message": "no such API route"});
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }
    if let Some(res) = asset(path) {
        return res;
    }
    // Paths with a file extension are real asset requests; everything else
    // is a client-side route handled by the SPA.
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    asset("index.html")
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "web UI not built").into_response())
}

fn asset(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref()).ok()?,
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
    ];
    Some((headers, file.data).into_response())
}
