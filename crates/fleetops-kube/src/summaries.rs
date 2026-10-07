//! One row per object (spec §2.4): the only kind-specific code. Objects
//! that don't parse as their typed struct still get a row, with health
//! Unknown, so one odd object never fails a whole list.

use crate::kinds::typed_json;
use fleetops_core::kube::{
    KubeCondition, KubeKind, KubeRef, KubeRow, RowHealth, SUSPENDED_ANNOTATION,
};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{ConfigMap, Event, Namespace, Node, Pod, Secret, Service},
    networking::v1::Ingress,
};
use kube::api::DynamicObject;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

type Cols = BTreeMap<String, String>;

fn typed<K: DeserializeOwned>(kind: KubeKind, obj: &DynamicObject) -> Option<K> {
    serde_json::from_value(typed_json(kind, obj)).ok()
}

fn cols<const N: usize>(pairs: [(&str, String); N]) -> Cols {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `ready` of `want`, with the health that implies.
fn replicas(ready: i32, want: i32) -> (String, RowHealth) {
    let h = if ready >= want {
        RowHealth::Ok
    } else if ready > 0 {
        RowHealth::Warn
    } else {
        RowHealth::Bad
    };
    (format!("{ready}/{want}"), h)
}

pub fn row(kind: KubeKind, obj: &DynamicObject) -> KubeRow {
    let m = &obj.metadata;
    let base = KubeRow {
        name: m.name.clone().unwrap_or_default(),
        namespace: if kind.cluster_scoped() {
            None
        } else {
            m.namespace.clone()
        },
        created: m
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.as_second())
            .unwrap_or(0),
        status: String::new(),
        health: RowHealth::Unknown,
        columns: Cols::new(),
    };
    let summary = match kind {
        KubeKind::Namespaces => typed::<Namespace>(kind, obj).map(namespace),
        KubeKind::Nodes => typed::<Node>(kind, obj).map(node),
        KubeKind::Deployments => typed::<Deployment>(kind, obj).map(deployment),
        KubeKind::StatefulSets => typed::<StatefulSet>(kind, obj).map(statefulset),
        KubeKind::DaemonSets => typed::<DaemonSet>(kind, obj).map(daemonset),
        KubeKind::Jobs => typed::<Job>(kind, obj).map(job),
        KubeKind::CronJobs => typed::<CronJob>(kind, obj).map(cronjob),
        KubeKind::Pods => typed::<Pod>(kind, obj).map(pod),
        KubeKind::Services => typed::<Service>(kind, obj).map(service),
        KubeKind::Ingresses => typed::<Ingress>(kind, obj).map(ingress),
        KubeKind::ConfigMaps => typed::<ConfigMap>(kind, obj).map(configmap),
        KubeKind::Secrets => typed::<Secret>(kind, obj).map(secret),
        KubeKind::Events => typed::<Event>(kind, obj).map(event),
        _ => None,
    };
    match summary {
        Some((status, health, columns)) => KubeRow {
            status,
            health,
            columns,
            ..base
        },
        None => KubeRow {
            status: "Unknown".into(),
            ..base
        },
    }
}

type Summary = (String, RowHealth, Cols);

fn namespace(n: Namespace) -> Summary {
    let phase = n
        .status
        .and_then(|s| s.phase)
        .unwrap_or_else(|| "Active".into());
    let h = if phase == "Active" {
        RowHealth::Ok
    } else {
        RowHealth::Warn
    };
    (phase, h, Cols::new())
}

