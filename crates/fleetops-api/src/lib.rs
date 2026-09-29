//! HTTP API for fleetops. `app()` builds the complete router; the server
//! crate supplies the static-file router and sets `IsTls` per listener.

pub mod auth;
pub mod authz;
pub mod compose;
pub mod envs;
pub mod error;
pub mod extract;
mod headers;
mod request_id;
mod routes;
pub mod state;

use axum::{middleware::from_fn, Router};

pub use error::{ApiError, ApiResult};
pub use state::{ApiConfig, AppState};

/// Request extension telling handlers whether the connection is TLS.
/// Controls the cookie `Secure` flag and (when enabled) HSTS.
#[derive(Debug, Clone, Copy)]
pub struct IsTls(pub bool);

pub fn app(state: AppState, static_files: Router) -> Router {
    let hsts = state.config.hsts;
    Router::new()
        .nest("/api/v1", routes::api().with_state(state))
        .merge(static_files)
        .layer(from_fn(move |req, next| {
            headers::security_headers(hsts, req, next)
        }))
        .layer(from_fn(request_id::middleware))
}
