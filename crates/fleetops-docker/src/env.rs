use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion, Environment};
use fleetops_core::{Error, Result};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// A Docker endpoint registered as an environment. The connection is made
/// lazily and retried on each use until it succeeds, so an environment whose
/// socket appears later (Docker restarted) recovers on its own.
pub struct LocalDockerEnv {
    id: i64,
    name: String,
    url: String,
    client: Mutex<Option<Arc<LocalDocker>>>,
}

impl LocalDockerEnv {
    pub fn new(id: i64, name: &str, url: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            url: url.to_string(),
            client: Mutex::new(None),
        }
    }

    fn connect(&self) -> Result<Arc<LocalDocker>> {
        let mut slot = self.client.lock().unwrap();
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let c = Arc::new(LocalDocker::connect(&self.url)?);
        *slot = Some(c.clone());
        Ok(c)
    }
}

#[async_trait]
impl Environment for LocalDockerEnv {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::DockerLocal
    }

    async fn status(&self) -> EnvStatus {
        let client = match self.connect() {
            Ok(c) => c,
            Err(e) => {
                return EnvStatus::Down {
                    reason: e.to_string(),
                }
            }
        };
        match tokio::time::timeout(STATUS_TIMEOUT, client.version()).await {
            Ok(Ok(version)) => EnvStatus::Up {
                version: EnvVersion::Docker(version),
            },
            Ok(Err(e)) => EnvStatus::Down {
                reason: e.to_string(),
            },
            Err(_) => EnvStatus::Down {
                reason: format!("no response from {} within {STATUS_TIMEOUT:?}", self.url),
            },
        }
    }

    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        self.connect().ok().map(|c| c as Arc<dyn DockerOps>)
    }
}

/// Builds local, agent and Swarm environments. Agent environments read their
/// live sessions from `agents`, which the server shares with the connect
/// handler.
#[derive(Default)]
pub struct DockerEnvFactory {
    agents: Arc<fleetops_tunnel::registry::Registry>,
}

impl DockerEnvFactory {
    pub fn new(agents: Arc<fleetops_tunnel::registry::Registry>) -> Self {
        Self { agents }
    }
}

impl EnvFactory for DockerEnvFactory {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        match spec.kind {
            EnvKind::DockerLocal => Ok(Arc::new(LocalDockerEnv::new(
                spec.id, &spec.name, &spec.url,
            ))),
            EnvKind::DockerAgent => Ok(Arc::new(crate::AgentDockerEnv::new(
                spec.id,
                &spec.name,
                self.agents.clone(),
            ))),
            EnvKind::SwarmAgent => Ok(Arc::new(crate::SwarmEnv::new(
                spec.id,
                &spec.name,
                self.agents.clone(),
            ))),
            EnvKind::KubeConfig | EnvKind::KubeAgent | EnvKind::SwarmNode => Err(Error::Invalid(
                format!("{} is not a Docker environment kind", spec.kind.as_str()),
            )),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_socket_is_down_not_an_error() {
        let env = DockerEnvFactory::default()
            .build(&EnvSpec {
                id: 1,
                name: "local".into(),
                kind: EnvKind::DockerLocal,
                url: "unix:///nonexistent/docker.sock".into(),
                credential: None,
            })
            .unwrap();
        assert_eq!(env.name(), "local");
        assert!(env.docker().is_none());
        match env.status().await {
            EnvStatus::Down { reason } => {
                assert!(reason.contains("/nonexistent/docker.sock"), "{reason}")
            }
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refused_tcp_endpoint_is_down() {
        let env = LocalDockerEnv::new(1, "tcp", "tcp://127.0.0.1:1");
        assert!(matches!(env.status().await, EnvStatus::Down { .. }));
    }
}