fn node(n: Node) -> Summary {
    let roles: Vec<String> = n
        .metadata
        .labels
        .unwrap_or_default()
        .keys()
        .filter_map(|k| {
            k.strip_prefix("node-role.kubernetes.io/")
                .map(str::to_string)
        })
        .collect();
    let cordoned = n.spec.and_then(|s| s.unschedulable).unwrap_or(false);
    let status = n.status.unwrap_or_default();
    let ready = status
        .conditions
        .unwrap_or_default()
        .into_iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");
    let ip = status
        .addresses
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.type_ == "InternalIP")
        .map(|a| a.address)
        .unwrap_or_default();
    let version = status
        .node_info
        .map(|i| i.kubelet_version)
        .unwrap_or_default();
    let (s, h) = match (ready, cordoned) {
        // kubectl's words; nothing new lands there until it is uncordoned.
        (true, true) => ("Ready,SchedulingDisabled", RowHealth::Warn),
        (true, false) => ("Ready", RowHealth::Ok),
        (false, true) => ("NotReady,SchedulingDisabled", RowHealth::Bad),
        (false, false) => ("NotReady", RowHealth::Bad),
    };
    let mut c = cols([
        ("roles", roles.join(",")),
        ("version", version),
        ("internal_ip", ip),
    ]);
    if cordoned {
        c.insert("unschedulable".into(), "true".into());
    }
    (s.into(), h, c)
}

/// The replicas a FleetOps suspend put away, when the object is still at
/// zero. Scaled up by hand since, it is simply running again.
fn suspended(
    meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    want: i32,
) -> Option<String> {
    let kept = meta.annotations.as_ref()?.get(SUSPENDED_ANNOTATION)?;
    (want == 0).then(|| kept.clone())
}

fn suspended_summary(ready: String, kept: String, mut extra: Cols) -> Summary {
    extra.insert("ready".into(), ready);
    extra.insert("suspended_replicas".into(), kept);
    ("Suspended".into(), RowHealth::Off, extra)
}

fn deployment(d: Deployment) -> Summary {
    let want = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
    if let Some(kept) = suspended(&d.metadata, want) {
        let st = d.status.unwrap_or_default();
        return suspended_summary(
            format!("{}/0", st.ready_replicas.unwrap_or(0)),
            kept,
            cols([
                ("up_to_date", st.updated_replicas.unwrap_or(0).to_string()),
                ("available", st.available_replicas.unwrap_or(0).to_string()),
            ]),
        );
    }
    let st = d.status.unwrap_or_default();
    let available = st.available_replicas.unwrap_or(0);
    let (ready, h) = replicas(st.ready_replicas.unwrap_or(0), want);
    let h = if want == 0 {
        RowHealth::Ok
    } else {
        replicas(available, want).1.min_with(h)
    };
    (
        ready.clone(),
        h,
        cols([
            ("ready", ready),
            ("up_to_date", st.updated_replicas.unwrap_or(0).to_string()),
            ("available", available.to_string()),
        ]),
    )
}

fn statefulset(s: StatefulSet) -> Summary {
    let want = s.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
    if let Some(kept) = suspended(&s.metadata, want) {
        let ready = s.status.and_then(|s| s.ready_replicas).unwrap_or(0);
        return suspended_summary(format!("{ready}/0"), kept, Cols::new());
    }
    let (ready, h) = replicas(s.status.and_then(|s| s.ready_replicas).unwrap_or(0), want);
    let h = if want == 0 { RowHealth::Ok } else { h };
    (ready.clone(), h, cols([("ready", ready)]))
}

fn daemonset(d: DaemonSet) -> Summary {
    let st = d.status.unwrap_or_default();
    let (ready, h) = replicas(st.number_ready, st.desired_number_scheduled);
    (
        ready.clone(),
        h,
        cols([
            ("desired", st.desired_number_scheduled.to_string()),
            ("current", st.current_number_scheduled.to_string()),
            ("ready", st.number_ready.to_string()),
            ("available", st.number_available.unwrap_or(0).to_string()),
        ]),
    )
}

