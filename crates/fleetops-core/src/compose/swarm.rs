//! A Swarm stack's services as Docker ServiceSpecs (swarm spec §5), and
//! how two specs compare: drift, plans and the spec-hash label all go
//! through [`canonical`], which absorbs what Docker fills in. Pure.

use super::model::*;
use crate::swarm::STACK_LABEL;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The hash of the spec FleetOps deployed, on the service itself.
pub const LABEL_SPEC_HASH: &str = "io.fleetops.stack.spec-hash";
/// The image as written, as `docker stack deploy` labels it.
pub const LABEL_STACK_IMAGE: &str = "com.docker.stack.image";

/// A stack exercising every field the renderer maps, for tests here and in
/// the Docker crate. Its secret takes [`sample_variables`].
#[doc(hidden)]
pub const SAMPLE_STACK: &str = r#"
services:
  web:
    image: nginx:1.27
    entrypoint: ["/docker-entrypoint.sh"]
    command: ["nginx", "-g", "daemon off;"]
    environment: { MODE: prod, DB_PASSWORD: hunter2 }
    labels: { tier: front }
    user: "101"
    working_dir: /srv
    hostname: web
    extra_hosts: ["db.local:10.0.0.2"]
    dns: [1.1.1.1]
    cap_add: [NET_ADMIN]
    cap_drop: [ALL]
    init: true
    read_only: true
    stop_signal: SIGQUIT
    stop_grace_period: 20s
    healthcheck: { test: ["CMD", "true"], interval: 10s, timeout: 2s, retries: 3, start_period: 5s }
    logging: { driver: json-file, options: { max-size: 10m } }
    ports: ["8080:80", { target: 443, published: 8443, mode: host }]
    volumes: ["data:/data", "/srv/static:/static:ro", "/cache"]
    networks: { front: { aliases: [www] } }
    secrets: [db]
    configs: [{ source: site, target: /etc/nginx/conf.d/site.conf, mode: 0440 }]
    deploy:
      replicas: 3
      placement:
        constraints: ["node.role == worker"]
        preferences: [{ spread: node.labels.zone }]
        max_replicas_per_node: 2
      resources:
        limits: { cpus: "0.5", memory: 128M, pids: 100 }
        reservations: { cpus: "0.25", memory: 64M }
      restart_policy: { condition: on-failure, delay: 5s, max_attempts: 3, window: 2m }
      update_config: { parallelism: 2, delay: 10s, failure_action: rollback, monitor: 30s, max_failure_ratio: 0.1, order: start-first }
      rollback_config: { parallelism: 1, failure_action: pause }
      labels: { team: web }
      endpoint_mode: vip
networks:
  front: { driver: overlay, attachable: true }
volumes:
  data: { driver: local, driver_opts: { type: none } }
secrets:
  db: { environment: DB_PW }
configs:
  site: { content: "server {}" }
"#;

/// The variables [`SAMPLE_STACK`] takes.
#[doc(hidden)]
pub fn sample_variables() -> Vec<String> {
    vec!["DB_PW=s3cr3t-value".to_string()]
}

/// `<stack>_<key>`.
pub fn service_name(project: &Project, key: &str) -> String {
    format!("{}_{key}", project.name)
}

/// The Docker name of a secret or config: an external one's own name, or
/// `<stack>_<key>_<digest>` so a new value makes a new object (Swarm's are
/// immutable).
pub fn object_name(project: &Project, kind: FileKind, key: &str) -> String {
    let decls = match kind {
        FileKind::Secret => &project.secrets,
        FileKind::Config => &project.configs,
    };
    if let Some(FileSource::External { name }) = decls.get(key) {
        return name.clone();
    }
    let digest = project
        .services
        .values()
        .flat_map(|s| &s.files)
        .find(|f| f.kind == kind && f.source == key)
        .map(|f| f.digest.as_str())
        .unwrap_or_default();
    format!("{}_{key}_{}", project.name, &digest[..digest.len().min(10)])
}

const NS: i64 = 1_000_000_000;

fn secs(n: Option<i64>) -> Value {
    n.map(|s| json!(s * NS)).unwrap_or(Value::Null)
}

/// Sets `key` unless `v` is null, so the spec carries only what was said.
fn put(o: &mut Map<String, Value>, key: &str, v: Value) {
    if !v.is_null() {
        o.insert(key.to_string(), v);
    }
}

