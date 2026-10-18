//! HTTP API for fleetops. `app()` builds the complete router; the server
//! crate supplies the static-file router and sets `IsTls` per listener.

pub mod agents;
pub mod auth;
pub mod authz;
pub mod backup;
pub mod compose;
pub mod envs;
pub mod error;
pub mod external;
pub mod extract;
mod headers;
pub mod openapi;
mod request_id;
mod routes;
pub mod state;
pub mod tasks;
pub mod ws;

use axum::{middleware::from_fn, Router};
use std::sync::Arc;
use utoipa_axum::router::OpenApiRouter;

pub use auth::token::hash_token as auth_token_hash;
pub use error::{ApiError, ApiResult};
pub use routes::access::{AccessEntry, SetAccess, Wanted};
pub use routes::audit_detail;
pub use routes::backup::write_to_tmp as write_backup_to_tmp;
pub use state::{ApiConfig, AppState};

/// Request extension telling handlers whether the connection is TLS.
/// Controls the cookie `Secure` flag and (when enabled) HSTS.
#[derive(Debug, Clone, Copy)]
pub struct IsTls(pub bool);

pub fn app(state: AppState, static_files: Router) -> Router {
    build(state, static_files, routes::api(), None)
}

/// `app` plus `extra` routes (the Enterprise API), which are served under
/// the API base and documented in the served OpenAPI document.
pub fn app_with(state: AppState, static_files: Router, extra: OpenApiRouter<AppState>) -> Router {
    let api = routes::api().merge(extra);
    let doc = openapi::document(api.clone())
        .to_pretty_json()
        .expect("OpenAPI document serialises");
    build(state, static_files, api, Some(doc))
}

fn build(
    state: AppState,
    static_files: Router,
    api: OpenApiRouter<AppState>,
    doc: Option<String>,
) -> Router {
    let hsts = state.config.hsts;
    let mut api = api.with_state(state).split_for_parts().0;
    if let Some(doc) = doc {
        api = api.layer(axum::Extension(openapi::Served(Arc::from(doc))));
    }
    Router::new()
        .nest(openapi::API_BASE, api)
        .merge(static_files)
        .layer(from_fn(move |req, next| {
            headers::security_headers(hsts, req, next)
        }))
        .layer(from_fn(request_id::middleware))
}
