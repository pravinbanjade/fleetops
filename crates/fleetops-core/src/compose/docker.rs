//! From a normalised project to Docker objects: names, labels, config
//! hashes, start order and API create bodies (stacks spec §2).

use super::model::*;
use super::parse::deps;
use crate::docker::{NetworkSpec, VolumeSpec, COMPOSE_PROJECT_LABEL, COMPOSE_SERVICE_LABEL};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const LABEL_PROJECT: &str = COMPOSE_PROJECT_LABEL;
pub const LABEL_SERVICE: &str = COMPOSE_SERVICE_LABEL;
pub const LABEL_NUMBER: &str = "com.docker.compose.container-number";
pub const LABEL_ONEOFF: &str = "com.docker.compose.oneoff";
pub const LABEL_CONFIG_FILES: &str = "com.docker.compose.project.config_files";
pub const LABEL_NETWORK: &str = "com.docker.compose.network";
pub const LABEL_VOLUME: &str = "com.docker.compose.volume";
pub const LABEL_HASH: &str = "io.fleetops.stack.config-hash";
pub const LABEL_REVISION: &str = "io.fleetops.stack.revision";

const NS: i64 = 1_000_000_000;

pub fn container_name(project: &Project, svc: &Service, n: u32) -> String {
    match &svc.container_name {
        Some(name) => name.clone(),
        None => format!("{}-{}-{n}", project.name, svc.name),
    }
}

fn sha256_hex(v: &impl serde::Serialize) -> String {
    let bytes = serde_json::to_vec(v).expect("model types serialise");
    format!("{:x}", Sha256::digest(bytes))
}

/// The service's config hash. The replica count is left out, so scaling
/// does not recreate.
pub fn service_hash(svc: &Service) -> String {
    let mut s = svc.clone();
    s.replicas = 0;
    sha256_hex(&s)
}

pub fn network_hash(net: &Network) -> String {
    sha256_hex(net)
}

/// Services in start order: every dependency first, ties by name.
pub fn start_order(project: &Project) -> Vec<String> {
    let mut remaining: BTreeMap<&str, BTreeSet<String>> = project
        .services
        .iter()
        .map(|(n, s)| {
            let d = deps(s)
                .into_iter()
                .filter(|d| project.services.contains_key(d))
                .collect();
            (n.as_str(), d)
        })
        .collect();
    let mut out = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining
            .iter()
            .filter(|(_, d)| d.is_empty())
            .map(|(n, _)| *n)
            .collect();
        // A cycle is rejected by `parse`; break one defensively by name.
        let next = ready
            .first()
            .copied()
            .unwrap_or_else(|| *remaining.keys().next().unwrap());
        remaining.remove(next);
        for d in remaining.values_mut() {
            d.remove(next);
        }
        out.push(next.to_string());
    }
    out
}

