//! A real agent in Swarm mode against a real server and fake node daemons.

use fleetops_agent::Config;
use fleetops_api::{ApiConfig, AppState};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Pki {
    root: String,
    ca: Issuer<'static, KeyPair>,
}

impl Pki {
    fn new() -> Self {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name.push(DnType::CommonName, "swarm-ca");
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let root = p.self_signed(&key).unwrap().pem();
        Pki {
            root,
            ca: Issuer::new(p, key),
        }
    }

    /// Writes the node's certificate directory as Docker lays it out.
    fn node_certs(&self, dir: &std::path::Path, node: &str, manager: bool) -> PathBuf {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name.push(DnType::CommonName, node);
        p.distinguished_name.push(DnType::OrganizationName, "c1");
        let ou = if manager { "swarm-manager" } else { "swarm-worker" };
        p.distinguished_name.push(DnType::OrganizationalUnitName, ou);
        let cert = p.signed_by(&key, &self.ca).unwrap().pem();
        let certs = dir.join(format!("certs-{node}"));
        std::fs::create_dir_all(&certs).unwrap();
        std::fs::write(certs.join("swarm-node.crt"), cert).unwrap();
        std::fs::write(certs.join("swarm-node.key"), key.serialize_pem()).unwrap();
        std::fs::write(certs.join("swarm-root-ca.crt"), &self.root).unwrap();
        certs
    }
}

fn node_json(id: &str, manager: bool) -> serde_json::Value {
    let mut n = serde_json::json!({
        "ID": id,
        "Description": {"Hostname": format!("host-{id}")},
        "Spec": {"Role": if manager { "manager" } else { "worker" }, "Availability": "active"},
        "Status": {"State": "ready"},
    });
    if manager {
        n["ManagerStatus"] = serde_json::json!({"Leader": true});
    }
    n
}

/// One node's daemon. `local_state` is `Swarm.LocalNodeState`.
fn fake_node(
    dir: &std::path::Path,
    node: &'static str,
    manager: bool,
    local_state: &'static str,
    root: String,
) -> PathBuf {
    let path = dir.join(format!("{node}.sock"));
    let l = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            let root = root.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let Ok(n) = c.read(&mut buf).await else { return };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let nodes = serde_json::json!([node_json("n1", true), node_json("n2", false)]);
                let (status, body) = if path.contains("/info") {
                    (200, serde_json::json!({"Swarm": {
                        "NodeID": node, "LocalNodeState": local_state,
                        "ControlAvailable": manager, "Cluster": {"ID": "c1"}}}).to_string())
                } else if let Some(id) = path.split("/nodes/").nth(1) {
                    match nodes.as_array().unwrap().iter().find(|n| n["ID"] == id) {
                        Some(n) => (200, n.to_string()),
                        None => (404, r#"{"message":"no such node"}"#.into()),
                    }
                } else if path.contains("/nodes") {
                    (200, nodes.to_string())
                } else if path.contains("/swarm") {
                    (200, serde_json::json!({"ID": "c1", "TLSInfo": {"TrustRoot": root}}).to_string())
                } else {
                    (200, r#"{"Version":"27.3.1","ApiVersion":"1.47","Os":"linux","Arch":"x86_64"}"#.into())
                };
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

struct Server {
    base: String,
    state: AppState,
}

async fn server(store: Store) -> Server {
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    let state = AppState::new(
        store.clone(),
        ApiConfig::default(),
        Arc::new(DockerEnvFactory::new(agents.clone())),
    )
    .with_agents(agents);
    state.envs.load(&store).await.unwrap();
    let app = fleetops_api::app(state.clone(), axum::Router::new());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server { base, state }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A swarm environment and its key, written where `--key-file` reads it.
async fn swarm_env(store: &Store, dir: &std::path::Path) -> (i64, PathBuf) {
    let key = fleetops_api::agents::new_agent_key();
    let env = store
        .create_swarm_environment(
            "prod",
            &fleetops_api::auth_token_hash(&key),
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    let file = dir.join("fleetops-agent-key");
    std::fs::write(&file, format!("{key}\n")).unwrap();
    (env.id, file)
}

fn config(base: &str, docker: &std::path::Path, key: &std::path::Path, certs: &std::path::Path) -> Config {
    Config {
        server: base.into(),
        join_token: None,
        data: certs.join("unused-data"),
        docker: format!("unix://{}", docker.display()),
        ca: None,
        ca_pem: None,
        log: "warn".into(),
        server_silence: Duration::from_secs(45),
        kube: false,
        key_secret: None,
        kube_upstream: None,
        swarm: true,
        key_file: Some(key.into()),
        swarm_certs: certs.into(),
    }
}

#[tokio::test]
async fn a_swarm_agent_proves_its_node_and_connects() {
    let dir = tempfile::tempdir().unwrap();
    let pki = Pki::new();
    let store = Store::in_memory().await.unwrap();
    let (env_id, key) = swarm_env(&store, dir.path()).await;
    let srv = server(store.clone()).await;
    let docker = fake_node(dir.path(), "n1", true, "active", pki.root.clone());
    let certs = pki.node_certs(dir.path(), "n1", true);
    tokio::spawn(fleetops_agent::run(config(&srv.base, &docker, &key, &certs)));

    eventually("registered", || srv.state.agents.nodes(env_id).len() == 1).await;
    let env = srv.state.envs.get("prod").unwrap();
    match env.status().await {
        fleetops_core::env::EnvStatus::Up {
            version: fleetops_core::env::EnvVersion::Swarm(v),
        } => assert_eq!(v.serving_node, "n1"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        store.get_swarm_cluster(env_id).await.unwrap().unwrap().cluster_id,
        "c1"
    );
}

#[tokio::test]
async fn a_node_outside_a_swarm_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let pki = Pki::new();
    let store = Store::in_memory().await.unwrap();
    let (_, key) = swarm_env(&store, dir.path()).await;
    let srv = server(store.clone()).await;
    let docker = fake_node(dir.path(), "", false, "inactive", pki.root.clone());
    let certs = pki.node_certs(dir.path(), "n1", true);
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        fleetops_agent::run(config(&srv.base, &docker, &key, &certs)),
    )
    .await
    .expect("returns")
    .unwrap_err();
    assert!(
        err.to_string().contains("not part of an active swarm"),
        "{err:#}"
    );
}

#[tokio::test]
async fn not_yet_backs_off_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let pki = Pki::new();
    let store = Store::in_memory().await.unwrap();
    let (env_id, key) = swarm_env(&store, dir.path()).await;
    let srv = server(store.clone()).await;
    let worker = fake_node(dir.path(), "n2", false, "active", pki.root.clone());
    let worker_certs = pki.node_certs(dir.path(), "n2", false);
    let w = tokio::spawn(fleetops_agent::run(config(&srv.base, &worker, &key, &worker_certs)));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(srv.state.agents.nodes(env_id).is_empty(), "no manager yet");
    assert!(!w.is_finished(), "the worker keeps retrying");

    let manager = fake_node(dir.path(), "n1", true, "active", pki.root.clone());
    let manager_certs = pki.node_certs(dir.path(), "n1", true);
    tokio::spawn(fleetops_agent::run(config(&srv.base, &manager, &key, &manager_certs)));
    eventually("both nodes", || srv.state.agents.nodes(env_id).len() == 2).await;
}
