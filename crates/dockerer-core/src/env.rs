//! Environments are the things dockerer manages. Sub-project 1 has one kind
//! (a local Docker socket); agents and Kubernetes add kinds later.

use crate::{docker::DockerOps, docker::EngineVersion, Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvKind {
    DockerLocal,
}

impl EnvKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvKind::DockerLocal => "docker-local",
        }
    }
}

impl FromStr for EnvKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "docker-local" => Ok(EnvKind::DockerLocal),
            other => Err(Error::Invalid(format!(
                "unknown environment kind '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum EnvStatus {
    Up { version: EngineVersion },
    Down { reason: String },
}

#[async_trait]
pub trait Environment: Send + Sync {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
    fn kind(&self) -> EnvKind;
    async fn status(&self) -> EnvStatus;
    /// Docker operations, when this environment is Docker-kind and its
    /// endpoint can be reached. `None` means "unavailable right now".
    fn docker(&self) -> Option<Arc<dyn DockerOps>>;
}

/// Builds live environments from stored records. The server supplies the
/// bollard-backed factory; tests supply fakes.
pub trait EnvFactory: Send + Sync {
    fn build(&self, id: i64, name: &str, kind: EnvKind, url: &str) -> Result<Arc<dyn Environment>>;
}

/// Environment names appear in URLs (`/envs/{name}/...`).
pub fn validate_env_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "environment name must be 1-64 letters, digits, '_', '-' or '.'".into(),
        ))
    }
}

pub fn validate_env_url(url: &str) -> Result<()> {
    let rest = ["unix://", "tcp://", "http://", "https://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme));
    match rest {
        Some(r) if !r.is_empty() => Ok(()),
        _ => Err(Error::Invalid(
            "environment URL must be unix://, tcp://, http:// or https://".into(),
        )),
    }
}
