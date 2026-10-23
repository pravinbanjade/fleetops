//! The server's environment factory: Docker and Swarm kinds from fleetops-docker,
//! Kubernetes kinds from fleetops-kube, one shared agent registry.

use fleetops_core::env::{EnvFactory, EnvSpec, Environment};
use fleetops_core::Result;
use fleetops_docker::DockerEnvFactory;
use fleetops_kube::env::KubeEnvFactory;
use fleetops_tunnel::registry::Registry;
use std::sync::Arc;

pub struct Factory {
    docker: DockerEnvFactory,
    kube: KubeEnvFactory,
}

impl Factory {
    pub fn new(agents: Arc<Registry>, exec: fleetops_kube::exec_auth::ExecPolicy) -> Self {
        Self {
            docker: DockerEnvFactory::new(agents.clone()),
            kube: KubeEnvFactory::new(agents).with_exec_policy(exec),
        }
    }
}

impl EnvFactory for Factory {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        // Swarm environments are Docker engines too: fleetops-docker builds them.
        if spec.kind.is_kube() {
            self.kube.build(spec)
        } else {
            self.docker.build(spec)
        }
    }
}
