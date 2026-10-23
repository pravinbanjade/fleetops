//! A Docker Swarm behind one agent per node (swarm spec §1, §2). Cluster
//! calls go to a manager's session, the leader's when known; node calls go
//! to that node's session. Up while any manager's agent is connected.

use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion, Environment};
use fleetops_core::swarm::{NodeRole, SwarmOps, SwarmVersion};
use fleetops_tunnel::registry::{Live, Registry};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SwarmEnv {
    id: i64,
    name: String,
    agents: Arc<Registry>,
    /// One client per node, keyed by the session it was made for.
    clients: Mutex<HashMap<String, (u64, Arc<LocalDocker>)>>,
    /// The leader's node id, as last reported by a manager.
    leader: Mutex<Option<String>>,
}

fn is_manager(live: &Live) -> bool {
    live.info.host.swarm.as_ref().is_some_and(|s| s.manager)
}

impl SwarmEnv {
    pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self {
        Self {
            id,
            name: name.to_string(),
            agents,
            clients: Mutex::default(),
            leader: Mutex::default(),
        }
    }

    fn client(&self, node: &str) -> Option<Arc<LocalDocker>> {
        let live = self.agents.get_node(self.id, node)?;
        let mut clients = self.clients.lock().unwrap();
        if let Some((id, c)) = clients.get(node) {
            if *id == live.id {
                return Some(c.clone());
            }
        }
        let c = Arc::new(LocalDocker::through(Arc::new(live.session)).ok()?);
        clients.insert(node.to_string(), (live.id, c.clone()));
        Some(c)
    }

    /// Connected managers, the cached leader first, then by node id.
    fn connected_managers(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .agents
            .nodes(self.id)
            .into_iter()
            .filter(|(_, l)| is_manager(l))
            .map(|(n, _)| n)
            .collect();
        if let Some(leader) = self.leader.lock().unwrap().as_ref() {
            if let Some(i) = v.iter().position(|n| n == leader) {
                let l = v.remove(i);
                v.insert(0, l);
            }
        }
        v
    }

    /// Live manager sessions, leader first when known. Asks a manager who
    /// leads, so the answer is fresh.
    pub async fn managers(&self) -> Vec<String> {
        for node in self.connected_managers() {
            let Some(c) = self.client(&node) else { continue };
            if let Ok(Ok(nodes)) = tokio::time::timeout(STATUS_TIMEOUT, c.nodes()).await {
                *self.leader.lock().unwrap() =
                    nodes.into_iter().find(|n| n.leader).map(|n| n.id);
                break;
            }
        }
        self.connected_managers()
    }

    fn serving(&self) -> Option<(String, Arc<LocalDocker>)> {
        self.connected_managers()
            .into_iter()
            .find_map(|n| self.client(&n).map(|c| (n, c)))
    }
}

#[async_trait]
impl Environment for SwarmEnv {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::SwarmAgent
    }

    async fn status(&self) -> EnvStatus {
        self.managers().await;
        let Some((node, client)) = self.serving() else {
            let reason = self
                .agents
                .offline_reason(self.id)
                .map(|r| format!("agent offline: {r}"))
                .unwrap_or_else(|| "waiting for a manager's agent to connect".into());
            return EnvStatus::Down { reason };
        };
        let probe = async {
            let version = client.version().await?;
            let info = client.swarm_info().await?;
            let nodes = client.nodes().await?;
            Ok::<_, fleetops_core::Error>((version, info, nodes))
        };
        match tokio::time::timeout(STATUS_TIMEOUT, probe).await {
            Ok(Ok((version, info, nodes))) => {
                let managers = nodes
                    .iter()
                    .filter(|n| n.role == NodeRole::Manager)
                    .count() as u32;
                EnvStatus::Up {
                    version: EnvVersion::Swarm(SwarmVersion {
                        engine: version.version,
                        cluster_id: info.cluster_id,
                        managers,
                        workers: nodes.len() as u32 - managers,
                        serving_node: node,
                    }),
                }
            }
            Ok(Err(e)) => EnvStatus::Down {
                reason: format!("manager agent connected, Docker unreachable: {e}"),
            },
            Err(_) => EnvStatus::Down {
                reason: format!("no response from the manager within {STATUS_TIMEOUT:?}"),
            },
        }
    }

    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }

    fn swarm(&self) -> Option<Arc<dyn SwarmOps>> {
        self.serving().map(|(_, c)| c as Arc<dyn SwarmOps>)
    }

    fn node(&self, node_id: &str) -> Option<Arc<dyn DockerOps>> {
        self.client(node_id).map(|c| c as Arc<dyn DockerOps>)
    }

    async fn wait_change(&self) {
        self.agents.wait_change().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_tunnel::{
        control::{HostInfo, Serves, SwarmClaim},
        registry::AgentInfo,
        testing::loopback,
        Target,
    };
    use tokio::sync::mpsc;

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

    fn add(agents: &Registry, node: &str, manager: bool) {
        let (bye, rx) = mpsc::channel(1);
        std::mem::forget(rx);
        agents.insert_node(
            5,
            node,
            "k",
            loopback(Target::Unix("/nonexistent.sock".into())),
            info(node, manager),
            bye,
        );
    }

    #[tokio::test]
    async fn without_sessions_it_is_down() {
        let env = SwarmEnv::new(5, "prod", Arc::new(Registry::default()));
        match env.status().await {
            EnvStatus::Down { reason } => {
                assert_eq!(reason, "waiting for a manager's agent to connect")
            }
            other => panic!("{other:?}"),
        }
        assert!(env.swarm().is_none());
    }

    #[tokio::test]
    async fn workers_alone_do_not_bring_it_up() {
        let agents = Arc::new(Registry::default());
        add(&agents, "w1", false);
        let env = SwarmEnv::new(5, "prod", agents);
        assert!(matches!(env.status().await, EnvStatus::Down { .. }));
        assert!(env.swarm().is_none());
        assert!(env.node("w1").is_some(), "node calls still reach the worker");
    }

    #[tokio::test]
    async fn node_clients_are_per_session() {
        let agents = Arc::new(Registry::default());
        let env = SwarmEnv::new(5, "prod", agents.clone());
        add(&agents, "n1", true);
        let a = env.client("n1").unwrap();
        assert!(Arc::ptr_eq(&a, &env.client("n1").unwrap()));
        add(&agents, "n1", true);
        assert!(!Arc::ptr_eq(&a, &env.client("n1").unwrap()));
    }

    #[tokio::test]
    async fn docker_is_none() {
        let agents = Arc::new(Registry::default());
        add(&agents, "n1", true);
        let env = SwarmEnv::new(5, "prod", agents);
        assert!(env.docker().is_none());
    }
}
