//! Live environments, keyed by name, built from the `environments` table.

use fleetops_core::env::{EnvFactory, EnvKind, EnvStatus, Environment};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{EnvironmentRecord, Store};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

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

    /// Builds every stored environment. Called once at startup.
    pub async fn load(&self, store: &Store) -> Result<()> {
        for rec in store.list_environments().await? {
            self.insert(&rec)?;
        }
        Ok(())
    }

    /// Adds or replaces the live environment for `rec`.
    pub fn insert(&self, rec: &EnvironmentRecord) -> Result<Arc<dyn Environment>> {
        let kind: EnvKind = rec.kind.parse()?;
        let env = self.factory.build(rec.id, &rec.name, kind, &rec.url)?;
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
    pub fn replace(&self, old_name: &str, rec: &EnvironmentRecord) -> Result<Arc<dyn Environment>> {
        let kind: EnvKind = rec.kind.parse()?;
        let env = self.factory.build(rec.id, &rec.name, kind, &rec.url)?;
        let mut envs = self.envs.write().unwrap();
        envs.remove(old_name);
        envs.insert(rec.name.clone(), env.clone());
        Ok(env)
    }

    pub fn remove(&self, name: &str) {
        self.envs.write().unwrap().remove(name);
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
    let probe = registry
        .factory
        .build(0, LOCAL_ENV_NAME, EnvKind::DockerLocal, url)?;
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
            registry.insert(&rec)?;
            Ok(Some(rec))
        }
        EnvStatus::Down { reason } => {
            tracing::warn!(%url, %reason, "no reachable Docker endpoint; add an environment manually");
            Ok(None)
        }
    }
}
