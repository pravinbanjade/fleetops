//! A Swarm node's daemon behind a test agent's tunnel streams, and a swarm
//! PKI to prove nodes with.

use fleetops_tunnel::StreamHandle;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A swarm root CA that issues node certificates.
pub struct SwarmPki {
    pub cluster_id: String,
    pub root: String,
    ca: Issuer<'static, KeyPair>,
}

/// A node certificate chain and its key, as on the node's disk.
#[derive(Clone)]
pub struct NodeCert {
    pub chain: String,
    pub key: String,
}

impl SwarmPki {
    pub fn new(cluster_id: &str) -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name.push(DnType::CommonName, "swarm-ca");
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let root = p.self_signed(&key).unwrap().pem();
        SwarmPki {
            cluster_id: cluster_id.into(),
            root,
            ca: Issuer::new(p, key),
        }
    }

    /// CN = node, O = cluster, OU = role.
    pub fn node(&self, node: &str, manager: bool) -> NodeCert {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name.push(DnType::CommonName, node);
        p.distinguished_name
            .push(DnType::OrganizationName, self.cluster_id.as_str());
        let ou = if manager {
            "swarm-manager"
        } else {
            "swarm-worker"
        };
        p.distinguished_name
            .push(DnType::OrganizationalUnitName, ou);
        NodeCert {
            chain: p.signed_by(&key, &self.ca).unwrap().pem(),
            key: key.serialize_pem(),
        }
    }
}

pub fn services_json() -> serde_json::Value {
    json!([
        {
            "ID": "s1", "Version": {"Index": 10},
            "CreatedAt": "2026-10-20T10:00:00Z", "UpdatedAt": "2026-10-21T10:00:00Z",
            "Spec": {
                "Name": "blog_web",
                "Labels": {"com.docker.stack.namespace": "blog"},
                "TaskTemplate": {
                    "ContainerSpec": {
                        "Image": "nginx:1.27@sha256:abc",
                        "Env": ["DB_PASSWORD=hunter2", "MODE=prod"],
                        "Secrets": [{"SecretID": "sec1", "SecretName": "blog_db"}],
                        "Configs": [{"ConfigID": "cfg1", "ConfigName": "blog_nginx"}]
                    },
                    "Networks": [{"Target": "net1"}]
                },
                "Mode": {"Replicated": {"Replicas": 3}},
                "EndpointSpec": {"Ports": [{"Protocol": "tcp", "TargetPort": 80, "PublishedPort": 8080}]}
            },
            "Endpoint": {"Ports": [{"Protocol": "tcp", "TargetPort": 80, "PublishedPort": 8080}]},
            "ServiceStatus": {"RunningTasks": 2, "DesiredTasks": 3},
            "UpdateStatus": {"State": "updating", "Message": "update in progress"}
        },
        {
            "ID": "s2", "Version": {"Index": 4},
            "Spec": {
                "Name": "agent",
                "TaskTemplate": {"ContainerSpec": {"Image": "fleets/fleetops-agent:0.7.0"}},
                "Mode": {"Global": {}}
            },
            "ServiceStatus": {"RunningTasks": 3, "DesiredTasks": 3}
        }
    ])
}

pub fn tasks_json() -> serde_json::Value {
    json!([
        {
            "ID": "t1", "Version": {"Index": 20}, "ServiceID": "s1", "Slot": 1, "NodeID": "n1",
            "Spec": {"ContainerSpec": {"Labels": {"com.docker.stack.namespace": "blog"}}},
            "UpdatedAt": "2026-10-21T10:00:00Z", "DesiredState": "running",
            "Status": {"State": "running", "Message": "started", "ContainerStatus": {"ContainerID": "c1"}}
        },
        {
            "ID": "t2", "Version": {"Index": 21}, "ServiceID": "s1", "Slot": 2, "NodeID": "n3",
            "Spec": {"ContainerSpec": {"Labels": {"com.docker.stack.namespace": "blog"}}},
            "UpdatedAt": "2026-10-21T10:01:00Z", "DesiredState": "shutdown",
            "Status": {"State": "rejected", "Message": "preparing", "Err": "No such image: nginx:9"}
        }
    ])
}

