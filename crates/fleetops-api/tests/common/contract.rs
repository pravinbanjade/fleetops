//! Checks API traffic against the published OpenAPI document.
//!
//! `TestApp::send` passes every exchange of the API test suite through
//! [`check`], so the whole suite doubles as a contract test. A test fails
//! when the server answers:
//! - with a status the operation does not document (`default` covers
//!   errors only, never a success),
//! - with a JSON body that does not match the documented schema,
//! - or with a 2xx to a JSON request body the documented schema rejects.

use jsonschema::Validator;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

pub fn doc() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| serde_json::from_str(fleetops_api::openapi::json()).expect("valid JSON"))
}

/// One request and its response, as `TestApp::send` saw them.
pub struct Exchange<'a> {
    pub method: &'a str,
    /// The request path, without the query string.
    pub path: &'a str,
    /// The request body, when it was JSON.
    pub request: Option<&'a Value>,
    pub status: u16,
    pub content_type: Option<&'a str>,
    /// The response body: parsed JSON, a string for other bodies, or null
    /// when empty.
    pub body: &'a Value,
}

/// The documented path that axum routes `path` to. Like axum's router, a
/// literal segment beats a `{param}` at the same position.
pub fn route_of(path: &str) -> Option<&'static str> {
    let segs: Vec<&str> = path.split('/').collect();
    doc()["paths"]
        .as_object()?
        .keys()
        .filter_map(|tpl| {
            let t: Vec<&str> = tpl.split('/').collect();
            if t.len() != segs.len() {
                return None;
            }
            let mut rank = Vec::with_capacity(t.len());
            for (a, b) in t.iter().zip(&segs) {
                if a.starts_with('{') && a.ends_with('}') && !b.is_empty() {
                    rank.push(1);
                } else if a == b {
                    rank.push(0);
                } else {
                    return None;
                }
            }
            Some((rank, tpl.as_str()))
        })
        .min()
        .map(|(_, tpl)| tpl)
}

fn is_json(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| c.starts_with("application/json"))
}

/// Validates `instance` against `schema`, which may `$ref` the document's
/// components. Compiled validators are cached under `key`.
fn validate(key: String, schema: &Value, instance: &Value) -> Result<(), String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Validator>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let v = {
        let mut c = cache.lock().unwrap();
        match c.get(&key) {
            Some(v) => v.clone(),
            None => {
                let mut root = schema.clone();
                root["components"] = doc()["components"].clone();
                let v = Arc::new(
                    jsonschema::validator_for(&root)
                        .map_err(|e| format!("{key}: bad schema: {e}"))?,
                );
                c.insert(key.clone(), v.clone());
                v
            }
        }
    };
    let errors: Vec<String> = v
        .iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{key}: {}\nbody: {instance:#}", errors.join("; ")))
    }
}

pub fn check(x: &Exchange) -> Result<(), String> {
    if !x.path.starts_with(fleetops_api::openapi::API_BASE) {
        return Ok(());
    }
    let what = format!("{} {} -> {}", x.method, x.path, x.status);
    let Some(tpl) = route_of(x.path) else {
        // Routes documented in later tasks are not checked yet.
        return Ok(());
    };
    let method = x.method.to_ascii_lowercase();
    let Some(op) = doc()["paths"][tpl].get(&method) else {
        return if x.status == 405 {
            Ok(())
        } else {
            Err(format!("{what}: {method} is not documented for {tpl}"))
        };
    };
    let success = (200..300).contains(&x.status);
    let documented = &op["responses"][x.status.to_string()];
    let resp = match (documented.is_null(), success) {
        (false, _) => documented,
        (true, false) => &op["responses"]["default"],
        (true, true) => return Err(format!("{what}: status not documented for {tpl}")),
    };
    let key = format!("{method} {tpl} {}", x.status);
    match resp.get("content").and_then(Value::as_object) {
        None if !x.body.is_null() => {
            return Err(format!("{what}: documented without a body, got {}", x.body))
        }
        None => {}
        Some(content) => {
            let (ct, media) = content.iter().next().expect("one media type");
            let sent = x.content_type.unwrap_or("");
            if !sent.starts_with(ct.as_str()) {
                return Err(format!("{what}: documented as {ct}, sent as {sent:?}"));
            }
            if is_json(Some(ct)) {
                validate(key.clone(), &media["schema"], x.body)?;
            }
        }
    }
    if let (true, Some(request)) = (success, x.request) {
        if let Some(schema) = op["requestBody"]["content"]["application/json"].get("schema") {
            validate(format!("{key} request"), schema, request)?;
        }
    }
    Ok(())
}
