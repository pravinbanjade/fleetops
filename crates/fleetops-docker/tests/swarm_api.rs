//! Swarm calls against a fake manager daemon, directly and through an agent
//! session.

use fleetops_core::env::{EnvStatus, EnvVersion, Environment};
use fleetops_core::swarm::{NodeRole, SwarmOps};
use fleetops_docker::{LocalDocker, SwarmEnv};
use fleetops_tunnel::{
    control::{HostInfo, Serves, SwarmClaim},
    registry::{AgentInfo, Registry},
    testing::loopback,
    Target,
};
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

fn node(id: &str, host: &str, role: &str, leader: bool) -> serde_json::Value {
    let mut n = serde_json::json!({
        "ID": id,
        "Description": {"Hostname": host},
        "Spec": {"Role": role, "Availability": "active"},
        "Status": {"State": "ready"},
    });
    if role == "manager" {
        n["ManagerStatus"] = serde_json::json!({"Leader": leader, "Reachability": "reachable"});
    }
    n
}

fn answer(path: &str) -> (u16, String) {
    let nodes = serde_json::json!([
        node("n1", "mgr-1", "manager", false),
        node("n2", "mgr-2", "manager", true),
        node("n3", "wrk-1", "worker", false),
    ]);
    if let Some(id) = path.split("/nodes/").nth(1) {
        let id = id.split('?').next().unwrap();
        return match nodes.as_array().unwrap().iter().find(|n| n["ID"] == id) {
            Some(n) => (200, n.to_string()),
            None => (404, format!(r#"{{"message":"node {id} not found"}}"#)),
        };
    }
    if path.contains("/nodes") {
        return (200, nodes.to_string());
    }
    if path.contains("/swarm") {
        return (
            200,
            r#"{"ID":"c1","TLSInfo":{"TrustRoot":"-----BEGIN CERTIFICATE-----\nROOT\n-----END CERTIFICATE-----\n"}}"#.into(),
        );
    }
    if path.contains("/version") {
        return (
            200,
            r#"{"Version":"27.3.1","ApiVersion":"1.47","Os":"linux","Arch":"amd64"}"#.into(),
        );
    }
    (404, r#"{"message":"not found"}"#.into())
}

/// A manager daemon on a unix socket: one request per connection.
fn fake_manager(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("docker.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = c.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = answer(&path);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = c.write_all(resp.as_bytes()).await;
            });
        }
    });
    path
}

fn info(node: &str, manager: bool) -> AgentInfo {
    AgentInfo {
        agent_version: "0.7.0".into(),
        host: HostInfo {
            serves: Serves::Swarm,
            swarm: Some(SwarmClaim {
                node_id: node.into(),
                cluster_id: "c1".into(),
                manager,
            }),
            ..Default::default()
        },
        connected_at: 0,
    }
}

#[tokio::test]
async fn swarm_info_reads_trust_root() {
    let dir = tempfile::tempdir().unwrap();
    let sock = fake_manager(dir.path());
    let d = LocalDocker::connect(&format!("unix://{}", sock.display())).unwrap();
    let info = d.swarm_info().await.unwrap();
    assert_eq!(info.cluster_id, "c1");
    assert!(info.trust_root.contains("ROOT"));
    let nodes = d.nodes().await.unwrap();
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].hostname, "mgr-1");
    assert_eq!(nodes[0].role, NodeRole::Manager);
    assert!(!nodes[0].leader);
    assert!(nodes[1].leader);
    assert_eq!(nodes[2].role, NodeRole::Worker);
    assert_eq!(nodes[2].state, "ready");
    assert_eq!(nodes[2].availability, "active");
    assert_eq!(d.node("n3").await.unwrap().unwrap().hostname, "wrk-1");
    assert_eq!(d.node("zz").await.unwrap(), None);
}

#[tokio::test]
async fn a_manager_session_brings_it_up_and_the_leader_serves() {
    let dir = tempfile::tempdir().unwrap();
    let sock = fake_manager(dir.path());
    let agents = Arc::new(Registry::default());
    let env = SwarmEnv::new(9, "prod", agents.clone());
    for (node, manager) in [("n1", true), ("n2", true), ("n3", false)] {
        let (bye, rx) = mpsc::channel(1);
        std::mem::forget(rx);
        agents.insert_node(
            9,
            node,
            "k",
            loopback(Target::Unix(sock.clone())),
            info(node, manager),
            bye,
        );
    }
    assert_eq!(env.managers().await, ["n2", "n1"], "leader first");
    match env.status().await {
        EnvStatus::Up {
            version: EnvVersion::Swarm(v),
        } => {
            assert_eq!(v.engine, "27.3.1");
            assert_eq!(v.cluster_id, "c1");
            assert_eq!((v.managers, v.workers), (2, 1));
            assert_eq!(v.serving_node, "n2");
        }
        other => panic!("{other:?}"),
    }
    assert!(env.swarm().is_some());
    assert!(env.node("n3").is_some());
    assert!(env.node("n4").is_none());
}
