//! bollard-backed implementation of fleetops's Docker traits.

mod containers;
pub mod convert;
mod env;
mod error;
pub mod files;
mod resources;

pub use env::{DockerEnvFactory, LocalDockerEnv};

use bollard::Docker;
use fleetops_core::{Error, Result};

/// A connection to one Docker (or Podman) API endpoint.
pub struct LocalDocker {
    client: Docker,
}

impl LocalDocker {
    /// Accepts `unix://`, `tcp://` and `http://` URLs. Fails immediately if a
    /// unix socket path does not exist; other endpoints fail on first use.
    pub fn connect(url: &str) -> Result<Self> {
        let client = Docker::connect_with_host(url)
            .map_err(|e| Error::EnvUnavailable(format!("cannot connect to {url}: {e}")))?;
        Ok(Self { client })
    }
}
