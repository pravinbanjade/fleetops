//! Which container images a cluster uses: every Pod's containers, and the
//! templates of workloads that have no Pod right now.

use fleetops_core::kube::KubeImageUse;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{Pod, PodSpec},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::{HashMap, HashSet};

/// Everything [`uses`] reads, listed across all namespaces.
#[derive(Default)]
pub struct Objects {
    pub pods: Vec<Pod>,
    pub replica_sets: Vec<ReplicaSet>,
    pub deployments: Vec<Deployment>,
    pub stateful_sets: Vec<StatefulSet>,
    pub daemon_sets: Vec<DaemonSet>,
    pub jobs: Vec<Job>,
    pub cron_jobs: Vec<CronJob>,
}

type Key = (String, String, String); // namespace, kind, name

fn ns_name(m: &ObjectMeta) -> (String, String) {
    (
        m.namespace.clone().unwrap_or_default(),
        m.name.clone().unwrap_or_default(),
    )
}

fn controller(m: &ObjectMeta) -> Option<(String, String)> {
    m.owner_references
        .iter()
        .flatten()
        .find(|o| o.controller == Some(true))
        .or_else(|| m.owner_references.iter().flatten().next())
        .map(|o| (o.kind.clone(), o.name.clone()))
}

/// The digest part of a container status `imageID`
/// (`docker.io/library/nginx@sha256:…`, `docker-pullable://…@sha256:…`).
pub fn digest_of(image_id: &str) -> Option<String> {
    let d = image_id.rsplit('@').next().unwrap_or(image_id);
    let d = d.strip_prefix("docker-pullable://").unwrap_or(d);
    d.starts_with("sha256:").then(|| d.to_string())
}

pub fn uses(o: &Objects) -> Vec<KubeImageUse> {
    // ReplicaSet → Deployment, Job → CronJob: one step up is enough.
    let mut parent: HashMap<Key, (String, String)> = HashMap::new();
    for rs in &o.replica_sets {
        let (ns, name) = ns_name(&rs.metadata);
        if let Some(p) = controller(&rs.metadata) {
            parent.insert((ns, "ReplicaSet".into(), name), p);
        }
    }
    for j in &o.jobs {
        let (ns, name) = ns_name(&j.metadata);
        if let Some(p) = controller(&j.metadata) {
            parent.insert((ns, "Job".into(), name), p);
        }
    }
    let top = |ns: &str, kind: String, name: String| -> (String, String) {
        match parent.get(&(ns.to_string(), kind.clone(), name.clone())) {
            Some(p) => p.clone(),
            None => (kind, name),
        }
    };

    let mut out = Vec::new();
    let mut with_pods: HashSet<Key> = HashSet::new();
    for pod in &o.pods {
        let (ns, name) = ns_name(&pod.metadata);
        let (kind, owner) = match controller(&pod.metadata) {
            Some((k, n)) => top(&ns, k, n),
            None => ("Pod".into(), name.clone()),
        };
        with_pods.insert((ns.clone(), kind.clone(), owner.clone()));
        let Some(spec) = &pod.spec else { continue };
        let statuses: HashMap<&str, _> = pod
            .status
            .iter()
            .flat_map(|s| {
                s.container_statuses
                    .iter()
                    .flatten()
                    .chain(s.init_container_statuses.iter().flatten())
            })
            .map(|c| (c.name.as_str(), c))
            .collect();
        for c in containers(spec) {
            let st = statuses.get(c.0.as_str());
            out.push(KubeImageUse {
                namespace: ns.clone(),
                owner_kind: kind.clone(),
                owner_name: owner.clone(),
                container: c.0.clone(),
                image: c.1.clone(),
                image_id: st.and_then(|s| digest_of(&s.image_id)),
                running: st
                    .and_then(|s| s.state.as_ref())
                    .is_some_and(|s| s.running.is_some()),
                pull_secrets: pull_secrets(spec),
            });
        }
    }

    let mut template = |m: &ObjectMeta, kind: &str, spec: Option<&PodSpec>| {
        let (ns, name) = ns_name(m);
        if with_pods.contains(&(ns.clone(), kind.to_string(), name.clone())) {
            return;
        }
        let Some(spec) = spec else { return };
        for (container, image) in containers(spec) {
            out.push(KubeImageUse {
                namespace: ns.clone(),
                owner_kind: kind.into(),
                owner_name: name.clone(),
                container,
                image,
                image_id: None,
                running: false,
                pull_secrets: pull_secrets(spec),
            });
        }
    };
    for d in &o.deployments {
        template(
            &d.metadata,
            "Deployment",
            d.spec.as_ref().and_then(|s| s.template.spec.as_ref()),
        );
    }
    for s in &o.stateful_sets {
        template(
            &s.metadata,
            "StatefulSet",
            s.spec.as_ref().and_then(|s| s.template.spec.as_ref()),
        );
    }
    for d in &o.daemon_sets {
        template(
            &d.metadata,
            "DaemonSet",
            d.spec.as_ref().and_then(|s| s.template.spec.as_ref()),
        );
    }
    for j in o.jobs.iter().filter(|j| controller(&j.metadata).is_none()) {
        template(
            &j.metadata,
            "Job",
            j.spec.as_ref().and_then(|s| s.template.spec.as_ref()),
        );
    }
    for c in &o.cron_jobs {
        template(
            &c.metadata,
            "CronJob",
            c.spec
                .as_ref()
                .and_then(|s| s.job_template.spec.as_ref())
                .and_then(|s| s.template.spec.as_ref()),
        );
    }
    out
}

