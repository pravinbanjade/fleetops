//! Pure conversions between dockerer's domain types and Docker API JSON.
//! Kept free of I/O so they can be unit-tested without a daemon.

use dockerer_core::docker::{
    ContainerSpec, ContainerSummary, NetworkSummary, PortBinding, PortMapping, VolumeSummary,
    COMPOSE_PROJECT_LABEL, COMPOSE_SERVICE_LABEL,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

/// Builds a Docker `ContainerCreate` body. `spec.advanced` is deep-merged
/// last, so power users can set any field the form does not cover.
pub fn spec_to_body(spec: &ContainerSpec) -> Value {
    let mut exposed = Map::new();
    let mut bindings = Map::new();
    for p in &spec.ports {
        let key = format!("{}/{}", p.container_port, p.protocol);
        exposed.insert(key.clone(), json!({}));
        let entry = bindings.entry(key).or_insert_with(|| json!([]));
        if let Value::Array(list) = entry {
            list.push(json!({
                "HostIp": p.host_ip.clone().unwrap_or_default(),
                "HostPort": p.host_port.map(|n| n.to_string()).unwrap_or_default(),
            }));
        }
    }
    let mut host = json!({
        "Binds": spec.binds,
        "PortBindings": bindings,
        "Privileged": spec.privileged,
    });
    if let Some(policy) = &spec.restart_policy {
        host["RestartPolicy"] = json!({ "Name": policy });
    }
    if let Some(m) = spec.memory_bytes {
        host["Memory"] = json!(m);
    }
    if let Some(n) = spec.nano_cpus {
        host["NanoCpus"] = json!(n);
    }
    if !spec.cap_add.is_empty() {
        host["CapAdd"] = json!(spec.cap_add);
    }
    if let Some(net) = &spec.network {
        host["NetworkMode"] = json!(net);
    }
    let mut body = json!({
        "Image": spec.image,
        "Env": spec.env,
        "Labels": spec.labels,
        "ExposedPorts": exposed,
        "HostConfig": host,
    });
    if let Some(cmd) = &spec.cmd {
        body["Cmd"] = json!(cmd);
    }
    if let Some(ep) = &spec.entrypoint {
        body["Entrypoint"] = json!(ep);
    }
    if let Some(adv) = &spec.advanced {
        merge(&mut body, adv);
    }
    body
}

/// Deep-merges `over` into `base`: objects merge key by key, anything else
/// replaces.
pub fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn opt_strings(v: &Value) -> Option<Vec<String>> {
    v.is_array().then(|| strings(v))
}

fn positive(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n > 0)
}

/// `Config` fields the form does not model but a recreate must keep.
const CARRIED_CONFIG: &[&str] = &[
    "User",
    "WorkingDir",
    "Domainname",
    "Tty",
    "OpenStdin",
    "StdinOnce",
    "AttachStdin",
    "StopSignal",
    "StopTimeout",
    "Healthcheck",
    "Shell",
];

/// `HostConfig` fields the form does not model but a recreate must keep.
/// `Mounts` is handled separately so anonymous volumes can be reattached.
const CARRIED_HOST: &[&str] = &[
    "ExtraHosts",
    "Devices",
    "DeviceRequests",
    "LogConfig",
    "Dns",
    "DnsSearch",
    "DnsOptions",
    "CapDrop",
    "SecurityOpt",
    "Sysctls",
    "Ulimits",
    "Tmpfs",
    "ShmSize",
    "PidMode",
    "IpcMode",
    "UTSMode",
    "UsernsMode",
    "CgroupnsMode",
    "Init",
    "GroupAdd",
    "VolumesFrom",
    "Links",
    "ReadonlyRootfs",
    "OomKillDisable",
    "MemoryReservation",
    "MemorySwap",
    "CpuShares",
    "CpusetCpus",
    "CpuQuota",
    "CpuPeriod",
    "PidsLimit",
    "Runtime",
    "AutoRemove",
];