fn str_map(m: &BTreeMap<String, String>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// A Docker `ContainerCreate` body for replica `n` of `svc`.
pub fn container_body(project: &Project, svc: &Service, n: u32, revision: i64) -> Value {
    let mut labels = svc.labels.clone();
    labels.insert(LABEL_PROJECT.into(), project.name.clone());
    labels.insert(LABEL_SERVICE.into(), svc.name.clone());
    labels.insert(LABEL_NUMBER.into(), n.to_string());
    labels.insert(LABEL_ONEOFF.into(), "False".into());
    labels.insert(LABEL_CONFIG_FILES.into(), "fleetops".into());
    labels.insert(LABEL_HASH.into(), service_hash(svc));
    labels.insert(LABEL_REVISION.into(), revision.to_string());

    let mut exposed = Map::new();
    let mut bindings: Map<String, Value> = Map::new();
    for p in &svc.ports {
        let key = format!("{}/{}", p.target, p.protocol);
        exposed.insert(key.clone(), json!({}));
        let entry = bindings.entry(key).or_insert_with(|| json!([]));
        entry.as_array_mut().unwrap().push(json!({
            "HostIp": p.host_ip.clone().unwrap_or_default(),
            "HostPort": p.published.map(|n| n.to_string()).unwrap_or_default(),
        }));
    }

    let mounts: Vec<Value> = svc
        .mounts
        .iter()
        .map(|m| match m {
            Mount::Volume {
                source,
                target,
                read_only,
            } => {
                let src = project
                    .volumes
                    .get(source)
                    .map(|v| v.name.clone())
                    .unwrap_or_default();
                json!({"Type": "volume", "Source": src, "Target": target, "ReadOnly": read_only})
            }
            Mount::Bind {
                source,
                target,
                read_only,
            } => json!({"Type": "bind", "Source": source, "Target": target, "ReadOnly": read_only}),
            Mount::Tmpfs { target } => json!({"Type": "tmpfs", "Target": target}),
        })
        .collect();

    let restart = svc.restart.as_deref().map(|r| match r.split_once(':') {
        Some((name, n)) => {
            json!({"Name": name, "MaximumRetryCount": n.parse::<i64>().unwrap_or(0)})
        }
        None => json!({"Name": r}),
    });

    let tmpfs: Map<String, Value> = svc
        .tmpfs
        .iter()
        .map(|t| match t.split_once(':') {
            Some((path, opts)) => (path.to_string(), json!(opts)),
            None => (t.clone(), json!("")),
        })
        .collect();

    let devices: Vec<Value> = svc
        .devices
        .iter()
        .map(|d| {
            let parts: Vec<&str> = d.split(':').collect();
            let host = parts[0];
            let inside = parts.get(1).copied().unwrap_or(host);
            let perms = parts.get(2).copied().unwrap_or("rwm");
            json!({"PathOnHost": host, "PathInContainer": inside, "CgroupPermissions": perms})
        })
        .collect();

    let mut endpoints = Map::new();
    for (key, sn) in &svc.networks {
        let docker_name = project
            .networks
            .get(key)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| key.clone());
        let mut aliases = vec![svc.name.clone()];
        aliases.extend(sn.aliases.iter().cloned());
        let mut ep = json!({"Aliases": aliases});
        if let Some(ip) = &sn.ipv4_address {
            ep["IPAMConfig"] = json!({"IPv4Address": ip});
        }
        endpoints.insert(docker_name, ep);
    }
    let network_mode = match svc.network_mode.as_deref() {
        Some(m) => match m.strip_prefix("service:") {
            Some(target) => {
                let name = project
                    .services
                    .get(target)
                    .map(|t| container_name(project, t, 1))
                    .unwrap_or_else(|| target.to_string());
                format!("container:{name}")
            }
            None => m.to_string(),
        },
        None => endpoints
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "bridge".into()),
    };

    let mut host = json!({
        "PortBindings": bindings,
        "Mounts": mounts,
        "NetworkMode": network_mode,
        "Privileged": svc.privileged,
        "ReadonlyRootfs": svc.read_only,
    });
    let h = host.as_object_mut().unwrap();
    if let Some(r) = restart {
        h.insert("RestartPolicy".into(), r);
    }
    if !svc.extra_hosts.is_empty() {
        h.insert("ExtraHosts".into(), json!(svc.extra_hosts));
    }
    if !svc.dns.is_empty() {
        h.insert("Dns".into(), json!(svc.dns));
    }
    if !svc.cap_add.is_empty() {
        h.insert("CapAdd".into(), json!(svc.cap_add));
    }
    if !svc.cap_drop.is_empty() {
        h.insert("CapDrop".into(), json!(svc.cap_drop));
    }
    if let Some(i) = svc.init {
        h.insert("Init".into(), json!(i));
    }
    if !tmpfs.is_empty() {
        h.insert("Tmpfs".into(), Value::Object(tmpfs));
    }
    if !devices.is_empty() {
        h.insert("Devices".into(), json!(devices));
    }
    if let Some(s) = svc.shm_size {
        h.insert("ShmSize".into(), json!(s));
    }
    if let Some(l) = &svc.logging {
        h.insert(
            "LogConfig".into(),
            json!({"Type": l.driver, "Config": str_map(&l.options)}),
        );
    }
    if let Some(m) = svc.memory_bytes {
        h.insert("Memory".into(), json!(m));
    }
    if let Some(c) = svc.nano_cpus {
        h.insert("NanoCpus".into(), json!(c));
    }

    let env: Vec<String> = svc
        .environment
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let mut body = json!({
        "Image": svc.image,
        "Env": env,
        "Labels": labels,
        "ExposedPorts": exposed,
        "HostConfig": host,
    });
    let b = body.as_object_mut().unwrap();
    if svc.network_mode.is_none() {
        b.insert(
            "NetworkingConfig".into(),
            json!({"EndpointsConfig": endpoints}),
        );
    }
    if let Some(c) = &svc.command {
        b.insert("Cmd".into(), json!(c));
    }
    if let Some(e) = &svc.entrypoint {
        b.insert("Entrypoint".into(), json!(e));
    }
    if let Some(hc) = &svc.healthcheck {
        let v = if hc.disable {
            json!({"Test": ["NONE"]})
        } else {
            json!({
                "Test": hc.test,
                "Interval": hc.interval_secs * NS,
                "Timeout": hc.timeout_secs * NS,
                "Retries": hc.retries,
                "StartPeriod": hc.start_period_secs * NS,
            })
        };
        b.insert("Healthcheck".into(), v);
    }
    for (key, val) in [
        ("User", &svc.user),
        ("WorkingDir", &svc.working_dir),
        ("Hostname", &svc.hostname),
        ("StopSignal", &svc.stop_signal),
    ] {
        if let Some(v) = val {
            b.insert(key.into(), json!(v));
        }
    }
    if let Some(t) = svc.stop_grace_period_secs {
        b.insert("StopTimeout".into(), json!(t));
    }
    body
}

pub fn network_create(project: &Project, key: &str, net: &Network, revision: i64) -> NetworkSpec {
    let mut labels = net.labels.clone();
    labels.insert(LABEL_PROJECT.into(), project.name.clone());
    labels.insert(LABEL_NETWORK.into(), key.into());
    labels.insert(LABEL_HASH.into(), network_hash(net));
    labels.insert(LABEL_REVISION.into(), revision.to_string());
    NetworkSpec {
        name: net.name.clone(),
        driver: net.driver.clone(),
        subnet: None,
        gateway: None,
        internal: net.internal,
        attachable: net.attachable,
        labels,
        driver_opts: net.driver_opts.clone(),
    }
}

pub fn volume_create(project: &Project, key: &str, vol: &Volume, revision: i64) -> VolumeSpec {
    let mut labels = vol.labels.clone();
    labels.insert(LABEL_PROJECT.into(), project.name.clone());
    labels.insert(LABEL_VOLUME.into(), key.into());
    labels.insert(LABEL_REVISION.into(), revision.to_string());
    VolumeSpec {
        name: vol.name.clone(),
        driver: vol.driver.clone(),
        driver_opts: vol.driver_opts.clone(),
        labels,
    }
}
