//! Live environments, keyed by name, built from the `environments` table.

use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus, Environment};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{EnvironmentRecord, Store};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

/// How long a cluster gets to report its node count.
const NODE_COUNT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct EnvRegistry {
    factory: Arc<dyn EnvFactory>,
    envs: RwLock<HashMap<String, Arc<dyn Environment>>>,
    /// Serialises create/update/delete so the database write and the
    /// registry change happen as one step.
    mutations: tokio::sync::Mutex<()>,
}

impl EnvRegistry {
    pub fn new(factory: Arc<dyn EnvFactory>) -> Self {
        Self {
            factory,
            envs: RwLock::new(HashMap::new()),
            mutations: tokio::sync::Mutex::new(()),
        }
    }

    /// Builds every stored environment, each with its credential. Called
    /// once at startup.
    pub async fn load(&self, store: &Store) -> Result<()> {
        for rec in store.list_environments().await? {
            let credential = store.env_credential(rec.id).await?;
            self.insert(&rec, credential)?;
        }
        Ok(())
    }

    fn spec(rec: &EnvironmentRecord, credential: Option<Credential>) -> Result<EnvSpec> {
        Ok(EnvSpec {
            id: rec.id,
            name: rec.name.clone(),
            kind: rec.kind.parse()?,
            url: rec.url.clone(),
            credential,
        })
    }

    /// Adds or replaces the live environment for `rec`.
    pub fn insert(
        &self,
        rec: &EnvironmentRecord,
        credential: Option<Credential>,
    ) -> Result<Arc<dyn Environment>> {
        let env = self.factory.build(&Self::spec(rec, credential)?)?;
        self.envs
            .write()
            .unwrap()
            .insert(rec.name.clone(), env.clone());
        Ok(env)
    }

    /// Held by every environment mutation, across its database write and
    /// registry change.
    pub async fn lock_mutations(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mutations.lock().await
    }

    /// Swaps the entry `old_name` for `rec` in one step, so requests never
    /// see a moment with neither entry.
    pub fn replace(
        &self,
        old_name: &str,
        rec: &EnvironmentRecord,
        credential: Option<Credential>,
    ) -> Result<Arc<dyn Environment>> {
        let env = self.factory.build(&Self::spec(rec, credential)?)?;
        let mut envs = self.envs.write().unwrap();
        envs.remove(old_name);
        envs.insert(rec.name.clone(), env.clone());
        Ok(env)
    }

    pub fn remove(&self, name: &str) {
        self.envs.write().unwrap().remove(name);
    }

    /// The nodes the license counts: each Docker or Podman host is one, and
    /// each Kubernetes cluster or Docker Swarm is its node count. A cluster
    /// or swarm that cannot say (down, or not allowed to list nodes) counts
    /// as one.
    pub async fn count_nodes(&self) -> u64 {
        let envs: Vec<_> = self.envs.read().unwrap().values().cloned().collect();
        let counts = envs.into_iter().map(|env| async move {
            if let Some(swarm) = env.swarm() {
                return match tokio::time::timeout(NODE_COUNT_TIMEOUT, swarm.nodes()).await {
                    Ok(Ok(nodes)) => (nodes.len() as u64).max(1),
                    _ => 1,
                };
            }
            let Some(kube) = env.kube() else { return 1 };
            match tokio::time::timeout(NODE_COUNT_TIMEOUT, kube.version()).await {
                Ok(Ok(v)) => u64::from(v.nodes.unwrap_or(1).max(1)),
                _ => 1,
            }
        });
        futures_util::future::join_all(counts)
            .await
            .into_iter()
            .sum()
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn Environment>> {
        self.envs
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("environment '{name}'")))
    }
}

pub const LOCAL_ENV_NAME: &str = "local";

/// On first start (no environments stored), registers `url` as "local" if
/// it answers. Returns the new record, or None when nothing was registered.
pub async fn bootstrap_local(
    store: &Store,
    registry: &EnvRegistry,
    url: &str,
) -> Result<Option<EnvironmentRecord>> {
    let _guard = registry.lock_mutations().await;
    if !store.list_environments().await?.is_empty() {
        return Ok(None);
    }
    let probe = registry.factory.build(&EnvSpec {
        id: 0,
        name: LOCAL_ENV_NAME.into(),
        kind: EnvKind::DockerLocal,
        url: url.into(),
        credential: None,
    })?;
    match probe.status().await {
        EnvStatus::Up { .. } => {
            let rec = store
                .create_environment(
                    LOCAL_ENV_NAME,
                    EnvKind::DockerLocal.as_str(),
                    url,
                    now_unix(),
                )
                .await?;
            registry.insert(&rec, None)?;
            Ok(Some(rec))
        }
        EnvStatus::Down { reason } => {
            tracing::warn!(%url, %reason, "no reachable Docker endpoint; add an environment manually");
            Ok(None)
        }
    }
}