fn job(j: Job) -> Summary {
    let want = j.spec.and_then(|s| s.completions).unwrap_or(1);
    let st = j.status.unwrap_or_default();
    let done = |t: &str| {
        st.conditions
            .iter()
            .flatten()
            .any(|c| c.type_ == t && c.status == "True")
    };
    let (s, h) = if done("Complete") {
        ("Complete", RowHealth::Ok)
    } else if done("Failed") {
        ("Failed", RowHealth::Bad)
    } else {
        ("Running", RowHealth::Ok)
    };
    (
        s.into(),
        h,
        cols([(
            "completions",
            format!("{}/{want}", st.succeeded.unwrap_or(0)),
        )]),
    )
}

fn cronjob(c: CronJob) -> Summary {
    let spec = c.spec.unwrap_or_default();
    let st = c.status.unwrap_or_default();
    let active = st.active.map(|a| a.len()).unwrap_or(0);
    let (s, h) = if spec.suspend.unwrap_or(false) {
        ("Suspended", RowHealth::Off)
    } else if active > 0 {
        ("Active", RowHealth::Ok)
    } else {
        ("Scheduled", RowHealth::Ok)
    };
    (
        s.into(),
        h,
        cols([
            ("schedule", spec.schedule),
            ("active", active.to_string()),
            (
                "last_schedule",
                st.last_schedule_time
                    .map(|t| t.0.to_string())
                    .unwrap_or_default(),
            ),
        ]),
    )
}

fn pod(p: Pod) -> Summary {
    let terminating = p.metadata.deletion_timestamp.is_some();
    let node = p
        .spec
        .as_ref()
        .and_then(|s| s.node_name.clone())
        .unwrap_or_default();
    let want = p.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0);
    let st = p.status.unwrap_or_default();
    let cs = st.container_statuses.unwrap_or_default();
    let ready = cs.iter().filter(|c| c.ready).count();
    let restarts: i32 = cs.iter().map(|c| c.restart_count).sum();
    let reason = cs.iter().find_map(|c| {
        let state = c.state.as_ref()?;
        state
            .waiting
            .as_ref()
            .and_then(|w| w.reason.clone())
            .or_else(|| state.terminated.as_ref().and_then(|t| t.reason.clone()))
    });
    let phase = st.phase.unwrap_or_else(|| "Pending".into());
    let (status, health) = if terminating {
        ("Terminating".to_string(), RowHealth::Warn)
    } else if let Some(r) = reason.filter(|r| r != "Completed") {
        (r, RowHealth::Bad)
    } else {
        let h = match phase.as_str() {
            "Running" if ready == want => RowHealth::Ok,
            "Succeeded" => RowHealth::Ok,
            "Failed" => RowHealth::Bad,
            _ => RowHealth::Warn,
        };
        (phase, h)
    };
    (
        status,
        health,
        cols([
            ("ready", format!("{ready}/{want}")),
            ("restarts", restarts.to_string()),
            ("node", node),
            ("ip", st.pod_ip.unwrap_or_default()),
        ]),
    )
}

fn service(s: Service) -> Summary {
    let spec = s.spec.unwrap_or_default();
    let ports: Vec<String> = spec
        .ports
        .unwrap_or_default()
        .into_iter()
        .map(|p| format!("{}/{}", p.port, p.protocol.unwrap_or_else(|| "TCP".into())))
        .collect();
    let external: Vec<String> = s
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|l| l.ingress)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| i.ip.or(i.hostname))
        .collect();
    (
        spec.type_.unwrap_or_else(|| "ClusterIP".into()),
        RowHealth::Ok,
        cols([
            ("cluster_ip", spec.cluster_ip.unwrap_or_default()),
            ("external_ip", external.join(", ")),
            ("ports", ports.join(", ")),
        ]),
    )
}

fn ingress(i: Ingress) -> Summary {
    let spec = i.spec.unwrap_or_default();
    let hosts: Vec<String> = spec
        .rules
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.host)
        .collect();
    let address: Vec<String> = i
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|l| l.ingress)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| i.ip.or(i.hostname))
        .collect();
    let hosts = hosts.join(", ");
    (
        if hosts.is_empty() {
            "*".into()
        } else {
            hosts.clone()
        },
        RowHealth::Ok,
        cols([
            ("class", spec.ingress_class_name.unwrap_or_default()),
            ("hosts", hosts),
            ("address", address.join(", ")),
        ]),
    )
}

