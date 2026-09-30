//! The OpenAPI schemas derived for core types must describe the JSON serde
//! actually reads and writes, since the web client is generated from them.

use fleetops_core::docker::{ContainerSpec, ContainerSummary, PortBinding};
use fleetops_core::env::EnvStatus;
use fleetops_core::Role;
use serde_json::{json, Value};
use utoipa::PartialSchema;

fn schema<T: PartialSchema>() -> Value {
    serde_json::to_value(T::schema()).unwrap()
}

#[test]
fn enums_use_their_wire_names() {
    assert_eq!(schema::<Role>()["enum"], json!(["admin", "user"]));
}

#[test]
fn env_status_is_tagged_by_state() {
    let s = schema::<EnvStatus>();
    let states: Vec<Value> = s["oneOf"]
        .as_array()
        .expect("oneOf")
        .iter()
        .map(|v| v["properties"]["state"]["enum"].clone())
        .collect();
    assert_eq!(states, vec![json!(["up"]), json!(["down"])]);
}

#[test]
fn defaulted_fields_are_optional_in_requests() {
    let spec = schema::<ContainerSpec>();
    assert!(spec.get("required").is_none(), "{spec:#}");
    assert_eq!(
        spec["properties"]["advanced"]["type"],
        json!(["object", "null"])
    );
    assert_eq!(
        schema::<PortBinding>()["required"],
        json!(["container_port"])
    );
}

#[test]
fn option_fields_may_be_null() {
    let s = schema::<ContainerSummary>();
    assert_eq!(
        s["properties"]["compose_project"]["type"],
        json!(["string", "null"])
    );
    let required = s["required"].as_array().unwrap();
    assert!(!required.contains(&json!("compose_project")), "{s:#}");
}
