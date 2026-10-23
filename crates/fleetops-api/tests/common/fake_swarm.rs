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
        p.distinguished_name.push(DnType::OrganizationalUnitName, ou);
        NodeCert {
            chain: p.signed_by(&key, &self.ca).unwrap().pem(),
            key: key.serialize_pem(),
        }
    }
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

    pub fn remove(&self, id: &str) {
        self.nodes.lock().unwrap().retain(|n| n["ID"] != id);
    }

    fn answer(&self, path: &str) -> (u16, String) {
        let nodes = self.nodes.lock().unwrap().clone();
        if let Some(id) = path.split("/nodes/").nth(1) {
            let id = id.split('?').next().unwrap_or_default();
            return match nodes.iter().find(|n| n["ID"] == id) {
                Some(n) => (200, n.to_string()),
                None => (404, json!({"message": format!("node {id} not found")}).to_string()),
            };
        }
        if path.contains("/nodes") {
            return (200, Value::Array(nodes).to_string());
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
            let Ok(n) = s.read(&mut buf).await else { return };
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
