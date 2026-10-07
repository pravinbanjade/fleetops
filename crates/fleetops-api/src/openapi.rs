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
use utoipa::{IntoParams, OpenApi, ToSchema};
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
        ContainerActionName,
        ComposeActionName,
        crate::ws::protocol::ClientMsg,
        crate::ws::protocol::ServerMsg,
        fleetops_core::streams::DockerEvent,
        fleetops_core::streams::StatsSample,
        fleetops_core::streams::LogLine,
        fleetops_core::kube::WatchEvent,
        crate::agents::KubeAccess,
        fleetops_core::kube::EditableObject,
        fleetops_core::kube::ApplyResult,
        fleetops_core::kube::ApplyError,
        fleetops_core::kube::ApplyAction,
        crate::routes::kube::ReplaceBody,
        crate::routes::kube::ApplyBody,
        fleetops_core::helm::HelmRelease,
        fleetops_core::helm::HelmRevision,
        fleetops_core::helm::ReleaseDetail,
        fleetops_core::helm::HelmChange,
        fleetops_core::helm::HelmObjectChange,
        fleetops_core::helm::HelmPlan,
        crate::routes::helm::RollbackBody,
        crate::routes::helm::UninstallBody,
        fleetops_core::streams::LogOptions,
        fleetops_core::streams::ExecOptions,
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
        (name = "kubernetes", description = "Objects in Kubernetes environments"),
        (name = "helm", description = "Helm releases in Kubernetes environments"),
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

// Path parameters, shared by the handlers that document them. Handlers
// extract tuples; `tests/openapi.rs` checks every `{name}` in a documented
// path has a parameter of that name.

#[derive(IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(dead_code)]
pub struct EnvParam {
    /// Environment name.
    env: String,
}

#[derive(IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(dead_code)]
pub struct EnvIdParam {
    /// Environment name.
    env: String,
    /// Resource id or name.
    id: String,
}

#[derive(IntoParams)]
#[into_params(parameter_in = Path)]
#[allow(dead_code)]
pub struct EnvNameParam {
    /// Environment name.
    env: String,
    /// Volume name.
    name: String,
}

/// Multipart body of `POST /envs/{env}/images/build`: send `context` or
/// `dockerfile`, not both.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct BuildForm {
    /// A tar of the build context.
    #[schema(value_type = Option<String>, format = Binary)]
    context: Option<Vec<u8>>,
    /// A lone Dockerfile, when there is no other context.
    #[schema(value_type = Option<String>, format = Binary)]
    dockerfile: Option<Vec<u8>>,
    /// Tag for the built image, e.g. `app:1.2`.
    tag: Option<String>,
    /// Dockerfile path inside the context (default `Dockerfile`).
    dockerfile_path: Option<String>,
}

/// `POST /envs/{env}/containers/{id}/{action}` actions.
#[derive(ToSchema)]
#[schema(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum ContainerActionName {
    Start,
    Stop,
    Restart,
    Kill,
    Pause,
    Unpause,
}

/// `POST /envs/{env}/compose-projects/{project}/{action}` actions; `down`
/// removes the project's containers.
#[derive(ToSchema)]
#[schema(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum ComposeActionName {
    Start,
    Stop,
    Restart,
    Down,
}

/// Raw bytes, e.g. a downloaded file.
#[derive(ToSchema)]
#[schema(value_type = String, format = Binary)]
#[allow(dead_code)]
pub struct Bytes(Vec<u8>);
