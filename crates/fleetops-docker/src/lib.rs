//! bollard-backed implementation of fleetops's Docker traits.

mod agent_env;
mod containers;
pub mod convert;
mod env;
mod error;
pub mod files;
mod node_env;
mod raw;
mod resources;
mod streams;
mod swarm;
mod swarm_env;
mod tunnel;

pub use agent_env::AgentDockerEnv;
pub use env::{DockerEnvFactory, LocalDockerEnv};
pub use node_env::NodeEnv;
pub use swarm_env::SwarmEnv;

use bollard::Docker;
use fleetops_core::{Error, Result};

/// A connection to one Docker (or Podman) API endpoint.
pub struct LocalDocker {
    client: Docker,
    /// Raw requests where bollard's model loses data; `None` over TCP,
    /// where those fall back to bollard.
    raw: Option<raw::Raw>,
}

impl LocalDocker {
    /// Accepts `unix://`, `tcp://` and `http://` URLs. Fails immediately if a
    /// unix socket path does not exist; other endpoints fail on first use.
    pub fn connect(url: &str) -> Result<Self> {
        let client = Docker::connect_with_host(url)
            .map_err(|e| Error::EnvUnavailable(format!("cannot connect to {url}: {e}")))?;
        let raw = url
            .strip_prefix("unix://")
            .map(|p| raw::unix(std::path::PathBuf::from(p)));
        Ok(Self { client, raw })
    }
}
