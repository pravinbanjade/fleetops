//! The OpenAPI document: served, complete and self-consistent. The
//! contract checker in `common::contract` is tested here too, since every
//! other API test relies on it.

mod common;

use common::contract::{self, doc, route_of, Exchange};
use common::*;
use fleetops_api::ws::protocol;
use serde_json::{json, Value};
use std::collections::BTreeSet;

const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
/// Documented operations; update when adding or removing a route.
const OPERATIONS: usize = 81;

fn operations() -> Vec<(&'static str, &'static str, &'static Value)> {
    let mut out = Vec::new();
    for (path, item) in doc()["paths"].as_object().unwrap() {
        for m in METHODS {
            if let Some(op) = item.get(m) {
                out.push((path.as_str(), m, op));
            }
        }
    }
    out
}

#[tokio::test]
async fn document_is_served_without_a_session() {
    let app = app().await;
    let r = app
        .send(req("GET", "/api/v1/openapi.json", None, None))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "application/json");
    assert_eq!(r.json["openapi"], "3.1.0");
    assert_eq!(r.json["info"]["title"], "fleetops");
    assert_eq!(r.json["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(&r.json, doc());
}

#[test]
fn every_operation_has_a_unique_id_a_tag_and_a_success_response() {
    let mut ids = BTreeSet::new();
    for (path, m, op) in operations() {
        let id = op["operationId"]
            .as_str()
            .unwrap_or_else(|| panic!("{m} {path}: no operationId"));
        assert!(ids.insert(id), "duplicate operationId {id}");
        assert_eq!(
            op["tags"].as_array().map(Vec::len),
            Some(1),
            "{m} {path}: one tag"
        );
        assert!(op["summary"].is_string(), "{m} {path}: no summary");
        let ok = op["responses"]
            .as_object()
            .unwrap()
            .keys()
            .any(|s| s.starts_with('2') || s == "101");
        assert!(ok, "{m} {path}: no success response");
        assert!(
            op["responses"]["default"].is_object(),
            "{m} {path}: no error response"
        );
    }
    assert_eq!(ids.len(), OPERATIONS, "operation count changed: {ids:?}");
}

#[test]
fn path_placeholders_match_path_parameters() {
    for (path, m, op) in operations() {
        let placeholders: BTreeSet<&str> = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .collect();
        let params: BTreeSet<&str> = op["parameters"]
            .as_array()
            .map(|ps| {
                ps.iter()
                    .filter(|p| p["in"] == "path")
                    .map(|p| p["name"].as_str().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(placeholders, params, "{m} {path}");
    }
}

#[test]
fn every_schema_reference_resolves() {
    fn walk(v: &Value, schemas: &Value, found: &mut usize) {
        match v {
            Value::Object(o) => {
                if let Some(Value::String(r)) = o.get("$ref") {
                    let name = r
                        .strip_prefix("#/components/schemas/")
                        .unwrap_or_else(|| panic!("{r}"));
                    assert!(schemas.get(name).is_some(), "dangling $ref {r}");
                    *found += 1;
                }
                o.values().for_each(|x| walk(x, schemas, found));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, schemas, found)),
            _ => {}
        }
    }
    let mut found = 0;
    walk(doc(), &doc()["components"]["schemas"], &mut found);
    assert!(found >= OPERATIONS, "only {found} refs");
}

#[test]
fn mutations_need_a_session_and_a_csrf_token_and_reads_a_session() {
    let public = ["getHealth", "getOpenApi", "getAuthStatus", "setup", "login"];
    for (path, m, op) in operations() {
        let id = op["operationId"].as_str().unwrap();
        let want = if public.contains(&id) {
            json!([{}])
        } else if m == "get" {
            json!([{ "session": [] }])
        } else {
            json!([{ "session": [], "csrf": [] }])
        };
        assert_eq!(op["security"], want, "{m} {path}");
    }
    let schemes = &doc()["components"]["securitySchemes"];
    assert_eq!(
        schemes["session"],
        json!({"type": "apiKey", "in": "cookie", "name": "fleetops_session"})
    );
    assert_eq!(
        schemes["csrf"],
        json!({"type": "apiKey", "in": "header", "name": "x-csrf-token"})
    );
}

fn schema_check(name: &str, v: &Value) -> Result<(), String> {
    let mut root = json!({ "$ref": format!("#/components/schemas/{name}") });
    root["components"] = doc()["components"].clone();
    let validator = jsonschema::validator_for(&root).unwrap();
    let errors: Vec<String> = validator.iter_errors(v).map(|e| e.to_string()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn text(out: protocol::Out) -> Value {
    match out {
        protocol::Out::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn websocket_messages_match_their_schemas() {
    let task = json!({"id": "t1", "kind": "image.pull", "env": "local", "target": "nginx:latest",
        "status": "succeeded", "error": null, "created_at": 1, "finished_at": 2});
    let up = json!({"state": "up", "version": {"kind": "docker", "version": "27", "api_version": "1.47", "os": "linux", "arch": "arm64"}});
    let server = [
        protocol::data(1, json!({"kind": "container"})),
        protocol::gap(1, 12),
        protocol::end(1, "eof", Value::Null),
        protocol::end(1, "exited", json!({ "exit_code": 0 })),
        protocol::end(1, "exited", json!({ "exit_code": null })),
        protocol::end(1, "succeeded", json!({ "task": task })),
        protocol::error(Some(1), "not_found", "no such container"),
        protocol::error(None, "invalid", "bad frame"),
        protocol::env_status("local", &up),
        protocol::env_status("local", &json!({"state": "down", "reason": "refused"})),
        protocol::end(1, "access", json!({ "message": "your access changed" })),
        protocol::access_changed(),
    ];
    for out in server {
        let v = text(out);
        schema_check("ServerMsg", &v).unwrap_or_else(|e| panic!("{v}: {e}"));
    }
    assert!(schema_check("ServerMsg", &json!({"type": "gap", "sub": 1})).is_err());

    let client = [
        json!({"op": "sub", "sub": 1, "topic": "events", "env": "local"}),
        json!({"op": "sub", "sub": 2, "topic": "logs", "env": "local", "id": "web", "opts": {"tail": 10}}),
        json!({"op": "unsub", "sub": 1}),
        json!({"op": "input", "sub": 3, "data": "ls\n"}),
        json!({"op": "resize", "sub": 3, "cols": 120, "rows": 40}),
    ];
    for v in client {
        schema_check("ClientMsg", &v).unwrap_or_else(|e| panic!("{v}: {e}"));
        serde_json::from_value::<protocol::ClientMsg>(v).unwrap();
    }
    assert!(schema_check(
        "ClientMsg",
        &json!({"op": "sub", "sub": 1, "topic": "nope"})
    )
    .is_err());
}

// The contract checker itself.

fn exchange<'a>(method: &'a str, path: &'a str, status: u16, body: &'a Value) -> Exchange<'a> {
    Exchange {
        method,
        path,
        request: None,
        status,
        content_type: Some("application/json"),
        body,
    }
}

#[test]
fn routes_resolve_like_axum() {
    assert_eq!(
        route_of("/api/v1/envs/local/containers/prune"),
        Some("/api/v1/envs/{env}/containers/prune")
    );
    assert_eq!(
        route_of("/api/v1/envs/local/containers/web"),
        Some("/api/v1/envs/{env}/containers/{id}")
    );
    assert_eq!(
        route_of("/api/v1/envs/local/containers/web/stop"),
        Some("/api/v1/envs/{env}/containers/{id}/{action}")
    );
    assert_eq!(
        route_of("/api/v1/envs/local/containers/web/spec"),
        Some("/api/v1/envs/{env}/containers/{id}/spec")
    );
    assert_eq!(route_of("/api/v1/nope"), None);
    let err = json!({"code": "not_found", "message": "not found: x", "request_id": "r1"});
    let x = exchange("GET", "/api/v1/envs/local/containers/x", 404, &err);
    assert_eq!(contract::check(&x), Ok(()));
}

#[test]
fn checker_accepts_matching_traffic() {
    let ok = json!({"status": "ok"});
    assert_eq!(
        contract::check(&exchange("GET", "/api/v1/health", 200, &ok)),
        Ok(())
    );
    let err = json!({"code": "not_found", "message": "not found: x", "request_id": "r1"});
    assert_eq!(
        contract::check(&exchange("GET", "/api/v1/nope", 404, &err)),
        Ok(())
    );
    assert_eq!(
        contract::check(&exchange("PUT", "/api/v1/health", 405, &Value::Null)),
        Ok(())
    );
    assert_eq!(
        contract::check(&exchange("GET", "/index.html", 200, &Value::Null)),
        Ok(())
    );
}

#[test]
fn checker_rejects_mismatched_traffic() {
    let wrong = json!({"status": 5});
    let e = contract::check(&exchange("GET", "/api/v1/health", 200, &wrong)).unwrap_err();
    assert!(e.contains("get /api/v1/health 200"), "{e}");
    let ok = json!({"status": "ok"});
    let e = contract::check(&exchange("GET", "/api/v1/health", 201, &ok)).unwrap_err();
    assert!(e.contains("status not documented"), "{e}");
    let e = contract::check(&exchange("POST", "/api/v1/auth/logout", 204, &ok)).unwrap_err();
    assert!(e.contains("without a body"), "{e}");
    let bad_error = json!({"message": "no code"});
    assert!(contract::check(&exchange("GET", "/api/v1/auth/me", 401, &bad_error)).is_err());
    let text_body = Value::String("oops".into());
    let mut x = exchange("GET", "/api/v1/health", 200, &text_body);
    x.content_type = Some("text/plain");
    assert!(contract::check(&x)
        .unwrap_err()
        .contains("documented as application/json"));

    let user =
        json!({"id": 2, "username": "bob", "role": "user", "created_at": 1, "updated_at": 1});
    let request = json!({"username": "bob", "password": "long-enough", "role": "superuser"});
    let mut x = exchange("POST", "/api/v1/users", 201, &user);
    x.request = Some(&request);
    assert!(contract::check(&x).unwrap_err().contains("request"));
}

#[test]
fn checker_rejects_undocumented_routes() {
    let ok = json!({"status": "ok"});
    let e = contract::check(&exchange("GET", "/api/v1/nope", 200, &ok)).unwrap_err();
    assert!(e.contains("the route is not documented"), "{e}");
}

/// `crates/fleetops-api/openapi.json` is committed for the web client
/// generator (`pnpm gen:api`) and for review of API changes.
#[test]
fn committed_document_matches_the_code() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed.trim_end() == fleetops_api::openapi::json(),
        "{path} is out of date; run `make openapi` and commit the result"
    );
}