/// What a manager's daemon reports. Tests edit `nodes` to add or remove
/// members.
pub struct FakeSwarm {
    pub cluster_id: String,
    pub trust_root: String,
    pub nodes: Mutex<Vec<Value>>,
    /// Every request path, oldest first.
    pub requests: Mutex<Vec<String>>,
    /// While set, listing tasks fails.
    pub fail_tasks: std::sync::atomic::AtomicBool,
    /// What listing tasks returns; starts as `tasks_json()`.
    pub tasks: Mutex<Value>,
    /// What listing services returns; starts as `services_json()`.
    pub services: Mutex<Value>,
    /// Every non-GET call: method, path, JSON body (null when empty).
    pub writes: Mutex<Vec<(String, String, Value)>>,
    /// What listing secrets, configs and swarm networks returns; creates
    /// add to them and deletes remove from them.
    pub secrets: Mutex<Value>,
    pub configs: Mutex<Value>,
    pub networks: Mutex<Value>,
    /// The `UpdateStatus` (state, message) a service update leaves, instead
    /// of `completed`.
    pub update_outcome: Mutex<Option<(String, String)>>,
    /// A task added for each service created, its `ServiceID` filled in.
    pub on_create_task: Mutex<Option<Value>>,
    /// Objects created so far, for their ids.
    created: Mutex<u32>,
    /// What a node's engine lists as its containers.
    pub containers: Mutex<Value>,
    /// What the event stream sends, before falling silent.
    pub events: Mutex<Vec<Value>>,
    /// Update statuses a manager has not reported yet: (service id, status),
    /// shown from the second service listing after the update on.
    late: Mutex<Vec<(String, Value, u32)>>,
    /// How many task listings a removed service's tasks keep running for,
    /// as with a long stop grace period; meanwhile no network is removed.
    pub linger: Mutex<u32>,
    /// Removed services whose tasks still run: (service id, listings left).
    lingering: Mutex<Vec<(String, u32)>>,
}

pub fn secrets_json() -> Value {
    json!([{"ID": "sec1", "Version": {"Index": 1}, "CreatedAt": "2026-10-20T10:00:00Z", "UpdatedAt": "2026-10-20T10:00:00Z",
        "Spec": {"Name": "blog_db", "Labels": {"com.docker.stack.namespace": "blog"}}}])
}

pub fn configs_json() -> Value {
    json!([{"ID": "cfg1", "Version": {"Index": 1}, "Spec": {"Name": "blog_nginx", "Data": "c2VydmVyIHt9"}},
           {"ID": "cfg2", "Version": {"Index": 1}, "Spec": {"Name": "unused", "Data": ""}}])
}

/// The id of the swarm's own `host` network, which network lists lack.
pub const SWARM_HOST_ID: &str = "swarmhost";

pub fn networks_json() -> Value {
    json!([
        {"Name": "blog_default", "Id": "net1", "Scope": "swarm", "Driver": "overlay",
         "Attachable": false, "Internal": false, "Options": {"encrypted": ""},
         "IPAM": {"Config": [{"Subnet": "10.0.1.0/24"}]},
         "Labels": {"com.docker.stack.namespace": "blog"}},
        {"Name": "bridge", "Id": "b", "Scope": "local", "Driver": "bridge"}
    ])
}

/// A node container of stack `stack`, as `docker ps` lists it.
pub fn container_json(id: &str, stack: &str) -> Value {
    json!({
        "Id": id, "Names": [if id == "c0ffee" { "/web.1.abc".to_string() } else { format!("/{stack}_web.1.{id}") }], "Image": "nginx:1.27",
        "ImageID": "sha256:1", "Command": "nginx", "Created": 1,
        "State": "running", "Status": "Up 1 minute", "Ports": [],
        "Labels": {"com.docker.compose.project": stack, "com.docker.compose.service": "web",
                   "com.docker.stack.namespace": stack},
        "Mounts": [], "NetworkSettings": {"Networks": {}}
    })
}

