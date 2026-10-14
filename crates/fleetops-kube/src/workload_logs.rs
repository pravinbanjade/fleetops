//! Logs merged across a workload's pods (spec 3i §4): its selector, a
//! watch of its pods, and one log task per pod into one stream.

use fleetops_core::kube::{KubeRow, RowHealth};
use fleetops_core::{Error, Result};
use kube::api::DynamicObject;
use serde_json::Value;
use std::collections::BTreeMap;

/// Most pods one merged log reads at once (spec 3i §2).
pub const WORKLOAD_LOG_PODS: usize = 25;

/// `spec.selector` as a label selector string, as `kubectl` writes one.
/// An empty selector would match every pod in the namespace, so it is refused.
pub fn selector(sel: &Value) -> Result<String> {
    let mut parts = Vec::new();
    for (k, v) in sel["matchLabels"].as_object().into_iter().flatten() {
        parts.push(format!("{k}={}", v.as_str().unwrap_or_default()));
    }
    for e in sel["matchExpressions"].as_array().into_iter().flatten() {
        let key = e["key"].as_str().unwrap_or_default();
        let values: Vec<&str> = e["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        parts.push(match e["operator"].as_str().unwrap_or_default() {
            "In" => format!("{key} in ({})", values.join(",")),
            "NotIn" => format!("{key} notin ({})", values.join(",")),
            "Exists" => key.to_string(),
            "DoesNotExist" => format!("!{key}"),
            op => return Err(Error::Invalid(format!("unknown selector operator '{op}'"))),
        });
    }
    if parts.is_empty() {
        return Err(Error::Invalid("workload has no selector".into()));
    }
    Ok(parts.join(","))
}

/// A pod as the merged log needs it. The list's row puts a container's
/// reason (`CrashLoopBackOff`) in `status`, which hides the phase this needs.
pub fn log_row(o: &DynamicObject) -> KubeRow {
    let m = &o.metadata;
    let status = if m.deletion_timestamp.is_some() {
        "Terminating"
    } else {
        o.data["status"]["phase"].as_str().unwrap_or("Pending")
    };
    let names = |field: &str| -> Vec<String> {
        o.data["spec"][field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["name"].as_str().map(str::to_string))
            .collect()
    };
    let mut containers = names("containers");
    containers.extend(names("initContainers"));
    let restarts: i64 = o.data["status"]["containerStatuses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["restartCount"].as_i64())
        .sum();
    KubeRow {
        name: m.name.clone().unwrap_or_default(),
        namespace: m.namespace.clone(),
        created: m
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.as_second())
            .unwrap_or(0),
        status: status.to_string(),
        health: RowHealth::Unknown,
        columns: BTreeMap::from([
            ("containers".to_string(), containers.join(",")),
            ("restarts".to_string(), restarts.to_string()),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn match_labels_and_every_operator_render_as_kubectl_does() {
        let s = selector(&json!({
            "matchLabels": {"app": "web"},
            "matchExpressions": [
                {"key": "tier", "operator": "In", "values": ["a", "b"]},
                {"key": "env", "operator": "NotIn", "values": ["dev"]},
                {"key": "canary", "operator": "Exists"},
                {"key": "legacy", "operator": "DoesNotExist"}
            ]
        }))
        .unwrap();
        assert_eq!(s, "app=web,tier in (a,b),env notin (dev),canary,!legacy");
    }

    #[test]
    fn an_empty_or_missing_selector_is_refused() {
        for sel in [json!({}), json!(null), json!({"matchLabels": {}})] {
            let e = selector(&sel).unwrap_err();
            assert!(
                matches!(e, Error::Invalid(ref m) if m == "workload has no selector"),
                "{e}"
            );
        }
    }

    #[test]
    fn an_unknown_operator_is_refused() {
        let e = selector(&json!({"matchExpressions": [{"key": "a", "operator": "Gt"}]}));
        assert!(matches!(e, Err(Error::Invalid(_))));
    }

    fn pod(v: Value) -> DynamicObject {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_log_row_keeps_phase_containers_and_restarts() {
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "web-1", "namespace": "apps", "creationTimestamp": "2026-10-14T10:00:00Z"},
            "spec": {"containers": [{"name": "web"}, {"name": "side"}], "initContainers": [{"name": "migrate"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "web", "restartCount": 2}, {"name": "side", "restartCount": 1}]}})));
        assert_eq!(r.name, "web-1");
        assert_eq!(r.namespace.as_deref(), Some("apps"));
        assert_eq!(r.status, "Running");
        assert_eq!(r.columns["containers"], "web,side,migrate");
        assert_eq!(r.columns["restarts"], "3");
        assert!(r.created > 0);
    }

    #[test]
    fn a_deleting_pod_is_terminating_and_no_status_is_pending() {
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "a", "namespace": "apps", "deletionTimestamp": "2026-10-14T10:00:00Z"},
            "status": {"phase": "Running"}})));
        assert_eq!(r.status, "Terminating");
        let r = log_row(&pod(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": "b", "namespace": "apps"}})));
        assert_eq!(r.status, "Pending");
        assert_eq!(r.columns["restarts"], "0");
    }
}