fn configmap(c: ConfigMap) -> Summary {
    let n = c.data.map(|d| d.len()).unwrap_or(0) + c.binary_data.map(|d| d.len()).unwrap_or(0);
    (
        format!("{n} keys"),
        RowHealth::Ok,
        cols([("keys", n.to_string())]),
    )
}

fn secret(s: Secret) -> Summary {
    let n = s.data.map(|d| d.len()).unwrap_or(0);
    (
        s.type_.unwrap_or_else(|| "Opaque".into()),
        RowHealth::Ok,
        cols([("keys", n.to_string())]),
    )
}

/// An event: its type as the status, what it is about, and when it was
/// last seen (unix seconds) so the list can put the newest first.
fn event(e: Event) -> Summary {
    let type_ = e.type_.unwrap_or_else(|| "Normal".into());
    let h = if type_ == "Warning" {
        RowHealth::Warn
    } else {
        RowHealth::Ok
    };
    let last_seen = e
        .last_timestamp
        .map(|t| t.0.as_second())
        .or_else(|| {
            e.series
                .and_then(|s| s.last_observed_time)
                .map(|t| t.0.as_second())
        })
        .or_else(|| e.event_time.map(|t| t.0.as_second()))
        .or_else(|| e.metadata.creation_timestamp.map(|t| t.0.as_second()))
        .unwrap_or(0);
    let o = e.involved_object;
    let object_kind = o.kind.unwrap_or_default();
    let link = KubeKind::from_kind_name(&object_kind)
        .map(|k| k.as_str().to_string())
        .unwrap_or_default();
    let source = e
        .source
        .and_then(|s| s.component)
        .or(e.reporting_component)
        .unwrap_or_default();
    (
        type_,
        h,
        cols([
            ("reason", e.reason.unwrap_or_default()),
            ("object_kind", object_kind),
            ("object_name", o.name.unwrap_or_default()),
            ("object_namespace", o.namespace.unwrap_or_default()),
            ("object_link", link),
            ("message", e.message.unwrap_or_default()),
            ("count", e.count.unwrap_or(1).max(1).to_string()),
            ("last_seen", last_seen.to_string()),
            ("source", source),
        ]),
    )
}

trait Worst {
    fn min_with(self, other: RowHealth) -> RowHealth;
}

impl Worst for RowHealth {
    /// The worse of two healths.
    fn min_with(self, other: RowHealth) -> RowHealth {
        let rank = |h: RowHealth| match h {
            RowHealth::Bad => 0,
            RowHealth::Warn => 1,
            RowHealth::Unknown => 2,
            RowHealth::Ok | RowHealth::Off => 3,
        };
        if rank(self) <= rank(other) {
            self
        } else {
            other
        }
    }
}

pub fn conditions(obj: &DynamicObject) -> Vec<KubeCondition> {
    obj.data["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(KubeCondition {
                type_: c["type"].as_str()?.to_string(),
                status: c["status"].as_str().unwrap_or_default().to_string(),
                reason: c["reason"].as_str().map(str::to_string),
                message: c["message"].as_str().map(str::to_string),
            })
        })
        .collect()
}

pub fn owners(obj: &DynamicObject) -> Vec<KubeRef> {
    obj.metadata
        .owner_references
        .iter()
        .flatten()
        .map(|o| KubeRef {
            kind: o.kind.clone(),
            name: o.name.clone(),
            link: KubeKind::from_kind_name(&o.kind),
        })
        .collect()
}

pub fn containers(obj: &DynamicObject) -> Vec<String> {
    obj.data["spec"]["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .collect()
}