fn update_policy(u: &Option<UpdatePolicy>) -> Value {
    let Some(u) = u else { return Value::Null };
    let mut o = Map::new();
    put(&mut o, "Parallelism", json!(u.parallelism));
    put(&mut o, "Delay", secs(u.delay_secs));
    put(&mut o, "FailureAction", json!(u.failure_action));
    put(&mut o, "Monitor", secs(u.monitor_secs));
    put(&mut o, "MaxFailureRatio", json!(u.max_failure_ratio));
    put(&mut o, "Order", json!(u.order));
    Value::Object(o)
}

/// The ServiceSpec of service `key`, with its spec-hash label.
pub fn service_spec(project: &Project, key: &str) -> Value {
    let svc = &project.services[key];
    let d = svc.swarm.clone().unwrap_or_default();
    let stack = &project.name;

    let mut cs = Map::new();
    cs.insert("Image".into(), json!(svc.image));
    let mut labels = svc.labels.clone();
    labels.insert(STACK_LABEL.into(), stack.clone());
    cs.insert("Labels".into(), json!(labels));
    put(&mut cs, "Command", json!(svc.entrypoint));
    put(&mut cs, "Args", json!(svc.command));
    put(&mut cs, "Hostname", json!(svc.hostname));
    if !svc.environment.is_empty() {
        let env: Vec<String> = svc
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        cs.insert("Env".into(), json!(env));
    }
    put(&mut cs, "Dir", json!(svc.working_dir));
    put(&mut cs, "User", json!(svc.user));
    if svc.read_only {
        cs.insert("ReadOnly".into(), json!(true));
    }
    put(&mut cs, "Init", json!(svc.init));
    let mounts: Vec<Value> = svc.mounts.iter().map(|m| mount(project, m)).collect();
    if !mounts.is_empty() {
        cs.insert("Mounts".into(), json!(mounts));
    }
    put(&mut cs, "StopSignal", json!(svc.stop_signal));
    put(&mut cs, "StopGracePeriod", secs(svc.stop_grace_period_secs));
    if let Some(h) = &svc.healthcheck {
        cs.insert(
            "HealthCheck".into(),
            if h.disable {
                json!({"Test": ["NONE"]})
            } else {
                json!({
                    "Test": h.test,
                    "Interval": h.interval_secs * NS,
                    "Timeout": h.timeout_secs * NS,
                    "Retries": h.retries,
                    "StartPeriod": h.start_period_secs * NS,
                })
            },
        );
    }
    if !svc.extra_hosts.is_empty() {
        let hosts: Vec<String> = svc
            .extra_hosts
            .iter()
            .map(|h| match h.split_once(':') {
                Some((host, ip)) => format!("{ip} {host}"),
                None => h.clone(),
            })
            .collect();
        cs.insert("Hosts".into(), json!(hosts));
    }
    if !svc.dns.is_empty() {
        cs.insert("DNSConfig".into(), json!({"Nameservers": svc.dns}));
    }
    if !svc.cap_add.is_empty() {
        cs.insert("CapabilityAdd".into(), json!(svc.cap_add));
    }
    if !svc.cap_drop.is_empty() {
        cs.insert("CapabilityDrop".into(), json!(svc.cap_drop));
    }
    for (kind, field, name_key) in [
        (FileKind::Secret, "Secrets", "SecretName"),
        (FileKind::Config, "Configs", "ConfigName"),
    ] {
        let refs: Vec<Value> = svc
            .files
            .iter()
            .filter(|f| f.kind == kind)
            .map(|f| {
                json!({
                    "File": {"Name": f.target, "UID": f.uid, "GID": f.gid, "Mode": f.mode},
                    name_key: object_name(project, kind, &f.source),
                })
            })
            .collect();
        if !refs.is_empty() {
            cs.insert(field.into(), json!(refs));
        }
    }

    let mut tt = Map::new();
    tt.insert("ContainerSpec".into(), Value::Object(cs));
    let mut limits = Map::new();
    put(&mut limits, "NanoCPUs", json!(svc.nano_cpus));
    put(&mut limits, "MemoryBytes", json!(svc.memory_bytes));
    put(&mut limits, "Pids", json!(d.pids_limit));
    let mut reserve = Map::new();
    put(&mut reserve, "NanoCPUs", json!(d.reserve_nano_cpus));
    put(&mut reserve, "MemoryBytes", json!(d.reserve_memory_bytes));
    let mut resources = Map::new();
    if !limits.is_empty() {
        resources.insert("Limits".into(), Value::Object(limits));
    }
    if !reserve.is_empty() {
        resources.insert("Reservations".into(), Value::Object(reserve));
    }
    if !resources.is_empty() {
        tt.insert("Resources".into(), Value::Object(resources));
    }
    if let Some(r) = &d.restart_policy {
        let mut o = Map::new();
        put(&mut o, "Condition", json!(r.condition));
        put(&mut o, "Delay", secs(r.delay_secs));
        put(&mut o, "MaxAttempts", json!(r.max_attempts));
        put(&mut o, "Window", secs(r.window_secs));
        tt.insert("RestartPolicy".into(), Value::Object(o));
    }
    let mut placement = Map::new();
    if !d.constraints.is_empty() {
        placement.insert("Constraints".into(), json!(d.constraints));
    }
    if !d.preferences.is_empty() {
        let prefs: Vec<Value> = d
            .preferences
            .iter()
            .map(|p| json!({"Spread": {"SpreadDescriptor": p}}))
            .collect();
        placement.insert("Preferences".into(), json!(prefs));
    }
    put(
        &mut placement,
        "MaxReplicas",
        json!(d.max_replicas_per_node),
    );
    if !placement.is_empty() {
        tt.insert("Placement".into(), Value::Object(placement));
    }
    let networks: Vec<Value> = svc
        .networks
        .iter()
        .map(|(k, n)| {
            let target = project
                .networks
                .get(k)
                .map(|net| net.name.clone())
                .unwrap_or_else(|| k.clone());
            // As `docker stack deploy`: a predefined network takes no
            // service-name alias.
            let own = (!predefined(&target)).then_some(&svc.name);
            let aliases: Vec<&String> = own.into_iter().chain(&n.aliases).collect();
            if aliases.is_empty() {
                json!({"Target": target})
            } else {
                json!({"Target": target, "Aliases": aliases})
            }
        })
        .collect();
    if !networks.is_empty() {
        tt.insert("Networks".into(), json!(networks));
    }
    if let Some(l) = &svc.logging {
        let mut o = Map::new();
        o.insert("Name".into(), json!(l.driver));
        if !l.options.is_empty() {
            o.insert("Options".into(), json!(l.options));
        }
        tt.insert("LogDriver".into(), Value::Object(o));
    }

    let mut spec = Map::new();
    spec.insert("Name".into(), json!(service_name(project, key)));
    let mut labels = d.labels.clone();
    labels.insert(STACK_LABEL.into(), stack.clone());
    labels.insert(LABEL_STACK_IMAGE.into(), svc.image.clone());
    spec.insert("Labels".into(), json!(labels));
    spec.insert("TaskTemplate".into(), Value::Object(tt));
    spec.insert(
        "Mode".into(),
        match d.mode {
            SwarmMode::Replicated => json!({"Replicated": {"Replicas": svc.replicas}}),
            SwarmMode::Global => json!({"Global": {}}),
            // Docker runs one at a time unless told otherwise; said
            // explicitly, so the stored spec reads back the same.
            SwarmMode::ReplicatedJob => json!({
                "ReplicatedJob": {"MaxConcurrent": 1, "TotalCompletions": svc.replicas}
            }),
            SwarmMode::GlobalJob => json!({"GlobalJob": {}}),
        },
    );
    put(&mut spec, "UpdateConfig", update_policy(&d.update_config));
    put(
        &mut spec,
        "RollbackConfig",
        update_policy(&d.rollback_config),
    );
    let mut endpoint = Map::new();
    put(&mut endpoint, "Mode", json!(d.endpoint_mode));
    let ports: Vec<Value> = svc
        .ports
        .iter()
        .map(|p| {
            let mut o = Map::new();
            o.insert("Protocol".into(), json!(p.protocol));
            o.insert("TargetPort".into(), json!(p.target));
            put(&mut o, "PublishedPort", json!(p.published));
            o.insert(
                "PublishMode".into(),
                json!(p.mode.as_deref().unwrap_or("ingress")),
            );
            Value::Object(o)
        })
        .collect();
    if !ports.is_empty() {
        endpoint.insert("Ports".into(), json!(ports));
    }
    if !endpoint.is_empty() {
        spec.insert("EndpointSpec".into(), Value::Object(endpoint));
    }
    let mut spec = Value::Object(spec);
    let hash = spec_hash(&spec);
    spec["Labels"][LABEL_SPEC_HASH] = json!(hash);
    spec
}