/// A service update, a container start (the manager's own), a swarm
/// network's creation.
pub fn events_json() -> Vec<Value> {
    [
        ("service", "update", "s1", "swarm", json!({"name": "s1"})),
        ("container", "start", "c1", "local", json!({"name": "c1"})),
        (
            "network",
            "create",
            "net1",
            "swarm",
            json!({"name": "net1"}),
        ),
    ]
    .into_iter()
    .map(|(kind, action, id, scope, attrs)| {
        json!({"Type": kind, "Action": action, "Actor": {"ID": id, "Attributes": attrs},
               "scope": scope, "time": 1_792_576_800, "timeNano": 1_792_576_800_000_000_000i64})
    })
    .collect()
}

pub fn node_json(id: &str, manager: bool) -> Value {
    let mut n = json!({
        "ID": id,
        "Description": {"Hostname": format!("host-{id}")},
        "Version": {"Index": 5},
        "Spec": {"Role": if manager { "manager" } else { "worker" }, "Availability": "active"},
        "Status": {"State": "ready"},
    });
    if manager {
        n["ManagerStatus"] = json!({"Leader": id == "n1"});
    }
    n
}

impl FakeSwarm {
    pub fn new(pki: &SwarmPki, nodes: &[(&str, bool)]) -> Arc<Self> {
        Arc::new(FakeSwarm {
            cluster_id: pki.cluster_id.clone(),
            trust_root: pki.root.clone(),
            nodes: Mutex::new(nodes.iter().map(|(n, m)| node_json(n, *m)).collect()),
            requests: Mutex::default(),
            fail_tasks: Default::default(),
            tasks: Mutex::new(tasks_json()),
            services: Mutex::new(services_json()),
            writes: Mutex::default(),
            secrets: Mutex::new(secrets_json()),
            configs: Mutex::new(configs_json()),
            networks: Mutex::new(networks_json()),
            update_outcome: Mutex::default(),
            on_create_task: Mutex::default(),
            created: Mutex::default(),
            late: Mutex::default(),
            containers: Mutex::new(json!([container_json("c0ffee", "blog")])),
            events: Mutex::new(events_json()),
            linger: Mutex::default(),
            lingering: Mutex::default(),
        })
    }

    /// Promotes or demotes node `id`.
    pub fn set_role(&self, id: &str, manager: bool) {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(n) = nodes.iter_mut().find(|n| n["ID"] == id) {
            *n = node_json(id, manager);
        }
    }

    pub fn remove(&self, id: &str) {
        self.nodes.lock().unwrap().retain(|n| n["ID"] != id);
    }

    /// Sets node `id`'s status (`ready`, `down`, ...).
    pub fn set_state(&self, id: &str, state: &str) {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(n) = nodes.iter_mut().find(|n| n["ID"] == id) {
            n["Status"]["State"] = state.into();
        }
    }

    /// FleetOps' own service `s3` (`fleetops_fleetops`, labelled), with a
    /// running task on `node`.
    pub fn add_fleetops_server(&self, node: &str) {
        self.services.lock().unwrap().as_array_mut().unwrap().push(json!({
            "ID": "s3", "Version": {"Index": 7},
            "Spec": {
                "Name": "fleetops_fleetops",
                "Labels": {"io.fleetops.role": "server", "com.docker.stack.namespace": "fleetops"},
                "TaskTemplate": {"ContainerSpec": {"Image": "registry.example/ops/fleetops-server:1"}},
                "Mode": {"Replicated": {"Replicas": 1}}
            },
            "ServiceStatus": {"RunningTasks": 1, "DesiredTasks": 1}
        }));
        self.tasks
            .lock()
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(json!({
                "ID": "t3", "Version": {"Index": 30}, "ServiceID": "s3", "Slot": 1, "NodeID": node,
                "DesiredState": "running", "Status": {"State": "running", "Message": "started"}
            }));
    }

