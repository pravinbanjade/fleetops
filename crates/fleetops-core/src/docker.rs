//! Docker domain types and operation traits. Implementations (bollard in
//! `fleetops-docker`, fakes in tests) never leak their own types past here.
//! Inspect-style payloads are passed through as raw JSON because the UI shows
//! them verbatim.

use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, str::FromStr};
use utoipa::ToSchema;

pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
pub const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EngineVersion {
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PortMapping {
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub ip: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ContainerSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    /// created | running | paused | restarting | exited | removing | dead
    pub state: String,
    pub status: String,
    pub created: i64,
    pub ports: Vec<PortMapping>,
    pub labels: BTreeMap<String, String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    /// Names of named volumes mounted by this container.
    pub volumes: Vec<String>,
}

fn tcp() -> String {
    "tcp".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PortBinding {
    pub container_port: u16,
    #[serde(default = "tcp")]
    pub protocol: String,
    #[serde(default)]
    pub host_ip: Option<String>,
    /// None publishes on a random host port.
    #[serde(default)]
    pub host_port: Option<u16>,
}

/// Everything the create/recreate form edits. `advanced` is a Docker API
/// `ContainerCreate` JSON object merged over the generated body.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ContainerSpec {
    pub name: Option<String>,
    pub image: String,
    pub cmd: Option<Vec<String>>,
    pub entrypoint: Option<Vec<String>>,
    pub env: Vec<String>,
    pub ports: Vec<PortBinding>,
    /// `source:target[:mode]` bind or named-volume mounts.
    pub binds: Vec<String>,
    pub network: Option<String>,
    /// no | always | unless-stopped | on-failure
    pub restart_policy: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub memory_bytes: Option<i64>,
    pub nano_cpus: Option<i64>,
    pub cap_add: Vec<String>,
    pub privileged: bool,
    #[schema(value_type = Option<Object>)]
    pub advanced: Option<Value>,
}

pub fn validate_container_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && name.len() <= 128;
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "invalid container name '{name}': use letters, digits, '_', '.', '-' and start with a letter or digit"
        )))
    }
}

impl ContainerSpec {
    pub fn validate(&self) -> Result<()> {
        if self.image.trim().is_empty() {
            return Err(Error::Invalid("image is required".into()));
        }
        if let Some(name) = &self.name {
            validate_container_name(name)?;
        }
        for p in &self.ports {
            if !matches!(p.protocol.as_str(), "tcp" | "udp" | "sctp") {
                return Err(Error::Invalid(format!(
                    "invalid port protocol '{}'",
                    p.protocol
                )));
            }
            if p.container_port == 0 {
                return Err(Error::Invalid("container port must be 1-65535".into()));
            }
        }
        if let Some(policy) = &self.restart_policy {
            if !matches!(
                policy.as_str(),
                "no" | "always" | "unless-stopped" | "on-failure"
            ) {
                return Err(Error::Invalid(format!("invalid restart policy '{policy}'")));
            }
        }
        if let Some(adv) = &self.advanced {
            if !adv.is_object() {
                return Err(Error::Invalid("advanced must be a JSON object".into()));
            }
        }
        Ok(())
    }
}

/// Splits `repo[:tag]` into repo and tag, defaulting the tag to `latest`.
/// A colon before the last `/` is a registry port, not a tag. Digest
/// references (`repo@sha256:...`) come back whole with an empty tag.
pub fn split_image_reference(reference: &str) -> Result<(String, String)> {
    let r = reference.trim();
    if r.is_empty() || r.len() > 512 || r.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::Invalid(format!(
            "invalid image reference '{reference}'"
        )));
    }
    if r.contains('@') {
        return Ok((r.to_string(), String::new()));
    }
    let slash = r.rfind('/').map_or(0, |i| i + 1);
    match r[slash..].rfind(':') {
        Some(i) if i > 0 && slash + i + 1 < r.len() => {
            Ok((r[..slash + i].to_string(), r[slash + i + 1..].to_string()))
        }
        Some(_) => Err(Error::Invalid(format!(
            "invalid image reference '{reference}'"
        ))),
        None => Ok((r.to_string(), "latest".into())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Kill,
    Pause,
    Unpause,
}

impl ContainerAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContainerAction::Start => "start",
            ContainerAction::Stop => "stop",
            ContainerAction::Restart => "restart",
            ContainerAction::Kill => "kill",
            ContainerAction::Pause => "pause",
            ContainerAction::Unpause => "unpause",
        }
    }
}