fn mount(project: &Project, m: &Mount) -> Value {
    match m {
        Mount::Volume {
            source,
            target,
            read_only,
        } => {
            let mut o = Map::new();
            o.insert("Type".into(), json!("volume"));
            if let Some(vol) = project.volumes.get(source) {
                o.insert("Source".into(), json!(vol.name));
                if !vol.external {
                    let mut opts = Map::new();
                    let mut labels = vol.labels.clone();
                    labels.insert(STACK_LABEL.into(), project.name.clone());
                    opts.insert("Labels".into(), json!(labels));
                    if let Some(driver) = &vol.driver {
                        let mut dc = Map::new();
                        dc.insert("Name".into(), json!(driver));
                        if !vol.driver_opts.is_empty() {
                            dc.insert("Options".into(), json!(vol.driver_opts));
                        }
                        opts.insert("DriverConfig".into(), Value::Object(dc));
                    }
                    o.insert("VolumeOptions".into(), Value::Object(opts));
                }
            }
            o.insert("Target".into(), json!(target));
            if *read_only {
                o.insert("ReadOnly".into(), json!(true));
            }
            Value::Object(o)
        }
        Mount::Bind {
            source,
            target,
            read_only,
        } => {
            let mut o = json!({"Type": "bind", "Source": source, "Target": target});
            if *read_only {
                o["ReadOnly"] = json!(true);
            }
            o
        }
        Mount::Tmpfs { target } => json!({"Type": "tmpfs", "Target": target}),
    }
}

