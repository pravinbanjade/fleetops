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
            "UpdatedAt": "2026-10-21T10:00:00Z", "DesiredState": "running",
            "Status": {"State": "running", "Message": "started", "ContainerStatus": {"ContainerID": "c1"}}
        },
        {
            "ID": "t2", "Version": {"Index": 21}, "ServiceID": "s1", "Slot": 2, "NodeID": "n3",
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
}

pub fn node_json(id: &str, manager: bool) -> Value {
    let mut n = json!({
        "ID": id,
        "Description": {"Hostname": format!("host-{id}")},
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

    fn answer(&self, path: &str) -> (u16, String) {
    if let Some(rest) = path.split("/services/").nth(1) {
        let id = rest.split(['?', '/']).next().unwrap();
        return match services_json().as_array().unwrap().iter().find(|s| s["ID"] == id) {
            Some(s) => (200, s.to_string()),
            None => (404, format!(r#"{{"message":"service {id} not found"}}"#)),
        };
    }
    if path.contains("/services") {
        return (200, services_json().to_string());
    }
    if path.contains("/tasks") {
        return (200, tasks_json().to_string());
    }
    if path.contains("/secrets") {
        return (
            200,
            json!([{"ID": "sec1", "CreatedAt": "2026-10-20T10:00:00Z", "UpdatedAt": "2026-10-20T10:00:00Z",
                "Spec": {"Name": "blog_db", "Labels": {"com.docker.stack.namespace": "blog"}}}])
            .to_string(),
        );
    }
    if path.contains("/configs") {
        return (
            200,
            json!([{"ID": "cfg1", "Spec": {"Name": "blog_nginx"}},
                               {"ID": "cfg2", "Spec": {"Name": "unused"}}])
            .to_string(),
        );
    }
    if path.contains("/networks") {
        return (
            200,
            json!([
                {"Name": "blog_default", "Id": "net1", "Scope": "swarm", "Driver": "overlay",
                 "Attachable": false, "Internal": false, "Options": {"encrypted": ""},
                 "IPAM": {"Config": [{"Subnet": "10.0.1.0/24"}]},
                 "Labels": {"com.docker.stack.namespace": "blog"}},
                {"Name": "bridge", "Id": "b", "Scope": "local", "Driver": "bridge"}
            ])
            .to_string(),
        );
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
            return (
                200,
                json!([{
                    "Id": "c0ffee", "Names": ["/web.1.abc"], "Image": "nginx:1.27",
                    "ImageID": "sha256:1", "Command": "nginx", "Created": 1,
                    "State": "running", "Status": "Up 1 minute", "Ports": [],
                    "Labels": {"com.docker.compose.project": "blog", "com.docker.compose.service": "web"},
                    "Mounts": [], "NetworkSettings": {"Networks": {}}
                }])
                .to_string(),
            );
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
            let mut buf = vec![0u8; 8192];
            let Ok(n) = s.read(&mut buf).await else {
                return;
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
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
    pub t: super::TestApp,
    pub admin: super::Session,
    pub env_id: i64,
    pub _agents: Vec<super::agent::TestAgent>,
}

/// A trusted swarm "prod" with n1 (manager) and n2 (worker) connected.
pub async fn connected_swarm() -> Swarm {
    let t = super::app().await;
    let admin = t.setup_admin().await;
    let key = fleetops_api::agents::new_agent_key();
    let env_id = t
        .store
        .create_swarm_environment("prod", &fleetops_api::auth_token_hash(&key), fleetops_core::now_unix())
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
        let mut a = super::agent::TestAgent::connect_swarm(&base, &key, daemon.clone() as Arc<FakeSwarm>)
            .await
            .unwrap();
        a.hello_swarm(c.clone()).await;
        a.answer_challenge(&pki.node(node, manager), &c).await;
        assert!(matches!(a.next().await, Some(fleetops_tunnel::control::Msg::Welcome { .. })));
        agents.push(a);
    }
    super::eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
    Swarm {
        t,
        admin,
        env_id,
        _agents: agents,
    }
}

