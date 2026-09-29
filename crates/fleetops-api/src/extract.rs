//! `Json`/`Path` wrappers whose rejections use the standard error body
//! `{code, message, request_id}` instead of axum's plain-text defaults.

use axum::{
    extract::{
        rejection::{JsonRejection, PathRejection},
        FromRequest, FromRequestParts, Path, Request,
    },
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;
use serde_json::json;

fn rejection(status: StatusCode, message: String) -> Response {
    let body = json!({
        "code": "invalid",
        "message": message,
        "request_id": crate::request_id::current(),
    });
    (status, Json(body)).into_response()
}

pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(v)| ApiJson(v))
            .map_err(|r: JsonRejection| rejection(r.status(), r.body_text()))
    }
}

pub struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(v)| ApiPath(v))
            .map_err(|r: PathRejection| rejection(r.status(), r.body_text()))
    }
}
