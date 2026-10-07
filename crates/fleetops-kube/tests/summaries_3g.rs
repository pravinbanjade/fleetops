use fleetops_core::kube::{KubeKind, RowHealth};
use fleetops_kube::summaries::row;
use kube::api::DynamicObject;
use serde_json::json;

fn obj(v: serde_json::Value) -> DynamicObject {
    serde_json::from_value(v).unwrap()
}

fn meta(name: &str) -> serde_json::Value {
    json!({"name": name, "namespace": "apps", "creationTimestamp": "2026-10-01T00:00:00Z"})
}

#[test]
fn a_replicaset_short_of_replicas_warns_and_names_its_owner() {
    let mut m = meta("web-abc");
    m["ownerReferences"] =
        json!([{"apiVersion": "apps/v1", "kind": "Deployment", "name": "web", "uid": "u"}]);
    let r = row(
        KubeKind::ReplicaSets,
        &obj(json!({"metadata": m,
        "spec": {"replicas": 3, "selector": {}}, "status": {"replicas": 3, "readyReplicas": 1}})),
    );
    assert_eq!((r.status.as_str(), r.health), ("1/3", RowHealth::Warn));
    assert_eq!(r.columns["desired"], "3");
    assert_eq!(r.columns["ready"], "1");
    assert_eq!(r.columns["owner"], "web");
}

#[test]
fn an_old_replicaset_at_zero_is_off_not_a_problem() {
    let r = row(
        KubeKind::ReplicaSets,
        &obj(json!({"metadata": meta("web-old"),
        "spec": {"replicas": 0, "selector": {}}, "status": {"replicas": 0}})),
    );
    assert_eq!((r.status.as_str(), r.health), ("0/0", RowHealth::Off));
}

#[test]
fn an_hpa_pinned_at_max_warns() {
    let r = row(
        KubeKind::HorizontalPodAutoscalers,
        &obj(json!({"metadata": meta("web"),
        "spec": {"scaleTargetRef": {"kind": "Deployment", "name": "web"}, "minReplicas": 1, "maxReplicas": 5,
                 "metrics": [{"type": "Resource", "resource": {"name": "cpu", "target": {"type": "Utilization", "averageUtilization": 80}}}]},
        "status": {"currentReplicas": 5, "desiredReplicas": 5,
                   "currentMetrics": [{"type": "Resource", "resource": {"name": "cpu", "current": {"averageUtilization": 95}}}],
                   "conditions": [{"type": "ScalingActive", "status": "True"}]}})),
    );
    assert_eq!(r.health, RowHealth::Warn);
    assert_eq!(r.columns["target"], "web");
    assert_eq!(r.columns["target_kind"], "deployments");
    assert_eq!(
        (
            r.columns["min"].as_str(),
            r.columns["max"].as_str(),
            r.columns["current"].as_str()
        ),
        ("1", "5", "5")
    );
    assert_eq!(r.columns["targets"], "cpu: 95%/80%");
}

#[test]
fn an_hpa_that_cannot_scale_is_bad() {
    let r = row(
        KubeKind::HorizontalPodAutoscalers,
        &obj(json!({"metadata": meta("web"),
        "spec": {"scaleTargetRef": {"kind": "Deployment", "name": "web"}, "maxReplicas": 5},
        "status": {"currentReplicas": 2, "desiredReplicas": 2,
                   "conditions": [{"type": "ScalingActive", "status": "False", "reason": "FailedGetResourceMetric"}]}})),
    );
    assert_eq!(
        (r.status.as_str(), r.health),
        ("FailedGetResourceMetric", RowHealth::Bad)
    );
    assert_eq!(r.columns["min"], "1");
}

#[test]
fn a_network_policy_shows_its_selector_and_types() {
    let r = row(
        KubeKind::NetworkPolicies,
        &obj(json!({"metadata": meta("deny"),
        "spec": {"podSelector": {"matchLabels": {"app": "web"}}, "policyTypes": ["Ingress", "Egress"]}})),
    );
    assert_eq!(r.health, RowHealth::Ok);
    assert_eq!(r.columns["pod_selector"], "app=web");
    assert_eq!(r.columns["policy_types"], "Ingress, Egress");
    let all = row(
        KubeKind::NetworkPolicies,
        &obj(json!({"metadata": meta("all"), "spec": {"podSelector": {}}})),
    );
    assert_eq!(all.columns["pod_selector"], "(all pods)");
    assert_eq!(all.columns["policy_types"], "Ingress");
}
