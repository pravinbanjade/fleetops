use fleetops_core::kube::{KubeKind, RowHealth};
use fleetops_kube::summaries::{containers, owners, row};
use kube::api::DynamicObject;
use serde_json::json;

fn obj(v: serde_json::Value) -> DynamicObject {
    serde_json::from_value(v).unwrap()
}

fn meta(name: &str) -> serde_json::Value {
    json!({"name": name, "namespace": "apps", "creationTimestamp": "2026-10-01T00:00:00Z"})
}

#[test]
fn a_partly_available_deployment_is_a_warning() {
    let d = obj(
        json!({"metadata": meta("web"), "spec": {"replicas": 3, "selector": {}, "template": {}},
        "status": {"readyReplicas": 1, "updatedReplicas": 3, "availableReplicas": 1}}),
    );
    let r = row(KubeKind::Deployments, &d);
    assert_eq!(
        (r.name.as_str(), r.namespace.as_deref()),
        ("web", Some("apps"))
    );
    assert_eq!(r.created, 1_790_812_800);
    assert_eq!(r.status, "1/3");
    assert_eq!(r.health, RowHealth::Warn);
    assert_eq!(r.columns["ready"], "1/3");
    assert_eq!(r.columns["up_to_date"], "3");
    assert_eq!(r.columns["available"], "1");
}

#[test]
fn a_crash_looping_pod_shows_its_reason_and_restarts() {
    let p = obj(
        json!({"metadata": meta("web-1"), "spec": {"nodeName": "n1", "containers": [{"name": "a"}, {"name": "b"}]},
        "status": {"phase": "Running", "podIP": "10.0.0.5", "containerStatuses": [
            {"name": "a", "ready": true, "restartCount": 0, "image": "x", "imageID": "", "state": {"running": {}}},
            {"name": "b", "ready": false, "restartCount": 5, "image": "x", "imageID": "", "state": {"waiting": {"reason": "CrashLoopBackOff"}}}]}}),
    );
    let r = row(KubeKind::Pods, &p);
    assert_eq!(r.status, "CrashLoopBackOff");
    assert_eq!(r.health, RowHealth::Bad);
    assert_eq!(r.columns["ready"], "1/2");
    assert_eq!(r.columns["restarts"], "5");
    assert_eq!(r.columns["node"], "n1");
    assert_eq!(containers(&p), ["a", "b"]);
}

#[test]
fn a_ready_running_pod_is_ok() {
    let p = obj(
        json!({"metadata": meta("ok"), "spec": {"containers": [{"name": "a"}]},
        "status": {"phase": "Running", "containerStatuses": [
            {"name": "a", "ready": true, "restartCount": 0, "image": "x", "imageID": "", "state": {"running": {}}}]}}),
    );
    let r = row(KubeKind::Pods, &p);
    assert_eq!((r.status.as_str(), r.health), ("Running", RowHealth::Ok));
}

#[test]
fn a_service_lists_its_type_and_ports() {
    let s = obj(
        json!({"metadata": meta("web"), "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.9",
        "ports": [{"port": 80, "protocol": "TCP"}, {"port": 53, "protocol": "UDP"}]}}),
    );
    let r = row(KubeKind::Services, &s);
    assert_eq!(r.status, "ClusterIP");
    assert_eq!(r.columns["cluster_ip"], "10.96.0.9");
    assert_eq!(r.columns["ports"], "80/TCP, 53/UDP");
}

#[test]
fn nodes_jobs_and_secrets_summarise() {
    let n = obj(
        json!({"metadata": {"name": "n1", "labels": {"node-role.kubernetes.io/control-plane": ""}},
        "status": {"conditions": [{"type": "Ready", "status": "True"}], "nodeInfo": {"kubeletVersion": "v1.32.0",
        "architecture": "", "bootID": "", "containerRuntimeVersion": "", "kernelVersion": "", "kubeProxyVersion": "",
        "machineID": "", "operatingSystem": "", "osImage": "", "systemUUID": ""}}}),
    );
    let r = row(KubeKind::Nodes, &n);
    assert_eq!((r.status.as_str(), r.health), ("Ready", RowHealth::Ok));
    assert_eq!(r.namespace, None);
    assert_eq!(r.columns["roles"], "control-plane");
    assert_eq!(r.columns["version"], "v1.32.0");

    let j = obj(
        json!({"metadata": meta("once"), "spec": {"template": {}}, "status": {"succeeded": 1,
        "conditions": [{"type": "Complete", "status": "True"}]}}),
    );
    let r = row(KubeKind::Jobs, &j);
    assert_eq!((r.status.as_str(), r.health), ("Complete", RowHealth::Ok));
    assert_eq!(r.columns["completions"], "1/1");

    let s = obj(
        json!({"metadata": meta("creds"), "type": "Opaque", "data": {"a": "eA==", "b": "eQ=="}}),
    );
    let r = row(KubeKind::Secrets, &s);
    assert_eq!(r.status, "Opaque");
    assert_eq!(r.columns["keys"], "2");
}

#[test]
fn an_unparseable_object_still_gets_a_row() {
    let d = obj(json!({"metadata": meta("odd"), "spec": {"replicas": "three"}}));
    let r = row(KubeKind::Deployments, &d);
    assert_eq!((r.name.as_str(), r.health), ("odd", RowHealth::Unknown));
}

#[test]
fn owners_link_kinds_fleetops_has_pages_for() {
    let p = obj(
        json!({"metadata": {"name": "web-1", "namespace": "apps", "ownerReferences": [
        {"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "web-7d9", "uid": "u"}]}}),
    );
    let o = owners(&p);
    assert_eq!(o.len(), 1);
    assert_eq!(
        (o[0].kind.as_str(), o[0].name.as_str(), o[0].link),
        ("ReplicaSet", "web-7d9", None)
    );
}

#[test]
fn a_warning_event_names_its_object_and_when_it_was_last_seen() {
    let e = obj(
        json!({"metadata": meta("api-7.17f"), "type": "Warning", "reason": "BackOff",
        "message": "Back-off restarting failed container", "count": 14,
        "lastTimestamp": "2026-10-01T00:05:00Z",
        "involvedObject": {"kind": "Pod", "name": "api-7", "namespace": "apps"},
        "source": {"component": "kubelet"}}),
    );
    let r = row(KubeKind::Events, &e);
    assert_eq!((r.status.as_str(), r.health), ("Warning", RowHealth::Warn));
    assert_eq!(r.columns["reason"], "BackOff");
    assert_eq!(r.columns["object_kind"], "Pod");
    assert_eq!(r.columns["object_name"], "api-7");
    assert_eq!(r.columns["object_link"], "pods");
    assert_eq!(r.columns["count"], "14");
    assert_eq!(r.columns["last_seen"], "1790813100");
    assert_eq!(r.columns["source"], "kubelet");
}

#[test]
fn an_event_without_a_last_timestamp_falls_back_to_its_event_time() {
    let e = obj(json!({"metadata": meta("x.1"), "reason": "Scheduled",
        "eventTime": "2026-10-01T00:01:00.000000Z", "reportingComponent": "default-scheduler",
        "involvedObject": {"kind": "Lease", "name": "x"}}));
    let r = row(KubeKind::Events, &e);
    assert_eq!((r.status.as_str(), r.health), ("Normal", RowHealth::Ok));
    assert_eq!(r.columns["last_seen"], "1790812860");
    assert_eq!(r.columns["object_link"], "");
    assert_eq!(r.columns["count"], "1");
    assert_eq!(r.columns["source"], "default-scheduler");
}
