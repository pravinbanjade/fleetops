//! A remote Docker host behind an agent tunnel. Every request goes through
//! the environment's live session in the registry; with none, the
//! environment is down and `docker()` is `None` (sub-project 2 spec §2.4).

use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvKind, EnvStatus, Environment};
use fleetops_tunnel::registry::Registry;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

pub struct AgentDockerEnv {
    id: i64,
    name: String,
    agents: Arc<Registry>,
    /// The client for the current session, keyed by session id.
    client: Mutex<Option<(u64, Arc<LocalDocker>)>>,
}

impl AgentDockerEnv {
    pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self {
        Self {
            id,
            name: name.to_string(),
            agents,
            client: Mutex::new(None),
        }
    }

    pub(crate) fn client(&self) -> Option<Arc<LocalDocker>> {
        let live = self.agents.get(self.id)?;
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
impl Environment for AgentDockerEnv {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::DockerAgent
    }

    async fn status(&self) -> EnvStatus {
        let Some(client) = self.client() else {
            let reason = self
                .agents
                .offline_reason(self.id)
                .map(|r| format!("agent offline: {r}"))
                .unwrap_or_else(|| "waiting for the agent to connect".into());
            return EnvStatus::Down { reason };
        };
        match tokio::time::timeout(STATUS_TIMEOUT, client.version()).await {
            Ok(Ok(version)) => EnvStatus::Up { version },
            Ok(Err(e)) => EnvStatus::Down {
                reason: format!("agent connected, Docker unreachable: {e}"),
            },
            Err(_) => EnvStatus::Down {
                reason: format!("no response through the agent within {STATUS_TIMEOUT:?}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::env::{EnvStatus, Environment};
    use fleetops_tunnel::{control::HostInfo, registry::AgentInfo, testing::loopback, Target};
    use tokio::sync::mpsc;

    fn info() -> AgentInfo {
        AgentInfo {
            agent_version: "0.2.0".into(),
            host: HostInfo::default(),
            connected_at: 0,
        }
    }

    #[tokio::test]
    async fn without_a_session_it_is_down_and_has_no_docker() {
        let agents = Arc::new(Registry::default());
        let env = AgentDockerEnv::new(5, "web-01", agents.clone());
        assert!(env.docker().is_none());
        match env.status().await {
            EnvStatus::Down { reason } => assert_eq!(reason, "waiting for the agent to connect"),
            other => panic!("{other:?}"),
        }
        agents.set_offline_reason(5, "agent v0.9 uses protocol 2; server needs 1");
        match env.status().await {
            EnvStatus::Down { reason } => assert!(reason.contains("protocol 2"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_live_session_gives_a_client_rebuilt_per_session() {
        let agents = Arc::new(Registry::default());
        let env = AgentDockerEnv::new(5, "web-01", agents.clone());
        let (bye, _rx) = mpsc::channel(1);
        agents.insert(
            5,
            loopback(Target::Unix("/nonexistent.sock".into())),
            info(),
            bye,
        );
        let a = env.client().expect("live");
        let again = env.client().expect("live");
        assert!(Arc::ptr_eq(&a, &again), "same session, same client");
        let (bye, _rx) = mpsc::channel(1);
        agents.insert(
            5,
            loopback(Target::Unix("/nonexistent.sock".into())),
            info(),
            bye,
        );
        let b = env.client().expect("live");
        assert!(!Arc::ptr_eq(&a, &b), "a new session gets a new client");
        // Reachable tunnel, unreachable daemon: down with the daemon's error.
        assert!(matches!(env.status().await, EnvStatus::Down { .. }));
    }

    #[tokio::test]
    async fn wait_change_follows_the_registry() {
        let agents = Arc::new(Registry::default());
        let env = Arc::new(AgentDockerEnv::new(5, "web-01", agents.clone()));
        let w = {
            let env = env.clone();
            tokio::spawn(async move { env.wait_change().await })
        };
        tokio::task::yield_now().await;
        agents.set_offline_reason(5, "x");
        tokio::time::timeout(std::time::Duration::from_secs(1), w)
            .await
            .unwrap()
            .unwrap();
    }
}
