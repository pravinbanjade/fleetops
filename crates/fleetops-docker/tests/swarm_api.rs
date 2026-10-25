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

fn services_json() -> serde_json::Value {
    serde_json::json!([
        {
            "ID": "s1", "Version": {"Index": 10},
            "CreatedAt": "2026-10-20T10:00:00Z", "UpdatedAt": "2026-10-21T10:00:00Z",
            "Spec": {
                "Name": "blog_web",
                "Labels": {"com.docker.stack.namespace": "blog"},
                "TaskTemplate": {
                    "ContainerSpec": {
                        "Image": "nginx:1.27@sha256:abc",
                        "Healthcheck": {"Test": ["CMD", "true"], "Interval": 10_000_000_000i64},
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

fn tasks_json() -> serde_json::Value {
    serde_json::json!([
        {
            "ID": "t1", "Version": {"Index": 20}, "ServiceID": "s1", "Slot": 1, "NodeID": "n1",
            "Spec": {"ContainerSpec": {"Labels": {"com.docker.stack.namespace": "blog"}}},
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

fn answer(path: &str) -> (u16, String) {
    if let Some(rest) = path.split("/services/").nth(1) {
        let id = rest.split(['?', '/']).next().unwrap();
        return match services_json()
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
        return (200, services_json().to_string());
    }
    if path.contains("/tasks") {
        return (200, tasks_json().to_string());
    }
    if path.contains("/secrets") {
        return (
            200,
            serde_json::json!([{"ID": "sec1", "CreatedAt": "2026-10-20T10:00:00Z", "UpdatedAt": "2026-10-20T10:00:00Z",
                "Spec": {"Name": "blog_db", "Labels": {"com.docker.stack.namespace": "blog"}}}])
            .to_string(),
        );
    }
    if path.contains("/configs") {
        return (
            200,
            serde_json::json!([{"ID": "cfg1", "Spec": {"Name": "blog_nginx"}},
                               {"ID": "cfg2", "Spec": {"Name": "unused"}}])
            .to_string(),
        );
    }
    if path.contains("/networks") {
        return (
            200,
            serde_json::json!([
                {"Name": "blog_default", "Id": "net1", "Scope": "swarm", "Driver": "overlay",
                 "Attachable": false, "Internal": false, "Options": {"encrypted": ""},
                 "IPAM": {"Config": [{"Subnet": "10.0.1.0/24"}]},
                 "Labels": {"com.docker.stack.namespace": "blog"}},
                {"Name": "bridge", "Id": "b", "Scope": "local", "Driver": "bridge"}
            ])
            .to_string(),
        );
    }
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
                if path.contains("/logs") {
                    let line = b"hello from blog_web.1\n";
                    let mut body = vec![1u8, 0, 0, 0];
                    body.extend_from_slice(&(line.len() as u32).to_be_bytes());
                    body.extend_from_slice(line);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.multiplexed-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = c.write_all(head.as_bytes()).await;
                    let _ = c.write_all(&body).await;
                    return;
                }
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

#[tokio::test]
async fn a_node_view_reaches_that_nodes_engine() {
    let dir = tempfile::tempdir().unwrap();
    let sock = fake_manager(dir.path());
    let agents = Arc::new(Registry::default());
    let env = SwarmEnv::new(9, "prod", agents.clone());
    let (bye, rx) = mpsc::channel(1);
    std::mem::forget(rx);
    agents.insert_node(
        9,
        "n3",
        "k",
        loopback(Target::Unix(sock)),
        info("n3", false),
        bye,
    );
    let view = env.node_env("n3").expect("a node view");
    assert_eq!(view.name(), "prod@n3");
    assert_eq!(view.id(), 9);
    assert_eq!(view.kind(), fleetops_core::env::EnvKind::SwarmNode);
    assert!(view.docker().is_some());
    match view.status().await {
        EnvStatus::Up {
            version: EnvVersion::Docker(v),
        } => assert_eq!(v.version, "27.3.1"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_node_without_an_agent_is_down() {
    let dir = tempfile::tempdir().unwrap();
    let sock = fake_manager(dir.path());
    let agents = Arc::new(Registry::default());
    let env = SwarmEnv::new(9, "prod", agents.clone());
    let (bye, rx) = mpsc::channel(1);
    std::mem::forget(rx);
    agents.insert_node(
        9,
        "n1",
        "k",
        loopback(Target::Unix(sock)),
        info("n1", true),
        bye,
    );
    // n3 is in the swarm (a manager reports it) but has no agent.
    env.status().await;
    let view = env.node_env("n3").expect("a known node");
    assert!(view.docker().is_none());
    match view.status().await {
        EnvStatus::Down { reason } => assert_eq!(reason, "node n3 has no connected agent"),
        other => panic!("{other:?}"),
    }
    assert!(env.node_env("zz").is_none(), "not in the swarm");
}

#[tokio::test]
async fn node_views_are_cached_per_node() {
    let agents = Arc::new(Registry::default());
    let env = SwarmEnv::new(9, "prod", agents.clone());
    let (bye, rx) = mpsc::channel(1);
    std::mem::forget(rx);
    agents.insert_node(
        9,
        "n3",
        "k",
        loopback(Target::Unix("/nonexistent.sock".into())),
        info("n3", false),
        bye,
    );
    let a = env.node_env("n3").unwrap();
    let b = env.node_env("n3").unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}

fn manager(dir: &std::path::Path) -> LocalDocker {
    let sock = fake_manager(dir);
    LocalDocker::connect(&format!("unix://{}", sock.display())).unwrap()
}

#[tokio::test]
async fn services_report_running_and_desired_and_their_stack() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let svcs = d.services().await.unwrap();
    assert_eq!(svcs.len(), 2);
    let web = &svcs[0];
    assert_eq!(web.name, "blog_web");
    assert_eq!(web.stack.as_deref(), Some("blog"));
    assert_eq!(web.mode, "replicated");
    assert_eq!(web.replicas_desired, Some(3));
    assert_eq!((web.tasks_running, web.tasks_desired), (2, 3));
    assert_eq!(web.image, "nginx:1.27");
    assert_eq!(web.ports, ["8080:80/tcp"]);
    assert_eq!(web.update_state.as_deref(), Some("updating"));
    assert_eq!(web.updated_at, 1_792_576_800);
    assert_eq!(svcs[1].mode, "global");
    assert_eq!(svcs[1].replicas_desired, None);
    let detail = d.service("s1").await.unwrap().unwrap();
    assert_eq!(detail.summary.name, "blog_web");
    assert_eq!(detail.spec["Name"], "blog_web");
}

#[tokio::test]
async fn a_missing_service_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    assert!(d.service("zz").await.unwrap().is_none());
}

#[tokio::test]
async fn tasks_carry_node_container_and_error() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let tasks = d.tasks().await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].service_name, "blog_web");
    assert_eq!(tasks[0].node_id.as_deref(), Some("n1"));
    assert_eq!(tasks[0].container_id.as_deref(), Some("c1"));
    assert_eq!(tasks[0].state, "running");
    assert_eq!(tasks[0].version, 20);
    assert_eq!(tasks[1].state, "rejected");
    assert_eq!(tasks[1].desired_state, "shutdown");
    assert_eq!(tasks[1].error.as_deref(), Some("No such image: nginx:9"));
}

#[tokio::test]
async fn secrets_and_configs_list_who_uses_them() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let secrets = d.secrets().await.unwrap();
    assert_eq!(secrets[0].name, "blog_db");
    assert_eq!(secrets[0].stack.as_deref(), Some("blog"));
    assert_eq!(secrets[0].used_by, ["blog_web"]);
    let configs = d.configs().await.unwrap();
    assert_eq!(configs[0].used_by, ["blog_web"]);
    assert!(configs[1].used_by.is_empty());
}

#[tokio::test]
async fn only_swarm_scoped_networks_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let nets = d.networks().await.unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].name, "blog_default");
    assert!(nets[0].encrypted);
    assert_eq!(nets[0].subnets, ["10.0.1.0/24"]);
    assert_eq!(nets[0].used_by, ["blog_web"]);
}

#[tokio::test]
async fn service_logs_stream_lines() {
    use futures_util::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let lines: Vec<_> = d
        .service_logs("s1", &fleetops_core::streams::LogOptions::default())
        .collect()
        .await;
    let line = lines[0].as_ref().unwrap();
    assert_eq!(line.text, "hello from blog_web.1");
}

/// Docker calls the field `Healthcheck`; bollard's model says `HealthCheck`
/// and drops it on read. A service read must keep it, under the name the
/// model writes back, or a scale or an edit would remove it.
#[tokio::test]
async fn a_services_healthcheck_survives_a_read() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let spec = d.service("s1").await.unwrap().unwrap().spec;
    let cs = &spec["TaskTemplate"]["ContainerSpec"];
    assert_eq!(
        cs["HealthCheck"]["Test"],
        serde_json::json!(["CMD", "true"]),
        "{cs}"
    );
    assert!(cs.get("Healthcheck").is_none(), "{cs}");
}

#[tokio::test]
async fn a_services_healthcheck_survives_a_read_through_an_agent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = fake_manager(dir.path());
    let agents = Arc::new(Registry::default());
    let env = SwarmEnv::new(9, "prod", agents.clone());
    let (bye, rx) = mpsc::channel(1);
    std::mem::forget(rx);
    agents.insert_node(
        9,
        "n2",
        "k",
        loopback(Target::Unix(sock.clone())),
        info("n2", true),
        bye,
    );
    let ops = env.swarm().expect("a manager serves");
    let spec = ops.service("s1").await.unwrap().unwrap().spec;
    assert_eq!(
        spec["TaskTemplate"]["ContainerSpec"]["HealthCheck"]["Interval"],
        10_000_000_000i64
    );
}

#[tokio::test]
async fn tasks_carry_their_stack() {
    let dir = tempfile::tempdir().unwrap();
    let d = manager(dir.path());
    let tasks = d.tasks().await.unwrap();
    assert_eq!(tasks[0].stack.as_deref(), Some("blog"));
    assert_eq!(tasks[1].stack, None);
}
