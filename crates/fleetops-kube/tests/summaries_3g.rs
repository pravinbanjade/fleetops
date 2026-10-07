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

fn cmeta(name: &str) -> serde_json::Value {
    json!({"name": name, "creationTimestamp": "2026-10-01T00:00:00Z"})
}

#[test]
fn pvc_health_follows_its_phase() {
    let pvc = |phase: &str| {
        row(
            KubeKind::PersistentVolumeClaims,
            &obj(json!({"metadata": meta("data"),
            "spec": {"accessModes": ["ReadWriteOnce"], "storageClassName": "standard", "volumeName": "pv-1",
                     "resources": {"requests": {"storage": "1Gi"}}},
            "status": {"phase": phase, "capacity": {"storage": "1Gi"}}})),
        )
    };
    let bound = pvc("Bound");
    assert_eq!(
        (bound.status.as_str(), bound.health),
        ("Bound", RowHealth::Ok)
    );
    assert_eq!(bound.columns["volume"], "pv-1");
    assert_eq!(bound.columns["capacity"], "1Gi");
    assert_eq!(bound.columns["access_modes"], "RWO");
    assert_eq!(bound.columns["storage_class"], "standard");
    assert_eq!(pvc("Pending").health, RowHealth::Warn);
    assert_eq!(pvc("Lost").health, RowHealth::Bad);
}

#[test]
fn a_pv_is_cluster_scoped_and_names_its_claim() {
    let r = row(
        KubeKind::PersistentVolumes,
        &obj(json!({"metadata": cmeta("pv-1"),
        "spec": {"capacity": {"storage": "5Gi"}, "accessModes": ["ReadWriteMany", "ReadOnlyMany"],
                 "persistentVolumeReclaimPolicy": "Delete", "storageClassName": "nfs",
                 "claimRef": {"name": "data", "namespace": "apps"}},
        "status": {"phase": "Released"}})),
    );
    assert_eq!(r.namespace, None);
    assert_eq!((r.status.as_str(), r.health), ("Released", RowHealth::Warn));
    assert_eq!(r.columns["claim"], "data");
    assert_eq!(r.columns["claim_namespace"], "apps");
    assert_eq!(r.columns["access_modes"], "RWX, ROX");
    assert_eq!(r.columns["reclaim_policy"], "Delete");
}

#[test]
fn the_default_storage_class_says_so() {
    let mut m = cmeta("standard");
    m["annotations"] = json!({"storageclass.kubernetes.io/is-default-class": "true"});
    let r = row(
        KubeKind::StorageClasses,
        &obj(json!({"metadata": m,
        "provisioner": "rancher.io/local-path", "volumeBindingMode": "WaitForFirstConsumer"})),
    );
    assert_eq!(r.columns["provisioner"], "rancher.io/local-path");
    assert_eq!(r.columns["reclaim_policy"], "Delete");
    assert_eq!(r.columns["binding_mode"], "WaitForFirstConsumer");
    assert_eq!(r.columns["default"], "Yes");
}

#[test]
fn rbac_rows_count_rules_and_list_subjects() {
    let role = row(
        KubeKind::Roles,
        &obj(json!({"metadata": meta("reader"),
        "rules": [{"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]},
                  {"apiGroups": [""], "resources": ["services"], "verbs": ["get"]}]})),
    );
    assert_eq!(role.columns["rules"], "2");
    let cr = row(
        KubeKind::ClusterRoles,
        &obj(json!({"metadata": cmeta("admin")})),
    );
    assert_eq!(cr.columns["rules"], "0");
    let rb = row(
        KubeKind::RoleBindings,
        &obj(json!({"metadata": meta("read"),
        "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": "view"},
        "subjects": [{"kind": "ServiceAccount", "name": "app", "namespace": "apps"}, {"kind": "User", "name": "ana"}]})),
    );
    assert_eq!(rb.columns["role"], "view");
    assert_eq!(rb.columns["role_kind"], "clusterroles");
    assert_eq!(rb.columns["subjects"], "ServiceAccount/app, User/ana");
    let sa = row(
        KubeKind::ServiceAccounts,
        &obj(json!({"metadata": meta("app"), "secrets": [{"name": "t"}]})),
    );
    assert_eq!(sa.columns["secrets"], "1");
}

fn widgets(namespaced: bool) -> fleetops_core::kube::ApiKind {
    use fleetops_core::kube::{ApiKind, KindRef, PrinterColumn};
    ApiKind {
        api: KindRef {
            group: "example.fleetops.io".into(),
            version: "v1".into(),
            resource: "widgets".into(),
        },
        kind: "Widget".into(),
        namespaced,
        verbs: vec!["get".into(), "list".into(), "watch".into()],
        columns: vec![PrinterColumn {
            name: "Size".into(),
            json_path: ".spec.size".into(),
            type_: "integer".into(),
        }],
        crd: Some("widgets.example.fleetops.io".into()),
    }
}

#[test]
fn a_generic_row_takes_printer_columns_and_ready_health() {
    use fleetops_kube::summaries::generic_row;
    let o = obj(
        json!({"apiVersion": "example.fleetops.io/v1", "kind": "Widget", "metadata": meta("w1"),
        "spec": {"size": 3},
        "status": {"conditions": [{"type": "Ready", "status": "False", "reason": "Pending"}]}}),
    );
    let r = generic_row(&widgets(true), &o);
    assert_eq!(r.namespace.as_deref(), Some("apps"));
    assert_eq!(r.columns["Size"], "3");
    assert_eq!((r.status.as_str(), r.health), ("Pending", RowHealth::Bad));
}

#[test]
fn without_a_ready_condition_health_is_unknown_and_cluster_scope_drops_the_namespace() {
    use fleetops_kube::summaries::generic_row;
    let o = obj(json!({"metadata": meta("w2"), "spec": {}}));
    let r = generic_row(&widgets(false), &o);
    assert_eq!(r.namespace, None);
    assert_eq!((r.status.as_str(), r.health), ("", RowHealth::Unknown));
    assert_eq!(r.columns["Size"], "");
    let ready = obj(
        json!({"metadata": meta("w3"), "status": {"conditions": [{"type": "Ready", "status": "True"}]}}),
    );
    assert_eq!(generic_row(&widgets(true), &ready).status, "Ready");
}