impl FromStr for ContainerAction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "start" => ContainerAction::Start,
            "stop" => ContainerAction::Stop,
            "restart" => ContainerAction::Restart,
            "kill" => ContainerAction::Kill,
            "pause" => ContainerAction::Pause,
            "unpause" => ContainerAction::Unpause,
            other => {
                return Err(Error::NotFound(format!(
                    "unknown container action '{other}'"
                )))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImageSummary {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
    pub in_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VolumeSummary {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub in_use: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct VolumeSpec {
    pub name: String,
    pub driver: Option<String>,
    pub driver_opts: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetworkSummary {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub attachable: bool,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct NetworkSpec {
    pub name: String,
    pub driver: Option<String>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub internal: bool,
    pub attachable: bool,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PruneReport {
    pub deleted: Vec<String>,
    pub space_reclaimed: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mode: u32,
    pub modified: i64,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<DirEntry>,
    /// True when the listing hit the size/entry cap and is incomplete.
    pub truncated: bool,
}

#[async_trait]
pub trait SystemOps: Send + Sync {
    async fn version(&self) -> Result<EngineVersion>;
    async fn info(&self) -> Result<Value>;
    async fn disk_usage(&self) -> Result<Value>;
}

#[async_trait]
pub trait ContainerOps: Send + Sync {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>>;
    async fn inspect_container(&self, id: &str) -> Result<Value>;
    /// The create-form view of an existing container.
    async fn container_spec(&self, id: &str) -> Result<ContainerSpec>;
    /// Creates (does not start) a container; returns its id.
    async fn create_container(&self, spec: &ContainerSpec) -> Result<String>;
    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()>;
    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()>;
    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> Result<()>;
    async fn prune_containers(&self) -> Result<PruneReport>;
    async fn list_dir(&self, id: &str, path: &str) -> Result<DirListing>;
    async fn read_file(&self, id: &str, path: &str, max_bytes: u64) -> Result<Vec<u8>>;
}

#[async_trait]
pub trait ImageOps: Send + Sync {
    async fn list_images(&self) -> Result<Vec<ImageSummary>>;
    async fn inspect_image(&self, id: &str) -> Result<Value>;
    async fn image_history(&self, id: &str) -> Result<Value>;
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()>;
    /// Returns the untagged/deleted references.
    async fn remove_image(&self, id: &str, force: bool) -> Result<Vec<String>>;
    /// `all = false` prunes only dangling images.
    async fn prune_images(&self, all: bool) -> Result<PruneReport>;
}

#[async_trait]
pub trait VolumeOps: Send + Sync {
    async fn list_volumes(&self) -> Result<Vec<VolumeSummary>>;
    async fn inspect_volume(&self, name: &str) -> Result<Value>;
    async fn create_volume(&self, spec: &VolumeSpec) -> Result<VolumeSummary>;
    async fn remove_volume(&self, name: &str, force: bool) -> Result<()>;
    async fn prune_volumes(&self) -> Result<PruneReport>;
}

#[async_trait]
pub trait NetworkOps: Send + Sync {
    async fn list_networks(&self) -> Result<Vec<NetworkSummary>>;
    async fn inspect_network(&self, id: &str) -> Result<Value>;
    /// Returns the new network id.
    async fn create_network(&self, spec: &NetworkSpec) -> Result<String>;
    async fn remove_network(&self, id: &str) -> Result<()>;
    async fn connect_network(&self, network: &str, container: &str) -> Result<()>;
    async fn disconnect_network(&self, network: &str, container: &str, force: bool) -> Result<()>;
    async fn prune_networks(&self) -> Result<PruneReport>;
}

/// Everything a Docker-kind environment can do.
pub trait DockerOps:
    SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps + crate::streams::StreamOps
{
}

impl<T> DockerOps for T where
    T: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps + crate::streams::StreamOps
{
}
