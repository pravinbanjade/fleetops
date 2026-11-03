//! Running one Trivy scan wherever the image lives.

use super::config::Effective;
use super::model::ScanTarget;
use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::{Error, Result};
use std::path::PathBuf;
use std::sync::Arc;

#[async_trait]
pub trait Runner: Send + Sync {
    /// Pulls the Trivy image and creates the database cache. An error means
    /// the scanner is unavailable on this environment (or Swarm node).
    async fn prepare(&self, env: &Arc<dyn Environment>, node: &str, cfg: &Effective) -> Result<()>;
    /// Scans one image; the raw Trivy JSON report.
    async fn scan(
        &self,
        env: &Arc<dyn Environment>,
        t: &ScanTarget,
        cfg: &Effective,
        scan_id: i64,
    ) -> Result<Vec<u8>>;
    /// Removes scan containers or Jobs a crash left behind.
    async fn cleanup_orphans(&self, env: &Arc<dyn Environment>) -> Result<()>;
}

/// Docker hosts and Swarm nodes run a container; clusters run a Job.
pub struct LiveRunner {
    /// Where an exported image is spooled for the export fallback.
    pub tmp_dir: PathBuf,
}

/// The Docker connection a scan on `node` (empty for a plain host) uses.
pub fn docker_for(env: &Arc<dyn Environment>, node: &str) -> Result<Arc<dyn DockerOps>> {
    let d = if node.is_empty() {
        env.docker()
    } else {
        env.node(node)
    };
    d.ok_or_else(|| Error::EnvUnavailable(format!("environment '{}' is unreachable", env.name())))
}

fn kube_for(env: &Arc<dyn Environment>) -> Result<Arc<dyn fleetops_core::kube::KubeOps>> {
    env.kube().ok_or_else(|| {
        Error::EnvUnavailable(format!("environment '{}' is unreachable", env.name()))
    })
}

#[async_trait]
impl Runner for LiveRunner {
    async fn prepare(&self, env: &Arc<dyn Environment>, node: &str, cfg: &Effective) -> Result<()> {
        if env.kind().is_kube() {
            return super::kube::prepare(kube_for(env)?.as_ref(), cfg).await;
        }
        super::docker::prepare(docker_for(env, node)?.as_ref(), cfg).await
    }

    async fn scan(
        &self,
        env: &Arc<dyn Environment>,
        t: &ScanTarget,
        cfg: &Effective,
        scan_id: i64,
    ) -> Result<Vec<u8>> {
        if env.kind().is_kube() {
            return super::kube::scan(kube_for(env)?.as_ref(), t, cfg, scan_id).await;
        }
        let d = docker_for(env, &t.node)?;
        super::docker::scan(d.as_ref(), t, cfg, scan_id, &self.tmp_dir).await
    }

    async fn cleanup_orphans(&self, env: &Arc<dyn Environment>) -> Result<()> {
        if env.kind().is_kube() {
            return super::kube::cleanup_orphans(kube_for(env)?.as_ref()).await;
        }
        if env.kind().is_swarm() {
            let Some(swarm) = env.swarm() else {
                return Ok(());
            };
            for n in swarm.nodes().await? {
                if let Some(d) = env.node(&n.id) {
                    super::docker::cleanup_orphans(d.as_ref()).await?;
                }
            }
            return Ok(());
        }
        super::docker::cleanup_orphans(docker_for(env, "")?.as_ref()).await
    }
}
