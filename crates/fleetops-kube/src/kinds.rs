//! Each kind's API group, version and resource, for dynamic requests.

use fleetops_core::kube::KubeKind;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{ConfigMap, Event, Namespace, Node, Pod, Secret, Service},
    networking::v1::Ingress,
};
use kube::discovery::ApiResource;

pub fn api_resource(kind: KubeKind) -> ApiResource {
    match kind {
        KubeKind::Namespaces => ApiResource::erase::<Namespace>(&()),
        KubeKind::Nodes => ApiResource::erase::<Node>(&()),
        KubeKind::Deployments => ApiResource::erase::<Deployment>(&()),
        KubeKind::StatefulSets => ApiResource::erase::<StatefulSet>(&()),
        KubeKind::DaemonSets => ApiResource::erase::<DaemonSet>(&()),
        KubeKind::Jobs => ApiResource::erase::<Job>(&()),
        KubeKind::CronJobs => ApiResource::erase::<CronJob>(&()),
        KubeKind::Pods => ApiResource::erase::<Pod>(&()),
        KubeKind::Services => ApiResource::erase::<Service>(&()),
        KubeKind::Ingresses => ApiResource::erase::<Ingress>(&()),
        KubeKind::ConfigMaps => ApiResource::erase::<ConfigMap>(&()),
        KubeKind::Secrets => ApiResource::erase::<Secret>(&()),
        KubeKind::Events => ApiResource::erase::<Event>(&()),
    }
}

/// The object as JSON with `apiVersion` and `kind` set: list items arrive
/// without them, and the typed k8s-openapi structs require them.
pub fn typed_json(kind: KubeKind, obj: &kube::api::DynamicObject) -> serde_json::Value {
    let ar = api_resource(kind);
    let mut v = serde_json::to_value(obj).unwrap_or_default();
    v["apiVersion"] = ar.api_version.into();
    v["kind"] = ar.kind.into();
    v
}
