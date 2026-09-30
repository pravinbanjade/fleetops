//! The OpenAPI 3.1 document, built from the same `utoipa-axum` router that
//! serves the API, so a route cannot exist without its documentation.
//! Served at `/api/v1/openapi.json` and printed by `fleetops --openapi`.

use crate::auth::{cookie::SESSION_COOKIE, CSRF_HEADER};
use crate::error::ErrorBody;
use axum::{
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};
use std::sync::OnceLock;
use utoipa::openapi::{
    security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme},
    OpenApi as Doc, RefOr, Response as DocResponse, ResponseBuilder,
};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

/// Where the API is mounted; documented paths include it.
pub const API_BASE: &str = "/api/v1";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "fleetops",
        description = "Container management API. Authenticate with `POST /auth/login`; \
            the session cookie authenticates later calls, and every call that is not \
            GET must also send the `csrf_token` from the login response in the \
            `x-csrf-token` header.",
        license(name = "Apache-2.0"),
    ),
    components(schemas(
        ErrorBody,
    )),
    tags(
        (name = "system", description = "Server health and this document"),
        (name = "auth", description = "Setup, login and sessions"),
        (name = "users", description = "User accounts (admin only)"),
        (name = "audit", description = "Audit log (admin only)"),
        (name = "environments", description = "Managed Docker endpoints"),
        (name = "containers"),
        (name = "images"),
        (name = "volumes"),
        (name = "networks"),
        (name = "compose", description = "Compose projects, grouped by label"),
        (name = "tasks", description = "Background image pulls and builds"),
        (name = "websocket", description = "Real-time events, stats, logs and exec"),
    )
)]
struct ApiDoc;

/// The complete document for `router` (the API, mounted at [`API_BASE`]).
pub fn document<S: Clone + Send + Sync + 'static>(router: OpenApiRouter<S>) -> Doc {
    let mut doc = OpenApiRouter::<S>::with_openapi(ApiDoc::openapi())
        .nest(API_BASE, router)
        .into_openapi();
    doc.info.version = env!("CARGO_PKG_VERSION").into();
    secure(&mut doc);
    doc
}

/// The document as pretty JSON, built once.
pub fn json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    JSON.get_or_init(|| {
        let doc = document(crate::routes::api());
        doc.to_pretty_json().expect("OpenAPI document serialises")
    })
}

/// The OpenAPI document for this API.
#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getOpenApi",
    tag = "system",
    security(()),
    responses((status = 200, description = "OpenAPI 3.1 document", body = serde_json::Value))
)]
pub async fn serve() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        json(),
    )
        .into_response()
}

/// Declares the session cookie and CSRF header, applies them to every
/// operation that did not opt out with `security(())`, and gives every
/// operation the standard error body as its `default` response.
fn secure(doc: &mut Doc) {
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "session",
        SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
    );
    components.add_security_scheme(
        "csrf",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new(CSRF_HEADER))),
    );
    let error: RefOr<DocResponse> = ResponseBuilder::new()
        .description("Error: `{code, message, request_id}`")
        .content(
            "application/json",
            utoipa::openapi::ContentBuilder::new()
                .schema(Some(utoipa::openapi::Ref::from_schema_name("ErrorBody")))
                .build(),
        )
        .build()
        .into();
    for item in doc.paths.paths.values_mut() {
        let ops = [
            (&mut item.get, true),
            (&mut item.post, false),
            (&mut item.put, false),
            (&mut item.patch, false),
            (&mut item.delete, false),
        ];
        for (op, safe) in ops {
            let Some(op) = op else { continue };
            if op.security.is_none() {
                let mut needs = vec![SecurityRequirement::new("session", Vec::<String>::new())];
                if !safe {
                    needs = vec![SecurityRequirement::new("session", Vec::<String>::new())
                        .add("csrf", Vec::<String>::new())];
                }
                op.security = Some(needs);
            }
            op.responses
                .responses
                .entry("default".into())
                .or_insert_with(|| error.clone());
        }
    }
}
