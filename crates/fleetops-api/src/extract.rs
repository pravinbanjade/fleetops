//! `Json`/`Path`/`Query`/`Multipart` wrappers whose rejections use the
//! standard error body `{code, message, request_id}` instead of axum's
//! plain-text defaults, as the OpenAPI document promises for every error.

use crate::error::ErrorBody;
use axum::{
    extract::{
        multipart::MultipartRejection,
        rejection::{JsonRejection, PathRejection, QueryRejection},
        FromRequest, FromRequestParts, Multipart, Path, Query, Request,
    },
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;

pub(crate) fn rejection(status: StatusCode, message: String) -> Response {
    let body = ErrorBody {
        code: "invalid",
        message,
        request_id: crate::request_id::current(),
    };
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

/// `ApiJson`, or `T::default()` for a request with no body.
pub struct ApiJsonOr<T>(pub T);

impl<S, T> FromRequest<S> for ApiJsonOr<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Default,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|r| rejection(r.status(), r.body_text()))?;
        if bytes.is_empty() {
            return Ok(ApiJsonOr(T::default()));
        }
        Json::<T>::from_bytes(&bytes)
            .map(|Json(v)| ApiJsonOr(v))
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

pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(v)| ApiQuery(v))
            .map_err(|r: QueryRejection| rejection(r.status(), r.body_text()))
    }
}

pub struct ApiMultipart(pub Multipart);

impl<S: Send + Sync> FromRequest<S> for ApiMultipart {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        Multipart::from_request(req, state)
            .await
            .map(ApiMultipart)
            .map_err(|r: MultipartRejection| rejection(r.status(), r.body_text()))
    }
}
