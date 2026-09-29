use crate::LocalDocker;
use async_trait::async_trait;
use dockerer_core::docker::{DockerOps, SystemOps};
use dockerer_core::env::{EnvFactory, EnvKind, EnvStatus, Environment};
use dockerer_core::Result;
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
            Ok(Ok(version)) => EnvStatus::Up { version },
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

pub struct DockerEnvFactory;

impl EnvFactory for DockerEnvFactory {
    fn build(&self, id: i64, name: &str, kind: EnvKind, url: &str) -> Result<Arc<dyn Environment>> {
        match kind {
            EnvKind::DockerLocal => Ok(Arc::new(LocalDockerEnv::new(id, name, url))),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_socket_is_down_not_an_error() {
        let env = DockerEnvFactory
            .build(
                1,
                "local",
                EnvKind::DockerLocal,
                "unix:///nonexistent/docker.sock",
            )
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