/// Docker's zero values, which need not be carried.
fn is_unset(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

fn copy_set(from: &Value, keys: &[&str], to: &mut Map<String, Value>) {
    for k in keys {
        if !is_unset(&from[*k]) {
            to.insert((*k).to_string(), from[*k].clone());
        }
    }
}

/// Everything in `inspect` that the create form does not model, as a
/// `ContainerCreate` fragment for `ContainerSpec::advanced`. Without this a
/// recreate would silently drop `--mount` volumes, anonymous volumes,
/// network aliases and the like.
fn unmodelled(v: &Value) -> Option<Value> {
    let config = &v["Config"];
    let host = &v["HostConfig"];
    let mut adv = Map::new();
    copy_set(config, CARRIED_CONFIG, &mut adv);
    // Docker defaults the hostname to the short id; only carry a chosen one.
    if let (Some(h), Some(id)) = (config["Hostname"].as_str(), v["Id"].as_str()) {
        if !h.is_empty() && !id.starts_with(h) {
            adv.insert("Hostname".into(), json!(h));
        }
    }

    let mut hc = Map::new();
    copy_set(host, CARRIED_HOST, &mut hc);
    let mut mounts: Vec<Value> = host["Mounts"].as_array().cloned().unwrap_or_default();
    let mut covered: HashSet<String> = strings(&host["Binds"])
        .iter()
        .filter_map(|b| b.split(':').nth(1).map(str::to_owned))
        .collect();
    covered.extend(
        mounts
            .iter()
            .filter_map(|m| m["Target"].as_str().map(str::to_owned)),
    );
    // Anonymous volumes (image VOLUME or `-v /path`) appear only in the
    // top-level Mounts; reattach them by name so their data survives.
    for m in v["Mounts"].as_array().into_iter().flatten() {
        let (Some("volume"), Some(name), Some(dest)) = (
            m["Type"].as_str(),
            m["Name"].as_str(),
            m["Destination"].as_str(),
        ) else {
            continue;
        };
        if covered.insert(dest.to_string()) {
            mounts.push(json!({"Type": "volume", "Source": name, "Target": dest}));
        }
    }
    if !mounts.is_empty() {
        hc.insert("Mounts".into(), Value::Array(mounts));
    }
    if !hc.is_empty() {
        adv.insert("HostConfig".into(), Value::Object(hc));
    }

    let mut endpoints = Map::new();
    for (net, ep) in v["NetworkSettings"]["Networks"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let mut cfg = Map::new();
        copy_set(
            ep,
            &["Aliases", "IPAMConfig", "Links", "DriverOpts"],
            &mut cfg,
        );
        if !cfg.is_empty() {
            endpoints.insert(net.clone(), Value::Object(cfg));
        }
    }
    if !endpoints.is_empty() {
        adv.insert(
            "NetworkingConfig".into(),
            json!({ "EndpointsConfig": endpoints }),
        );
    }
    (!adv.is_empty()).then_some(Value::Object(adv))
}

/// The create-form view of a container, from its inspect JSON.
pub fn inspect_to_spec(v: &Value) -> ContainerSpec {
    let config = &v["Config"];
    let host = &v["HostConfig"];
    let mut ports = Vec::new();
    if let Some(map) = host["PortBindings"].as_object() {
        for (key, list) in map {
            let (port, proto) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
            let Ok(container_port) = port.parse::<u16>() else {
                continue;
            };
            for b in list.as_array().into_iter().flatten() {
                ports.push(PortBinding {
                    container_port,
                    protocol: proto.to_string(),
                    host_ip: b["HostIp"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    host_port: b["HostPort"].as_str().and_then(|s| s.parse().ok()),
                });
            }
        }
    }
    ports.sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
    let labels = config["Labels"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    ContainerSpec {
        name: v["Name"]
            .as_str()
            .map(|n| n.trim_start_matches('/').to_string()),
        image: config["Image"].as_str().unwrap_or_default().to_string(),
        cmd: opt_strings(&config["Cmd"]),
        entrypoint: opt_strings(&config["Entrypoint"]),
        env: strings(&config["Env"]),
        ports,
        binds: strings(&host["Binds"]),
        network: host["NetworkMode"]
            .as_str()
            .filter(|n| *n != "default" && !n.is_empty())
            .map(str::to_owned),
        restart_policy: host["RestartPolicy"]["Name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
        labels,
        memory_bytes: positive(&host["Memory"]),
        nano_cpus: positive(&host["NanoCpus"]),
        cap_add: strings(&host["CapAdd"]),
        privileged: host["Privileged"].as_bool().unwrap_or(false),
        advanced: unmodelled(v),
    }
}

pub fn container_summary(c: bollard::models::ContainerSummary) -> ContainerSummary {
    let labels: BTreeMap<String, String> = c.labels.unwrap_or_default().into_iter().collect();
    ContainerSummary {
        id: c.id.unwrap_or_default(),
        name: c
            .names
            .and_then(|n| n.into_iter().next())
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default(),
        image: c.image.unwrap_or_default(),
        image_id: c.image_id.unwrap_or_default(),
        state: c.state.map(|s| s.to_string()).unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        created: c.created.unwrap_or_default(),
        ports: c
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(|p| PortMapping {
                private_port: p.private_port,
                public_port: p.public_port,
                ip: p.ip,
                protocol: p
                    .typ
                    .map(|t| t.to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "tcp".into()),
            })
            .collect(),
        compose_project: labels.get(COMPOSE_PROJECT_LABEL).cloned(),
        compose_service: labels.get(COMPOSE_SERVICE_LABEL).cloned(),
        labels,
        volumes: c
            .mounts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.name)
            .collect(),
    }
}

pub fn volume_summary(v: bollard::models::Volume, in_use: &HashSet<String>) -> VolumeSummary {
    VolumeSummary {
        in_use: in_use.contains(&v.name),
        name: v.name,
        driver: v.driver,
        mountpoint: v.mountpoint,
        created_at: v.created_at,
        labels: v.labels.into_iter().collect(),
    }
}

pub fn network_summary(n: bollard::models::Network) -> NetworkSummary {
    let ipam = n
        .ipam
        .and_then(|i| i.config)
        .and_then(|c| c.into_iter().next());
    NetworkSummary {
        id: n.id.unwrap_or_default(),
        name: n.name.unwrap_or_default(),
        driver: n.driver.unwrap_or_default(),
        scope: n.scope.unwrap_or_default(),
        internal: n.internal.unwrap_or(false),
        attachable: n.attachable.unwrap_or(false),
        subnet: ipam.as_ref().and_then(|c| c.subnet.clone()),
        gateway: ipam.and_then(|c| c.gateway),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn full_spec() -> ContainerSpec {
        ContainerSpec {
            name: Some("web".into()),
            image: "nginx:1.27".into(),
            cmd: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
            entrypoint: None,
            env: vec!["A=1".into()],
            ports: vec![
                PortBinding {
                    container_port: 80,
                    protocol: "tcp".into(),
                    host_ip: None,
                    host_port: Some(8080),
                },
                PortBinding {
                    container_port: 53,
                    protocol: "udp".into(),
                    host_ip: Some("127.0.0.1".into()),
                    host_port: None,
                },
            ],
            binds: vec!["data:/data".into()],
            network: Some("backend".into()),
            restart_policy: Some("unless-stopped".into()),
            labels: BTreeMap::from([("team".into(), "a".into())]),
            memory_bytes: Some(1 << 28),
            nano_cpus: Some(500_000_000),
            cap_add: vec!["NET_ADMIN".into()],
            privileged: false,
            advanced: None,
        }
    }

    #[test]
    fn spec_to_body_maps_every_field() {
        let b = spec_to_body(&full_spec());
        assert_eq!(b["Image"], "nginx:1.27");
        assert_eq!(b["Cmd"], json!(["nginx", "-g", "daemon off;"]));
        assert!(b.get("Entrypoint").is_none());
        assert_eq!(b["Env"], json!(["A=1"]));
        assert_eq!(b["Labels"], json!({"team": "a"}));
        assert_eq!(b["ExposedPorts"], json!({"80/tcp": {}, "53/udp": {}}));
        let h = &b["HostConfig"];
        assert_eq!(
            h["PortBindings"]["80/tcp"],
            json!([{"HostIp": "", "HostPort": "8080"}])
        );
        assert_eq!(
            h["PortBindings"]["53/udp"],
            json!([{"HostIp": "127.0.0.1", "HostPort": ""}])
        );
        assert_eq!(h["Binds"], json!(["data:/data"]));
        assert_eq!(h["NetworkMode"], "backend");
        assert_eq!(h["RestartPolicy"], json!({"Name": "unless-stopped"}));
        assert_eq!(h["Memory"], json!(1 << 28));
        assert_eq!(h["NanoCpus"], json!(500_000_000));
        assert_eq!(h["CapAdd"], json!(["NET_ADMIN"]));
        assert_eq!(h["Privileged"], false);
    }

    #[test]
    fn advanced_is_deep_merged_last() {
        let spec = ContainerSpec {
            advanced: Some(
                json!({"HostConfig": {"Privileged": true, "ShmSize": 64}, "WorkingDir": "/app"}),
            ),
            ..full_spec()
        };
        let b = spec_to_body(&spec);
        assert_eq!(b["HostConfig"]["Privileged"], true);
        assert_eq!(b["HostConfig"]["ShmSize"], 64);
        assert_eq!(
            b["HostConfig"]["NetworkMode"], "backend",
            "untouched keys survive"
        );
        assert_eq!(b["WorkingDir"], "/app");
    }

    #[test]
    fn body_deserializes_into_bollard_create_body() {
        let body: bollard::models::ContainerCreateBody =
            serde_json::from_value(spec_to_body(&full_spec())).unwrap();
        assert_eq!(body.image.as_deref(), Some("nginx:1.27"));
        let host = body.host_config.unwrap();
        assert_eq!(host.binds, Some(vec!["data:/data".to_string()]));
        assert!(body.exposed_ports.unwrap().contains(&"80/tcp".to_string()));
    }

    #[test]
    fn inspect_roundtrips_through_spec() {
        let spec = full_spec();
        let body = spec_to_body(&spec);
        // Shape an inspect response the way Docker does: name with a slash,
        // Config holding container fields, HostConfig alongside.
        let inspect = json!({
            "Name": "/web",
            "Config": {"Image": body["Image"], "Cmd": body["Cmd"], "Entrypoint": null, "Env": body["Env"], "Labels": body["Labels"]},
            "HostConfig": body["HostConfig"],
        });
        let mut back = inspect_to_spec(&inspect);
        let mut expected = spec;
        expected
            .ports
            .sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
        back.ports
            .sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
        assert_eq!(back, expected);
    }

    #[test]
    fn inspect_defaults_are_normalised() {
        let spec = inspect_to_spec(&json!({
            "Name": "/x",
            "Config": {"Image": "alpine", "Cmd": null},
            "HostConfig": {"NetworkMode": "default", "RestartPolicy": {"Name": ""}, "Memory": 0, "PortBindings": {}}
        }));
        assert_eq!(spec.network, None);
        assert_eq!(spec.restart_policy, None);
        assert_eq!(spec.memory_bytes, None);
        assert_eq!(spec.cmd, None);
        assert!(spec.ports.is_empty());
    }

    #[test]
    fn unmodelled_settings_are_carried_in_advanced() {
        // A compose-style container: --mount volume, anonymous image volume,
        // a user network with a service alias, and settings the form lacks.
        let inspect = json!({
            "Name": "/db",
            "Config": {"Image": "postgres:16", "User": "999", "WorkingDir": "/srv", "Tty": true, "OpenStdin": false, "Hostname": "abc123"},
            "HostConfig": {
                "NetworkMode": "app_default",
                "Binds": ["/host/conf:/etc/pg:ro"],
                "Mounts": [{"Type": "volume", "Source": "pgdata", "Target": "/var/lib/postgresql/data"}],
                "ExtraHosts": ["gw:10.0.0.1"],
                "LogConfig": {"Type": "json-file", "Config": {"max-size": "10m"}},
                "Devices": [],
                "Init": null,
                "ShmSize": 67108864
            },
            "Mounts": [
                {"Type": "bind", "Source": "/host/conf", "Destination": "/etc/pg"},
                {"Type": "volume", "Name": "pgdata", "Destination": "/var/lib/postgresql/data"},
                {"Type": "volume", "Name": "0f3e9a", "Destination": "/anon"}
            ],
            "NetworkSettings": {"Networks": {"app_default": {
                "Aliases": ["db", "postgres"], "IPAMConfig": null,
                "NetworkID": "n1", "EndpointID": "e1", "IPAddress": "172.18.0.2", "MacAddress": "02:42"
            }}}
        });
        let spec = inspect_to_spec(&inspect);
        let adv = spec
            .advanced
            .clone()
            .expect("advanced carries unmodelled settings");
        assert_eq!(adv["User"], "999");
        assert_eq!(adv["WorkingDir"], "/srv");
        assert_eq!(adv["Tty"], true);
        assert!(adv.get("OpenStdin").is_none(), "defaults are not carried");
        assert!(
            adv.get("Hostname").is_none(),
            "generated hostname is not carried"
        );
        let host = &adv["HostConfig"];
        assert_eq!(host["ExtraHosts"], json!(["gw:10.0.0.1"]));
        assert_eq!(host["LogConfig"]["Config"]["max-size"], "10m");
        assert_eq!(host["ShmSize"], 67108864);
        assert!(host.get("Devices").is_none() && host.get("Init").is_none());
        assert_eq!(
            host["Mounts"],
            json!([
                {"Type": "volume", "Source": "pgdata", "Target": "/var/lib/postgresql/data"},
                {"Type": "volume", "Source": "0f3e9a", "Target": "/anon"}
            ]),
            "--mount volumes are kept and anonymous volumes are reattached"
        );
        assert_eq!(
            adv["NetworkingConfig"],
            json!({"EndpointsConfig": {"app_default": {"Aliases": ["db", "postgres"]}}})
        );
        // And it reaches the create body.
        let body = spec_to_body(&spec);
        assert_eq!(
            body["HostConfig"]["Binds"],
            json!(["/host/conf:/etc/pg:ro"])
        );
        assert_eq!(body["HostConfig"]["Mounts"][1]["Source"], "0f3e9a");
        assert_eq!(
            body["NetworkingConfig"]["EndpointsConfig"]["app_default"]["Aliases"][0],
            "db"
        );
        serde_json::from_value::<bollard::models::ContainerCreateBody>(body).unwrap();
    }
}
