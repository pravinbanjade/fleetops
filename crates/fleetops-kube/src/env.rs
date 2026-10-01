//! The two Kubernetes environment kinds (spec §2.4).

use crate::{kubeconfig, tunnel, KubeClient};
use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::{
    Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion, Environment,
};
use fleetops_core::kube::KubeOps;
use fleetops_core::{Error, Result};
use fleetops_tunnel::registry::Registry;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// How long `status()` waits for the cluster.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const UNREADABLE: &str = "credential cannot be decrypted; re-enter the kubeconfig";

async fn kube_status(ops: &dyn KubeOps, unreachable: &str) -> EnvStatus {
    match tokio::time::timeout(STATUS_TIMEOUT, ops.version()).await {
        Ok(Ok(v)) => EnvStatus::Up {
            version: EnvVersion::Kube(v),
        },
        Ok(Err(e)) => EnvStatus::Down {
            reason: format!("{unreachable}: {e}"),
        },
        Err(_) => EnvStatus::Down {
            reason: format!(
                "{unreachable}: no answer within {}s",
                STATUS_TIMEOUT.as_secs()
            ),
        },
    }
}

/// A cluster reached directly with its stored kubeconfig. The client is
/// built up front (no I/O), so a dead cluster costs nothing until used.
pub struct KubeConfigEnv {
    id: i64,
    name: String,
    ops: std::result::Result<Arc<KubeClient>, String>,
}

impl KubeConfigEnv {
    pub fn new(id: i64, name: &str, credential: Option<&Credential>) -> Self {
        let ops = match credential {
            None => Err("no kubeconfig stored".to_string()),
            Some(Credential::Unreadable(_)) => Err(UNREADABLE.to_string()),
            Some(Credential::Kubeconfig(yaml)) => kubeconfig::config(yaml)
                .and_then(|c| kube::Client::try_from(c).map_err(|e| Error::Invalid(e.to_string())))
                .map(|c| Arc::new(KubeClient::new(c)))
                .map_err(|e| format!("cannot reach cluster: {e}")),
        };
        Self {
            id,
            name: name.to_string(),
            ops,
        }
    }
}

#[async_trait]
impl Environment for KubeConfigEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        EnvKind::KubeConfig
    }
    async fn status(&self) -> EnvStatus {
        match &self.ops {
            Err(reason) => EnvStatus::Down {
                reason: reason.clone(),
            },
            Ok(ops) => kube_status(ops.as_ref(), "cannot reach cluster").await,
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        self.ops
            .as_ref()
            .ok()
            .map(|c| c.clone() as Arc<dyn KubeOps>)
    }
}

/// A cluster behind an in-cluster agent: a client per live session.
pub struct KubeAgentEnv {
    id: i64,
    name: String,
    agents: Arc<Registry>,
    client: Mutex<Option<(u64, Arc<KubeClient>)>>,
}

impl KubeAgentEnv {
    pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self {
        Self {
            id,
            name: name.to_string(),
            agents,
            client: Mutex::new(None),
        }
    }

    fn client(&self) -> Option<Arc<KubeClient>> {
        let live = self.agents.get(self.id)?;
        let mut slot = self.client.lock().unwrap();
        if let Some((id, c)) = slot.as_ref() {
            if *id == live.id {
                return Some(c.clone());
            }
        }
        let c = Arc::new(KubeClient::new(tunnel::client(Arc::new(live.session))));
        *slot = Some((live.id, c.clone()));
        Some(c)
    }
}

#[async_trait]
impl Environment for KubeAgentEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        EnvKind::KubeAgent
    }
    async fn status(&self) -> EnvStatus {
        match self.client() {
            Some(c) => kube_status(c.as_ref(), "agent connected, cluster unreachable").await,
            None => EnvStatus::Down {
                reason: match self.agents.offline_reason(self.id) {
                    Some(r) => format!("agent offline: {r}"),
                    None => "waiting for the agent to connect".into(),
                },
            },
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        self.client().map(|c| c as Arc<dyn KubeOps>)
    }
    async fn wait_change(&self) {
        self.agents.wait_change().await
    }
}

/// Builds the Kubernetes kinds; the server combines it with Docker's.
pub struct KubeEnvFactory {
    agents: Arc<Registry>,
}

impl KubeEnvFactory {
    pub fn new(agents: Arc<Registry>) -> Self {
        Self { agents }
    }
}

impl EnvFactory for KubeEnvFactory {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        match spec.kind {
            EnvKind::KubeConfig => Ok(Arc::new(KubeConfigEnv::new(
                spec.id,
                &spec.name,
                spec.credential.as_ref(),
            ))),
            EnvKind::KubeAgent => Ok(Arc::new(KubeAgentEnv::new(
                spec.id,
                &spec.name,
                self.agents.clone(),
            ))),
            other => Err(Error::Invalid(format!(
                "{} is not a Kubernetes environment kind",
                other.as_str()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus};

    fn spec(kind: EnvKind, credential: Option<Credential>) -> EnvSpec {
        EnvSpec {
            id: 1,
            name: "k".into(),
            kind,
            url: "https://127.0.0.1:1".into(),
            credential,
        }
    }

    const UNREACHABLE: &str = "apiVersion: v1\nkind: Config\ncurrent-context: c\nclusters:\n- name: c\n  cluster: {server: 'https://127.0.0.1:1'}\ncontexts:\n- name: c\n  context: {cluster: c, user: u}\nusers:\n- name: u\n  user: {token: t}\n";

    #[tokio::test]
    async fn an_unreachable_cluster_is_down_without_blocking() {
        let f = KubeEnvFactory::new(Default::default());
        let started = std::time::Instant::now();
        let env = f
            .build(&spec(
                EnvKind::KubeConfig,
                Some(Credential::Kubeconfig(UNREACHABLE.into())),
            ))
            .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "build must not connect"
        );
        assert!(env.kube().is_some());
        assert!(env.docker().is_none());
        match tokio::time::timeout(STATUS_TIMEOUT * 2, env.status())
            .await
            .unwrap()
        {
            EnvStatus::Down { reason } => {
                assert!(reason.contains("cannot reach cluster"), "{reason}")
            }
            up => panic!("expected down, got {up:?}"),
        }
    }

    #[tokio::test]
    async fn an_unreadable_credential_says_to_re_enter_it() {
        let f = KubeEnvFactory::new(Default::default());
        let env = f
            .build(&spec(
                EnvKind::KubeConfig,
                Some(Credential::Unreadable("x".into())),
            ))
            .unwrap();
        assert!(env.kube().is_none());
        assert_eq!(
            env.status().await,
            EnvStatus::Down {
                reason: "credential cannot be decrypted; re-enter the kubeconfig".into()
            }
        );
    }

    #[tokio::test]
    async fn an_agent_cluster_waits_for_its_agent() {
        let f = KubeEnvFactory::new(Default::default());
        let env = f.build(&spec(EnvKind::KubeAgent, None)).unwrap();
        assert_eq!(env.kind(), EnvKind::KubeAgent);
        assert!(env.kube().is_none());
        assert!(
            matches!(env.status().await, EnvStatus::Down { reason } if reason.contains("waiting for the agent"))
        );
    }

    #[test]
    fn docker_kinds_are_not_built_here() {
        let f = KubeEnvFactory::new(Default::default());
        assert!(f.build(&spec(EnvKind::DockerLocal, None)).is_err());
    }
}
