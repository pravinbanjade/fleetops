//! One row per object (spec §2.4): the only kind-specific code. Objects
//! that don't parse as their typed struct still get a row, with health
//! Unknown, so one odd object never fails a whole list.

use crate::kinds::typed_json;
use fleetops_core::kube::{
    KubeCondition, KubeKind, KubeRef, KubeRow, RowHealth, SUSPENDED_ANNOTATION,
};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    autoscaling::v2::HorizontalPodAutoscaler,
    batch::v1::{CronJob, Job},
    core::v1::{
        ConfigMap, ContainerStatus, Event, Namespace, Node, PersistentVolume,
        PersistentVolumeClaim, Pod, PodSpec, PodStatus, Secret, Service, ServiceAccount,
    },
    networking::v1::{Ingress, NetworkPolicy},
    rbac::v1::{ClusterRole, ClusterRoleBinding, PolicyRule, Role, RoleBinding, RoleRef, Subject},
    storage::v1::StorageClass,
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
        KubeKind::ReplicaSets => typed::<ReplicaSet>(kind, obj).map(replicaset),
        KubeKind::HorizontalPodAutoscalers => typed::<HorizontalPodAutoscaler>(kind, obj).map(hpa),
        KubeKind::NetworkPolicies => typed::<NetworkPolicy>(kind, obj).map(network_policy),
        KubeKind::PersistentVolumeClaims => typed::<PersistentVolumeClaim>(kind, obj).map(pvc),
        KubeKind::PersistentVolumes => typed::<PersistentVolume>(kind, obj).map(pv),
        KubeKind::StorageClasses => typed::<StorageClass>(kind, obj).map(storage_class),
        KubeKind::ServiceAccounts => typed::<ServiceAccount>(kind, obj).map(service_account),
        KubeKind::Roles => typed::<Role>(kind, obj).map(|r| rules(r.rules)),
        KubeKind::ClusterRoles => typed::<ClusterRole>(kind, obj).map(|r| rules(r.rules)),
        KubeKind::RoleBindings => {
            typed::<RoleBinding>(kind, obj).map(|b| binding(b.role_ref, b.subjects))
        }
        KubeKind::ClusterRoleBindings => {
            typed::<ClusterRoleBinding>(kind, obj).map(|b| binding(b.role_ref, b.subjects))
        }
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
    let squares = container_states(p.spec.as_ref(), &st);
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
            ("containers", squares),
            ("restarts", restarts.to_string()),
            ("node", node),
            ("ip", st.pod_ip.unwrap_or_default()),
        ]),
    )
}

/// One `kind:name:state:reason` entry per container, init containers (`i`)
/// before the rest (`c`), joined by `;`; container names are DNS labels, so
/// neither separator can appear in one. A state is `ready`, `running` (not
/// ready yet), `waiting` (on its way up), `error`, `done` or `unknown` (no
/// status reported yet).
fn container_states(spec: Option<&PodSpec>, st: &PodStatus) -> String {
    let entry = |kind: &str, name: &str, cs: Option<&ContainerStatus>| {
        let (state, reason) = cs.map_or(("unknown", String::new()), container_state);
        format!("{kind}:{name}:{state}:{reason}")
    };
    fn find<'a>(all: &'a Option<Vec<ContainerStatus>>, name: &str) -> Option<&'a ContainerStatus> {
        all.as_ref()?.iter().find(|c| c.name == name)
    }
    let Some(spec) = spec else {
        return String::new();
    };
    let init = spec
        .init_containers
        .iter()
        .flatten()
        .map(|c| entry("i", &c.name, find(&st.init_container_statuses, &c.name)));
    let main = spec
        .containers
        .iter()
        .map(|c| entry("c", &c.name, find(&st.container_statuses, &c.name)));
    init.chain(main).collect::<Vec<_>>().join(";")
}