/// `spec` without what Docker fills in or FleetOps adds itself: the
/// spec-hash label, secret and config ids, empty values, Docker's defaults,
/// a redeploy's ForceUpdate counter and the digest Docker may pin an image
/// to. Object keys are sorted.
pub fn canonical(spec: &Value) -> Value {
    clean(spec, "").unwrap_or_else(|| json!({}))
}

fn clean(v: &Value, path: &str) -> Option<Value> {
    match v {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) if path == "TaskTemplate.ContainerSpec.Image" => {
            Some(json!(s.split('@').next().unwrap_or(s)))
        }
        Value::Array(items) => {
            let item_path = format!("{path}[]");
            let out: Vec<Value> = items.iter().filter_map(|i| clean(i, &item_path)).collect();
            (!out.is_empty()).then_some(Value::Array(out))
        }
        Value::Object(m) => {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                if is_default(path, k, v) {
                    continue;
                }
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if let Some(c) = clean(v, &p) {
                    out.insert(k.clone(), c);
                }
            }
            // An empty Global still says the service is global.
            let keep_empty = matches!(path, "Mode.Global" | "Mode.GlobalJob");
            (!out.is_empty() || keep_empty).then(|| Value::Object(out.into_iter().collect()))
        }
        other => Some(other.clone()),
    }
}

/// A key `canonical` drops at `path`.
fn is_default(path: &str, key: &str, v: &Value) -> bool {
    match (path, key) {
        ("Labels", LABEL_SPEC_HASH) => true,
        (_, "SecretID" | "ConfigID") => true,
        // Bumped by a redeploy to restart tasks; a deploy keeps it.
        ("TaskTemplate", "ForceUpdate") => true,
        ("TaskTemplate", "Runtime") => v == "container",
        ("TaskTemplate.ContainerSpec", "Isolation") => v == "default",
        ("EndpointSpec", "Mode") => v == "vip",
        ("EndpointSpec.Ports[]", "Protocol") => v == "tcp",
        ("EndpointSpec.Ports[]", "PublishMode") => v == "ingress",
        // Docker stores a healthcheck's zero durations and retries as
        // absent.
        ("TaskTemplate.ContainerSpec.HealthCheck", _) => v == &json!(0),
        // What Docker fills into a partial restart policy or update config.
        ("TaskTemplate.RestartPolicy", "MaxAttempts") => v == &json!(0),
        ("TaskTemplate.RestartPolicy", "Condition") => v == "any",
        (
            "UpdateConfig" | "RollbackConfig",
            "Parallelism" | "MaxFailureRatio" | "Monitor" | "Delay",
        ) => v.as_f64() == Some(0.0),
        ("UpdateConfig" | "RollbackConfig", "FailureAction") => v == "pause",
        ("UpdateConfig" | "RollbackConfig", "Order") => v == "stop-first",
        _ => false,
    }
}

