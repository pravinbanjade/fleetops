//! HTTP API for dockerer. `app()` builds the complete router; the server
//! crate supplies the static-file router and sets `IsTls` per listener.

pub mod auth;
pub mod error;
mod headers;
mod request_id;
mod routes;
pub mod state;

use axum::{middleware::from_fn, Router};

pub use error::{ApiError, ApiResult};
pub use state::{ApiConfig, AppState};

/// Request extension telling handlers whether the connection is TLS.
/// Controls the cookie `Secure` flag and HSTS.
#[derive(Debug, Clone, Copy)]
pub struct IsTls(pub bool);

pub fn app(state: AppState, static_files: Router) -> Router {
    Router::new()
        .nest("/api/v1", routes::api().with_state(state))
        .merge(static_files)
        .layer(from_fn(headers::security_headers))
        .layer(from_fn(request_id::middleware))
}
