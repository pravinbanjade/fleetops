use axum::{
    http::{header::RETRY_AFTER, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fleetops_core::Error;
use serde_json::json;

/// Wraps the domain error so it can be returned from handlers.
#[derive(Debug)]
pub struct ApiError(pub Error);

pub type ApiResult<T> = Result<T, ApiError>;

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Error::EnvUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Upstream { status, .. } => StatusCode::from_u16(*status)
                .ok()
                .filter(|s| s.is_client_error() || s.is_server_error())
                .unwrap_or(StatusCode::BAD_GATEWAY),
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match &self.0 {
            Error::Internal(detail) => {
                tracing::error!(error = %detail, "internal error");
                "internal server error".to_string()
            }
            other => other.to_string(),
        };
        let body = json!({
            "code": self.0.code(),
            "message": message,
            "request_id": crate::request_id::current(),
        });
        let mut res = (status, Json(body)).into_response();
        if let Error::RateLimited { retry_after_secs } = self.0 {
            res.headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn status_of(e: Error) -> StatusCode {
        ApiError(e).into_response().status()
    }

    #[test]
    fn maps_variants_to_status() {
        assert_eq!(
            status_of(Error::NotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status_of(Error::Conflict("x".into())), StatusCode::CONFLICT);
        assert_eq!(
            status_of(Error::Invalid("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::Unauthorized("x".into())),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(Error::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(Error::RateLimited {
                retry_after_secs: 5
            }),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status_of(Error::EnvUnavailable("x".into())),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(Error::Upstream {
                status: 409,
                message: "x".into()
            }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(Error::Upstream {
                status: 200,
                message: "x".into()
            }),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            status_of(Error::Internal("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn rate_limited_sets_retry_after() {
        let res = ApiError(Error::RateLimited {
            retry_after_secs: 42,
        })
        .into_response();
        assert_eq!(res.headers().get("retry-after").unwrap(), "42");
    }

    #[tokio::test]
    async fn internal_details_are_not_leaked() {
        use http_body_util::BodyExt;
        let res = ApiError(Error::Internal("db password is hunter2".into())).into_response();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("internal server error"));
    }
}