/// Docker's own networks, on every node and in no stack: `docker stack
/// deploy` neither looks them up nor aliases services on them.
pub fn predefined(network: &str) -> bool {
    matches!(network, "host" | "bridge" | "none")
}

/// A live spec with its networks named, as rendered specs name them:
/// Docker stores the id of the network a spec names.
pub fn named(live: &Value, networks: &BTreeMap<String, String>) -> Value {
    let mut out = live.clone();
    if let Some(list) = out["TaskTemplate"]["Networks"].as_array_mut() {
        for n in list {
            if let Some(name) = n["Target"].as_str().and_then(|id| networks.get(id)) {
                n["Target"] = json!(name);
            }
        }
    }
    out
}

/// Hex SHA-256 of `canonical(spec)`.
pub fn spec_hash(spec: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical(spec)).expect("serialisable");
    format!("{:x}", Sha256::digest(bytes))
}

const ENV: &str = "TaskTemplate.ContainerSpec.Env";
const IMAGE: &str = "TaskTemplate.ContainerSpec.Image";
const LABELS: &str = "TaskTemplate.ContainerSpec.Labels";
const REPLICAS: &str = "Mode.Replicated.Replicas";

fn at<'v>(v: &'v Value, path: &str) -> &'v Value {
    path.split('.').fold(v, |v, k| &v[k])
}

/// What differs between two canonical specs, never showing a value but an
/// image's or a replica count: env and label keys by name, then other
/// fields by path.
pub fn spec_changes(old: &Value, new: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let env = |v: &Value| -> BTreeMap<String, String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|e| match e.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (e.to_string(), String::new()),
            })
            .collect()
    };
    keyed(&env(at(old, ENV)), &env(at(new, ENV)), "env", &mut out);
    let (a, b) = (at(old, IMAGE), at(new, IMAGE));
    if a != b {
        out.push(format!(
            "image: {} → {}",
            a.as_str().unwrap_or_default(),
            b.as_str().unwrap_or_default()
        ));
    }
    let labels = |v: &Value, skip: &[&str]| -> BTreeMap<String, String> {
        v.as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect()
    };
    keyed(
        &labels(at(old, LABELS), &[]),
        &labels(at(new, LABELS), &[]),
        "label",
        &mut out,
    );
    let skip = [LABEL_STACK_IMAGE, LABEL_SPEC_HASH];
    keyed(
        &labels(&old["Labels"], &skip),
        &labels(&new["Labels"], &skip),
        "deploy label",
        &mut out,
    );
    if let (Some(a), Some(b)) = (at(old, REPLICAS).as_u64(), at(new, REPLICAS).as_u64()) {
        if a != b {
            out.push(format!("replicas: {a} → {b}"));
        }
    }
    let mut paths = BTreeSet::new();
    walk(old, new, "", &mut paths);
    out.extend(paths.into_iter().map(|p| format!("{p} changed")));
    out
}

fn keyed(
    a: &BTreeMap<String, String>,
    b: &BTreeMap<String, String>,
    what: &str,
    out: &mut Vec<String>,
) {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for k in keys {
        match (a.get(k), b.get(k)) {
            (None, Some(_)) => out.push(format!("{what} {k} added")),
            (Some(_), None) => out.push(format!("{what} {k} removed")),
            (Some(x), Some(y)) if x != y => out.push(format!("{what} {k} changed")),
            _ => {}
        }
    }
}

fn walk(a: &Value, b: &Value, path: &str, out: &mut BTreeSet<String>) {
    if a == b || matches!(path, ENV | IMAGE | LABELS | REPLICAS | "Labels") {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                    &p,
                    out,
                );
            }
        }
        _ => {
            out.insert(path.to_string());
        }
    }
}
