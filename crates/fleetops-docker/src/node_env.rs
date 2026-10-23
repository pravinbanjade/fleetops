//! One node of a Swarm environment as a Docker host of its own (swarm spec
//! §1, §4 node pages). It has the swarm's id, so roles on the swarm apply,
//! and reaches the node's engine through that node's agent session.

use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion, Environment};
use fleetops_tunnel::registry::Registry;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

pub struct NodeEnv {
    swarm_id: i64,
    name: String,
    node: String,
    agents: Arc<Registry>,
    client: Mutex<Option<(u64, Arc<LocalDocker>)>>,
}

impl NodeEnv {
    pub fn new(swarm_id: i64, swarm_name: &str, node: &str, agents: Arc<Registry>) -> Self {
        Self {
            swarm_id,
            name: fleetops_core::env::node_view_name(swarm_name, node),
            node: node.to_string(),
            agents,
            client: Mutex::new(None),
        }
    }

    fn client(&self) -> Option<Arc<LocalDocker>> {
        let live = self.agents.get_node(self.swarm_id, &self.node)?;
        let mut slot = self.client.lock().unwrap();
        if let Some((id, c)) = slot.as_ref() {
            if *id == live.id {
                return Some(c.clone());
            }
        }
        let c = Arc::new(LocalDocker::through(Arc::new(live.session)).ok()?);
        *slot = Some((live.id, c.clone()));
        Some(c)
    }
}

#[async_trait]
impl Environment for NodeEnv {
    fn id(&self) -> i64 {
        self.swarm_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::SwarmNode
    }

    async fn status(&self) -> EnvStatus {
        let Some(client) = self.client() else {
            return EnvStatus::Down {
                reason: format!("node {} has no connected agent", self.node),
            };
        };
        match tokio::time::timeout(STATUS_TIMEOUT, client.version()).await {
            Ok(Ok(version)) => EnvStatus::Up {
                version: EnvVersion::Docker(version),
            },
            Ok(Err(e)) => EnvStatus::Down {
                reason: format!("agent connected, Docker unreachable: {e}"),
            },
            Err(_) => EnvStatus::Down {
                reason: format!("no response from node {} within {STATUS_TIMEOUT:?}", self.node),
            },
        }
    }

    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        self.client().map(|c| c as Arc<dyn DockerOps>)
    }

    async fn wait_change(&self) {
        self.agents.wait_change().await
    }
}