    /// Sets the update state of the service named `name`.
    pub fn set_update_state(&self, name: &str, state: &str) {
        let mut services = self.services.lock().unwrap();
        if let Some(svc) = services
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["Spec"]["Name"] == name)
        {
            svc["UpdateStatus"] = json!({"State": state, "Message": ""});
        }
    }

    /// The non-GET calls so far, as `METHOD path`.
    pub fn write_paths(&self) -> Vec<String> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .map(|(m, p, _)| format!("{m} {p}"))
            .collect()
    }

    /// The body of the last write whose path contains `part`.
    pub fn body_of(&self, part: &str) -> Value {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(_, p, _)| p.contains(part))
            .map(|(_, _, b)| b.clone())
            .unwrap_or_else(|| panic!("no write to {part}: {:?}", self.write_paths()))
    }

    fn answer_write(&self, method: &str, path: &str, body: &Value) -> (u16, String) {
        // As Docker does, a service names the swarm's own `host` network by
        // an id its network list lacks.
        let mut body = body.clone();
        if path.contains("/services/") {
            for n in body["TaskTemplate"]["Networks"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                if n["Target"] == "host" {
                    n["Target"] = json!(SWARM_HOST_ID);
                }
            }
        }
        let body = &body;
        let version = path
            .split("version=")
            .nth(1)
            .and_then(|v| v.split('&').next())
            .and_then(|v| v.parse::<u64>().ok());
        let out_of_sequence = (500, r#"{"message":"update out of sequence"}"#.to_string());
        let id_after = |kind: &str| {
            path.split(&format!("/{kind}/"))
                .nth(1)
                .and_then(|r| r.split(['/', '?']).next())
                .unwrap_or_default()
                .to_string()
        };
        if path.contains("/nodes/") && path.contains("/update") {
            let id = id_after("nodes");
            let mut nodes = self.nodes.lock().unwrap();
            let Some(n) = nodes.iter_mut().find(|n| n["ID"] == id.as_str()) else {
                return (
                    404,
                    json!({"message": format!("node {id} not found")}).to_string(),
                );
            };
            if version != n["Version"]["Index"].as_u64() {
                return out_of_sequence;
            }
            n["Spec"] = body.clone();
            n["Version"]["Index"] = (version.unwrap_or(0) + 1).into();
            return (200, String::new());
        }
        if path.contains("/services/") && path.contains("/update") {
            let id = id_after("services");
            let mut services = self.services.lock().unwrap();
            let Some(svc) = services
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|s| s["ID"] == id.as_str())
            else {
                return (
                    404,
                    json!({"message": format!("service {id} not found")}).to_string(),
                );
            };
            if version != svc["Version"]["Index"].as_u64() {
                return out_of_sequence;
            }
            if body.is_object() {
                svc["Spec"] = body.clone();
                let n = body["Mode"]["Replicated"]["Replicas"].as_u64().unwrap_or(1);
                svc["ServiceStatus"] = json!({"RunningTasks": n, "DesiredTasks": n});
            }
            svc["Version"]["Index"] = (version.unwrap_or(0) + 1).into();
            let (state, message) = self
                .update_outcome
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(("completed".into(), "update completed".into()));
            // As swarmkit does: the update clears the status, and the
            // updater reports its own a little later.
            svc["UpdateStatus"] = Value::Null;
            self.late
                .lock()
                .unwrap()
                .push((id, json!({"State": state, "Message": message}), 1));
            return (200, r#"{"Warnings":[]}"#.into());
        }
        for (kind, list) in [
            ("services", &self.services),
            ("secrets", &self.secrets),
            ("configs", &self.configs),
            ("networks", &self.networks),
        ] {
            if !path.contains(&format!("/{kind}/create")) {
                continue;
            }
            let id = {
                let mut n = self.created.lock().unwrap();
                *n += 1;
                let short = kind.trim_end_matches('s').replace("network", "net");
                if *n == 1 {
                    format!("{short}-new")
                } else {
                    format!("{short}-new-{n}")
                }
            };
            let object = match kind {
                "services" => {
                    let n = body["Mode"]["Replicated"]["Replicas"].as_u64().unwrap_or(1);
                    // A service with a task that never starts never runs.
                    let mut running = n;
                    if let Some(mut t) = self.on_create_task.lock().unwrap().clone() {
                        t["ServiceID"] = id.clone().into();
                        self.tasks.lock().unwrap().as_array_mut().unwrap().push(t);
                        running = 0;
                    }
                    json!({"ID": id, "Version": {"Index": 1}, "Spec": body,
                           "ServiceStatus": {"RunningTasks": running, "DesiredTasks": n}})
                }
                "networks" => json!({"Name": body["Name"], "Id": id, "Scope": "swarm",
                                     "Driver": body["Driver"], "Labels": body["Labels"],
                                     "Options": body["Options"]}),
                _ => json!({"ID": id, "Version": {"Index": 1}, "Spec": body}),
            };
            list.lock().unwrap().as_array_mut().unwrap().push(object);
            return match kind {
                "networks" => (201, json!({"Id": id, "Warning": ""}).to_string()),
                _ => (201, json!({"ID": id}).to_string()),
            };
        }
        if method == "DELETE" {
            if path.contains("/networks/") && !self.lingering.lock().unwrap().is_empty() {
                return (
                    400,
                    json!({"message": "network is in use by a task"}).to_string(),
                );
            }
            if path.contains("/services/") {
                let id = id_after("services");
                let linger = *self.linger.lock().unwrap();
                if linger > 0 {
                    self.lingering.lock().unwrap().push((id, linger));
                } else {
                    self.tasks
                        .lock()
                        .unwrap()
                        .as_array_mut()
                        .unwrap()
                        .retain(|t| t["ServiceID"] != id.as_str());
                }
            }
            if path.contains("/nodes/") {
                let id = id_after("nodes");
                self.remove(&id);
            }
            for (kind, list) in [
                ("services", &self.services),
                ("secrets", &self.secrets),
                ("configs", &self.configs),
                ("networks", &self.networks),
            ] {
                if path.contains(&format!("/{kind}/")) {
                    let id = id_after(kind);
                    list.lock().unwrap().as_array_mut().unwrap().retain(|o| {
                        o["ID"] != id.as_str() && o["Id"] != id.as_str() && o["Name"] != id.as_str()
                    });
                }
            }
            return (200, String::new());
        }
        // Anything else (a node view's container actions): as for reads.
        self.answer(path)
    }

    fn answer(&self, path: &str) -> (u16, String) {
        if let Some(rest) = path.split("/services/").nth(1) {
            let id = rest.split(['?', '/']).next().unwrap();
            return match self
                .services
                .lock()
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["ID"] == id)
            {
                Some(s) => (200, s.to_string()),
                None => (404, format!(r#"{{"message":"service {id} not found"}}"#)),
            };
        }
        if path.contains("/services") {
            let mut services = self.services.lock().unwrap();
            let mut late = self.late.lock().unwrap();
            late.retain_mut(|(id, status, wait)| {
                if *wait > 0 {
                    *wait -= 1;
                    return true;
                }
                if let Some(svc) = services
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|s| s["ID"] == id.as_str())
                {
                    svc["UpdateStatus"] = status.clone();
                }
                false
            });
            return (200, services.to_string());
        }
        if path.contains("/tasks") {
            let mut tasks = self.tasks.lock().unwrap();
            self.lingering.lock().unwrap().retain_mut(|(id, left)| {
                if *left > 0 {
                    *left -= 1;
                    return true;
                }
                tasks
                    .as_array_mut()
                    .unwrap()
                    .retain(|t| t["ServiceID"] != id.as_str());
                false
            });
            return (200, tasks.to_string());
        }
        if path.contains("/secrets") {
            // Docker never hands back a secret's data.
            let mut list = self.secrets.lock().unwrap().clone();
            for s in list.as_array_mut().unwrap() {
                s["Spec"].as_object_mut().map(|o| o.remove("Data"));
            }
            return (200, list.to_string());
        }
        if let Some(id) = path.split("/configs/").nth(1) {
            let id = id.split('?').next().unwrap_or_default();
            return match self
                .configs
                .lock()
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["ID"] == id)
            {
                Some(c) => (200, c.to_string()),
                None => (
                    404,
                    json!({"message": format!("config {id} not found")}).to_string(),
                ),
            };
        }
        if path.contains("/configs") {
            let mut list = self.configs.lock().unwrap().clone();
            for c in list.as_array_mut().unwrap() {
                c["Spec"].as_object_mut().map(|o| o.remove("Data"));
            }
            return (200, list.to_string());
        }
        if path.contains(&format!("/networks/{SWARM_HOST_ID}")) {
            return (
                200,
                json!({"Name": "host", "Id": SWARM_HOST_ID, "Scope": "swarm", "Driver": "host"})
                    .to_string(),
            );
        }
        if path.contains("/networks") {
            return (200, self.networks.lock().unwrap().to_string());
        }
        let nodes = self.nodes.lock().unwrap().clone();
        if let Some(id) = path.split("/nodes/").nth(1) {
            let id = id.split('?').next().unwrap_or_default();
            return match nodes.iter().find(|n| n["ID"] == id) {
                Some(n) => (200, n.to_string()),
                None => (
                    404,
                    json!({"message": format!("node {id} not found")}).to_string(),
                ),
            };
        }
        if path.contains("/nodes") {
            return (200, Value::Array(nodes).to_string());
        }
        if path.contains("/containers/json") {
            return (200, self.containers.lock().unwrap().to_string());
        }
        if let Some(rest) = path.split("/containers/").nth(1) {
            let id = rest.split(['/', '?']).next().unwrap_or_default();
            if rest.contains("/json") {
                return match self
                    .containers
                    .lock()
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["Id"] == id)
                {
                    Some(c) => (
                        200,
                        json!({"Id": id, "Name": c["Names"][0], "Image": "sha256:1",
                               "Config": {"Labels": c["Labels"], "Image": c["Image"]},
                               "State": {"Status": "running", "Running": true},
                               "HostConfig": {}, "Mounts": [], "NetworkSettings": {"Networks": {}}})
                        .to_string(),
                    ),
                    None => (
                        404,
                        json!({"message": format!("No such container: {id}")}).to_string(),
                    ),
                };
            }
        }
        if path.contains("/swarm") {
            return (
                200,
                json!({"ID": self.cluster_id, "TLSInfo": {"TrustRoot": self.trust_root}})
                    .to_string(),
            );
        }
        (
            200,
            r#"{"Version":"27.3.1","ApiVersion":"1.47","Os":"linux","Arch":"x86_64"}"#.into(),
        )
    }

    /// Answers one HTTP request on a tunnel stream, then closes it.
    pub fn serve(self: &Arc<Self>, mut s: StreamHandle) {
        let me = self.clone();
        tokio::spawn(async move {
            // The head, then as many body bytes as it announces.
            let mut buf = Vec::new();
            let mut chunk = vec![0u8; 8192];
            let head_end = loop {
                let Ok(n) = s.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while buf.len() < head_end + len {
                let Ok(n) = s.read(&mut chunk).await else {
                    return;
                };
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let method = head.split_whitespace().next().unwrap_or("GET").to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            me.requests.lock().unwrap().push(path.clone());
            if method != "GET" {
                let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                let (status, out) = me.answer_write(&method, &path, &body);
                me.writes.lock().unwrap().push((method, path, body));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{out}",
                    out.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
                return;
            }
            if path.contains("/events") {
                // `events`, then silence.
                let mut body = String::new();
                for e in me.events.lock().unwrap().iter() {
                    body.push_str(&e.to_string());
                    body.push('\n');
                }
                let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(body.as_bytes()).await;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return;
            }
            if path.contains("/logs") {
                let line = b"hello from blog_web.1\n";
                let mut body = vec![1u8, 0, 0, 0];
                body.extend_from_slice(&(line.len() as u32).to_be_bytes());
                body.extend_from_slice(line);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.multiplexed-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
                let _ = s.shutdown().await;
                return;
            }
            if path.contains("/tasks") && me.fail_tasks.load(std::sync::atomic::Ordering::SeqCst) {
                let body = r#"{"message":"manager unavailable"}"#;
                let resp = format!(
                    "HTTP/1.1 500 X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
                return;
            }
            let (status, body) = me.answer(&path);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.shutdown().await;
        });
    }
}

impl FakeSwarm {
    /// The node ids the daemon reports as managers.
    pub fn managers(&self) -> Vec<String> {
        self.nodes
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n["Spec"]["Role"] == "manager")
            .map(|n| n["ID"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// Pins and confirms the swarm as an admin would, with the daemon's
/// managers as the known ones.
pub async fn trust(t: &super::TestApp, env_id: i64, pki: &SwarmPki, daemon: &FakeSwarm) {
    let now = fleetops_core::now_unix();
    t.store
        .pin_swarm_cluster(env_id, &pki.cluster_id, &pki.root, "n1", "host-n1", now)
        .await
        .unwrap();
    assert!(t
        .store
        .confirm_swarm_cluster(env_id, &pki.cluster_id, &pki.root, &daemon.managers(), now)
        .await
        .unwrap());
}

pub struct Swarm {
    pub daemon: Arc<FakeSwarm>,
    pub t: super::TestApp,
    pub admin: super::Session,
    pub env_id: i64,
    pub _agents: Vec<super::agent::TestAgent>,
}

/// A trusted swarm "prod" with n1 (manager) and n2 (worker) connected.
pub async fn connected_swarm() -> Swarm {
    connected_swarm_with(fleetops_api::ApiConfig::default()).await
}

/// [`connected_swarm`] on a server with `config`.
pub async fn connected_swarm_with(config: fleetops_api::ApiConfig) -> Swarm {
    connected_swarm_on(super::app_with(config).await).await
}

/// [`connected_swarm`] on `t`.
pub async fn connected_swarm_on(t: super::TestApp) -> Swarm {
    let admin = t.setup_admin().await;
    let key = fleetops_api::agents::new_agent_key();
    let env_id = t
        .store
        .create_swarm_environment(
            "prod",
            &fleetops_api::auth_token_hash(&key),
            fleetops_core::now_unix(),
        )
        .await
        .unwrap()
        .id;
    t.state.envs.load(&t.store).await.unwrap();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let base = format!("http://{}", t.serve().await);
    let mut agents = vec![];
    for (node, manager) in [("n1", true), ("n2", false)] {
        let c = fleetops_tunnel::control::SwarmClaim {
            node_id: node.into(),
            cluster_id: "c1".into(),
            manager,
        };
        let mut a =
            super::agent::TestAgent::connect_swarm(&base, &key, daemon.clone() as Arc<FakeSwarm>)
                .await
                .unwrap();
        a.hello_swarm(c.clone()).await;
        a.answer_challenge(&pki.node(node, manager), &c).await;
        assert!(matches!(
            a.next().await,
            Some(fleetops_tunnel::control::Msg::Welcome { .. })
        ));
        agents.push(a);
    }
    super::eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
    Swarm {
        daemon,
        t,
        admin,
        env_id,
        _agents: agents,
    }
}

/// Grants set by the test, by user id: Bob's custom role.
#[derive(Default)]
pub struct Fixed(std::sync::Mutex<Vec<fleetops_core::access::Grant>>);

#[async_trait::async_trait]
impl fleetops_core::access::ExtraGrants for Fixed {
    async fn env_grants(
        &self,
        _: i64,
        _: i64,
    ) -> fleetops_core::Result<Vec<fleetops_core::access::Grant>> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn user_grants(
        &self,
        _: i64,
    ) -> fleetops_core::Result<std::collections::HashMap<i64, Vec<fleetops_core::access::Grant>>>
    {
        Ok(std::collections::HashMap::from([(
            1,
            self.0.lock().unwrap().clone(),
        )]))
    }
}

/// A swarm, and Bob with a custom role holding exactly `perms` on it,
/// limited to `stacks` when given.
pub async fn with_custom_in(
    perms: &[(fleetops_core::perm::Row, fleetops_core::perm::Verb)],
    stacks: Option<&[&str]>,
) -> (Swarm, super::Session) {
    with_grants(vec![custom_grant(perms, stacks)]).await
}

/// A custom grant holding exactly `perms`, limited to `stacks` when given.
pub fn custom_grant(
    perms: &[(fleetops_core::perm::Row, fleetops_core::perm::Verb)],
    stacks: Option<&[&str]>,
) -> fleetops_core::access::Grant {
    use fleetops_core::perm::{PermSet, Permission};
    let set: PermSet = perms.iter().map(|(r, v)| Permission::new(*r, *v)).collect();
    fleetops_core::access::Grant::custom(
        fleetops_core::access::CustomRef {
            id: 1,
            name: "test".into(),
        },
        set,
        stacks.map(|s| s.iter().map(|n| n.to_string()).collect()),
    )
}

/// A swarm, and Bob holding exactly `grants` on it.
pub async fn with_grants(grants: Vec<fleetops_core::access::Grant>) -> (Swarm, super::Session) {
    with_grants_on(grants, fleetops_api::ApiConfig::default()).await
}

/// [`with_grants`] on a server with `config`.
pub async fn with_grants_on(
    grants: Vec<fleetops_core::access::Grant>,
    config: fleetops_api::ApiConfig,
) -> (Swarm, super::Session) {
    let extra = std::sync::Arc::new(Fixed::default());
    let t = super::app_with_extra_and(extra.clone(), config).await;
    let sw = connected_swarm_on(t).await;
    sw.t.create_user(&sw.admin, "bob", "bob-password", "user")
        .await;
    *extra.0.lock().unwrap() = grants;
    let bob = super::session_from(&sw.t.login("bob", "bob-password").await);
    (sw, bob)
}

impl FakeSwarm {
    /// Adds service `shop_api` (`s4`, stack `shop`) with a task `t4`.
    pub fn add_shop(&self) {
        self.services
            .lock()
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(json!({
                "ID": "s4", "Version": {"Index": 3},
                "Spec": {
                    "Name": "shop_api",
                    "Labels": {"com.docker.stack.namespace": "shop"},
                    "TaskTemplate": {"ContainerSpec": {"Image": "api:1"}},
                    "Mode": {"Replicated": {"Replicas": 1}}
                },
                "ServiceStatus": {"RunningTasks": 1, "DesiredTasks": 1}
            }));
        self.tasks
            .lock()
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(json!({
                "ID": "t4", "Version": {"Index": 40}, "ServiceID": "s4", "Slot": 1, "NodeID": "n2",
                "Spec": {"ContainerSpec": {"Labels": {"com.docker.stack.namespace": "shop"}}},
                "DesiredState": "running", "Status": {"State": "running", "Message": "started"}
            }));
    }
}

/// [`with_custom_in`] on the whole swarm.
pub async fn with_custom(
    perms: &[(fleetops_core::perm::Row, fleetops_core::perm::Verb)],
) -> (Swarm, super::Session) {
    with_custom_in(perms, None).await
}