fn container_state(c: &ContainerStatus) -> (&'static str, String) {
    let Some(state) = c.state.as_ref() else {
        return ("unknown", String::new());
    };
    if let Some(t) = &state.terminated {
        let reason = t
            .reason
            .clone()
            .unwrap_or_else(|| format!("exit {}", t.exit_code));
        return (if t.exit_code == 0 { "done" } else { "error" }, reason);
    }
    if let Some(w) = &state.waiting {
        let reason = w.reason.clone().unwrap_or_default();
        let normal = matches!(
            reason.as_str(),
            "" | "ContainerCreating" | "PodInitializing"
        );
        return (if normal { "waiting" } else { "error" }, reason);
    }
    if state.running.is_some() {
        return (if c.ready { "ready" } else { "running" }, String::new());
    }
    ("unknown", String::new())
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

fn replicaset(r: ReplicaSet) -> Summary {
    let want = r.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
    let ready = r
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let owner = r
        .metadata
        .owner_references
        .unwrap_or_default()
        .into_iter()
        .next()
        .map(|o| o.name)
        .unwrap_or_default();
    // A Deployment keeps its old ReplicaSets at 0: history, not a fault.
    let (status, h) = if want == 0 {
        ("0/0".to_string(), RowHealth::Off)
    } else {
        replicas(ready, want)
    };
    (
        status,
        h,
        cols([
            ("desired", want.to_string()),
            ("ready", ready.to_string()),
            ("owner", owner),
        ]),
    )
}

/// `metric: current/target` for each metric the HPA scales on.
fn hpa_targets(h: &HorizontalPodAutoscaler) -> String {
    let current = h
        .status
        .as_ref()
        .and_then(|s| s.current_metrics.clone())
        .unwrap_or_default();
    h.spec
        .as_ref()
        .and_then(|s| s.metrics.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            let r = m.resource?;
            let want = r.target.average_utilization?;
            let now = current
                .iter()
                .filter_map(|c| c.resource.as_ref())
                .find(|c| c.name == r.name)
                .and_then(|c| c.current.average_utilization)
                .map(|v| format!("{v}%"))
                .unwrap_or_else(|| "?".into());
            Some(format!("{}: {now}/{want}%", r.name))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn hpa(h: HorizontalPodAutoscaler) -> Summary {
    let targets = hpa_targets(&h);
    let spec = h.spec.unwrap_or_default();
    let st = h.status.unwrap_or_default();
    let min = spec.min_replicas.unwrap_or(1);
    let max = spec.max_replicas;
    let current = st.current_replicas.unwrap_or(0);
    let stuck = st
        .conditions
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.type_ == "ScalingActive" && c.status == "False");
    let target_kind = KubeKind::from_kind_name(&spec.scale_target_ref.kind)
        .map(|k| k.as_str().to_string())
        .unwrap_or_default();
    let (status, health) = match stuck {
        Some(c) => (
            c.reason.unwrap_or_else(|| "ScalingInactive".into()),
            RowHealth::Bad,
        ),
        None if max > 0 && current >= max => (format!("{current}/{max}"), RowHealth::Warn),
        None => (format!("{current}/{max}"), RowHealth::Ok),
    };
    (
        status,
        health,
        cols([
            ("target", spec.scale_target_ref.name),
            ("target_kind", target_kind),
            ("min", min.to_string()),
            ("max", max.to_string()),
            ("current", current.to_string()),
            ("targets", targets),
        ]),
    )
}

fn network_policy(n: NetworkPolicy) -> Summary {
    let spec = n.spec.unwrap_or_default();
    let labels = spec
        .pod_selector
        .and_then(|s| s.match_labels)
        .unwrap_or_default();
    let selector = if labels.is_empty() {
        "(all pods)".to_string()
    } else {
        labels
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    // Kubernetes' own default when policyTypes is unset.
    let types = spec
        .policy_types
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| vec!["Ingress".into()]);
    (
        types.join(", "),
        RowHealth::Ok,
        cols([
            ("pod_selector", selector),
            ("policy_types", types.join(", ")),
        ]),
    )
}

/// kubectl's short access modes: RWO, ROX, RWX, RWOP.
fn access_modes(modes: Option<Vec<String>>) -> String {
    modes
        .unwrap_or_default()
        .iter()
        .map(|m| match m.as_str() {
            "ReadWriteOnce" => "RWO",
            "ReadOnlyMany" => "ROX",
            "ReadWriteMany" => "RWX",
            "ReadWriteOncePod" => "RWOP",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn storage(
    q: Option<&BTreeMap<String, k8s_openapi::apimachinery::pkg::api::resource::Quantity>>,
) -> String {
    q.and_then(|m| m.get("storage"))
        .map(|q| q.0.clone())
        .unwrap_or_default()
}

fn pvc(c: PersistentVolumeClaim) -> Summary {
    let spec = c.spec.unwrap_or_default();
    let st = c.status.unwrap_or_default();
    let phase = st.phase.unwrap_or_else(|| "Pending".into());
    let h = match phase.as_str() {
        "Bound" => RowHealth::Ok,
        "Lost" => RowHealth::Bad,
        _ => RowHealth::Warn,
    };
    (
        phase,
        h,
        cols([
            ("volume", spec.volume_name.unwrap_or_default()),
            ("capacity", storage(st.capacity.as_ref())),
            ("access_modes", access_modes(spec.access_modes)),
            ("storage_class", spec.storage_class_name.unwrap_or_default()),
        ]),
    )
}

fn pv(v: PersistentVolume) -> Summary {
    let spec = v.spec.unwrap_or_default();
    let phase = v
        .status
        .and_then(|s| s.phase)
        .unwrap_or_else(|| "Pending".into());
    let h = match phase.as_str() {
        "Bound" | "Available" => RowHealth::Ok,
        "Failed" => RowHealth::Bad,
        _ => RowHealth::Warn,
    };
    let claim = spec.claim_ref.clone().unwrap_or_default();
    (
        phase,
        h,
        cols([
            ("claim", claim.name.unwrap_or_default()),
            ("claim_namespace", claim.namespace.unwrap_or_default()),
            ("capacity", storage(spec.capacity.as_ref())),
            ("access_modes", access_modes(spec.access_modes)),
            (
                "reclaim_policy",
                spec.persistent_volume_reclaim_policy
                    .unwrap_or_else(|| "Retain".into()),
            ),
            ("storage_class", spec.storage_class_name.unwrap_or_default()),
        ]),
    )
}

fn storage_class(s: StorageClass) -> Summary {
    let default = s
        .metadata
        .annotations
        .unwrap_or_default()
        .get("storageclass.kubernetes.io/is-default-class")
        .is_some_and(|v| v == "true");
    (
        s.provisioner.clone(),
        RowHealth::Ok,
        cols([
            ("provisioner", s.provisioner),
            (
                "reclaim_policy",
                s.reclaim_policy.unwrap_or_else(|| "Delete".into()),
            ),
            (
                "binding_mode",
                s.volume_binding_mode.unwrap_or_else(|| "Immediate".into()),
            ),
            ("default", if default { "Yes" } else { "" }.into()),
        ]),
    )
}

fn service_account(a: ServiceAccount) -> Summary {
    let n = a.secrets.map(|s| s.len()).unwrap_or(0);
    (
        String::new(),
        RowHealth::Ok,
        cols([("secrets", n.to_string())]),
    )
}

fn rules(r: Option<Vec<PolicyRule>>) -> Summary {
    let n = r.map(|r| r.len()).unwrap_or(0);
    (
        format!("{n} rules"),
        RowHealth::Ok,
        cols([("rules", n.to_string())]),
    )
}

fn binding(role: RoleRef, subjects: Option<Vec<Subject>>) -> Summary {
    let subjects = subjects
        .unwrap_or_default()
        .into_iter()
        .map(|s| format!("{}/{}", s.kind, s.name))
        .collect::<Vec<_>>()
        .join(", ");
    let role_kind = KubeKind::from_kind_name(&role.kind)
        .map(|k| k.as_str().to_string())
        .unwrap_or_default();
    (
        role.name.clone(),
        RowHealth::Ok,
        cols([
            ("role", role.name),
            ("role_kind", role_kind),
            ("subjects", subjects),
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

/// A row of a discovered kind (spec 3g §2.3): printer columns, and health
/// from the `Ready` condition when the object has one.
pub fn generic_row(k: &fleetops_core::kube::ApiKind, obj: &DynamicObject) -> KubeRow {
    let m = &obj.metadata;
    let v = serde_json::to_value(obj).unwrap_or_default();
    let columns = k
        .columns
        .iter()
        .map(|c| (c.name.clone(), crate::printer::evaluate(c, &v)))
        .collect();
    let (status, health) = match conditions(obj).into_iter().find(|c| c.type_ == "Ready") {
        Some(c) => {
            let h = match c.status.as_str() {
                "True" => RowHealth::Ok,
                "False" => RowHealth::Bad,
                _ => RowHealth::Warn,
            };
            let fallback = if c.status == "True" {
                "Ready"
            } else {
                "NotReady"
            };
            (
                c.reason
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| fallback.into()),
                h,
            )
        }
        None => (String::new(), RowHealth::Unknown),
    };
    KubeRow {
        name: m.name.clone().unwrap_or_default(),
        namespace: if k.namespaced {
            m.namespace.clone()
        } else {
            None
        },
        created: m
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.as_second())
            .unwrap_or(0),
        status,
        health,
        columns,
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