/// (name, image) of every container and init container.
fn containers(spec: &PodSpec) -> Vec<(String, String)> {
    spec.init_containers
        .iter()
        .flatten()
        .chain(spec.containers.iter())
        .filter_map(|c| Some((c.name.clone(), c.image.clone()?)))
        .collect()
}

fn pull_secrets(spec: &PodSpec) -> Vec<String> {
    spec.image_pull_secrets
        .iter()
        .flatten()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn from<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    fn owned(kind: &str, name: &str) -> serde_json::Value {
        json!([{"apiVersion": "v1", "kind": kind, "name": name, "uid": "u", "controller": true}])
    }

    fn template(image: &str) -> serde_json::Value {
        json!({"spec": {"containers": [{"name": "app", "image": image}]}})
    }

    #[test]
    fn deployment_pod_is_owned_by_the_deployment_and_running() {
        let o = Objects {
            replica_sets: vec![from(json!({"metadata": {"name": "web-1", "namespace": "a",
                "ownerReferences": owned("Deployment", "web")}}))],
            deployments: vec![from(json!({"metadata": {"name": "web", "namespace": "a"},
                "spec": {"selector": {}, "template": template("nginx:1.25")}}))],
            pods: vec![from(json!({
                "metadata": {"name": "web-1-x", "namespace": "a", "ownerReferences": owned("ReplicaSet", "web-1")},
                "spec": {"containers": [{"name": "app", "image": "nginx:1.25"}], "imagePullSecrets": [{"name": "reg"}]},
                "status": {"containerStatuses": [{"name": "app", "image": "nginx:1.25",
                    "imageID": "docker.io/library/nginx@sha256:abc", "ready": true, "restartCount": 0,
                    "state": {"running": {}}}]}
            }))],
            ..Default::default()
        };
        let u = uses(&o);
        assert_eq!(
            u.len(),
            1,
            "the Deployment has a pod, so no template use: {u:?}"
        );
        assert_eq!(u[0].owner_kind, "Deployment");
        assert_eq!(u[0].owner_name, "web");
        assert_eq!(u[0].image_id.as_deref(), Some("sha256:abc"));
        assert!(u[0].running);
        assert_eq!(u[0].pull_secrets, ["reg"]);
    }

    #[test]
    fn idle_cron_job_template_is_listed_without_a_digest() {
        let o = Objects {
            cron_jobs: vec![from(
                json!({"metadata": {"name": "nightly", "namespace": "b"},
                "spec": {"schedule": "0 0 * * *", "jobTemplate": {"spec": {"template": template("busybox:1.37")}}}}),
            )],
            ..Default::default()
        };
        let u = uses(&o);
        assert_eq!(u.len(), 1);
        assert_eq!(
            (u[0].owner_kind.as_str(), u[0].owner_name.as_str()),
            ("CronJob", "nightly")
        );
        assert_eq!(u[0].image, "busybox:1.37");
        assert_eq!(u[0].image_id, None);
        assert!(!u[0].running);
    }

    #[test]
    fn cron_job_pod_rolls_up_to_the_cron_job() {
        let o = Objects {
            jobs: vec![from(
                json!({"metadata": {"name": "nightly-1", "namespace": "b",
                "ownerReferences": owned("CronJob", "nightly")}, "spec": {"template": template("busybox:1.37")}}),
            )],
            cron_jobs: vec![from(
                json!({"metadata": {"name": "nightly", "namespace": "b"},
                "spec": {"schedule": "0 0 * * *", "jobTemplate": {"spec": {"template": template("busybox:1.37")}}}}),
            )],
            pods: vec![from(json!({
                "metadata": {"name": "nightly-1-x", "namespace": "b", "ownerReferences": owned("Job", "nightly-1")},
                "spec": {"containers": [{"name": "app", "image": "busybox:1.37"}]}
            }))],
            ..Default::default()
        };
        let u = uses(&o);
        assert_eq!(u.len(), 1, "{u:?}");
        assert_eq!(u[0].owner_kind, "CronJob");
        assert!(!u[0].running, "no status yet");
    }

    #[test]
    fn digest_of_reads_every_runtime_spelling() {
        assert_eq!(
            digest_of("docker-pullable://nginx@sha256:1").as_deref(),
            Some("sha256:1")
        );
        assert_eq!(
            digest_of("docker.io/library/nginx@sha256:2").as_deref(),
            Some("sha256:2")
        );
        assert_eq!(digest_of("sha256:3").as_deref(), Some("sha256:3"));
        assert_eq!(digest_of(""), None);
    }
}
