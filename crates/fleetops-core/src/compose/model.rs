//! The normalised project: every service, network and volume of a Compose
//! file with defaults filled in. Maps are ordered, so serialising a value
//! is canonical (the config hashes rely on that).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub services: BTreeMap<String, Service>,
    pub networks: BTreeMap<String, Network>,
    pub volumes: BTreeMap<String, Volume>,
    /// Top-level `secrets:` and `configs:`, by Compose key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, FileSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub configs: BTreeMap<String, FileSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub image: String,
    pub container_name: Option<String>,
    pub command: Option<Vec<String>>,
    pub entrypoint: Option<Vec<String>>,
    pub environment: BTreeMap<String, String>,
    pub ports: Vec<Port>,
    pub mounts: Vec<Mount>,
    /// Keyed by the Compose network key (not the Docker name).
    pub networks: BTreeMap<String, ServiceNetwork>,
    pub network_mode: Option<String>,
    pub depends_on: BTreeMap<String, DependsCondition>,
    /// no | always | unless-stopped | on-failure[:N]
    pub restart: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub healthcheck: Option<Healthcheck>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub hostname: Option<String>,
    pub extra_hosts: Vec<String>,
    pub dns: Vec<String>,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub tmpfs: Vec<String>,
    pub devices: Vec<String>,
    pub privileged: bool,
    pub read_only: bool,
    pub init: Option<bool>,
    pub shm_size: Option<i64>,
    pub stop_signal: Option<String>,
    pub stop_grace_period_secs: Option<i64>,
    pub logging: Option<Logging>,
    pub memory_bytes: Option<i64>,
    pub nano_cpus: Option<i64>,
    pub pull_policy: PullPolicy,
    pub replicas: u32,
    /// Secrets and configs copied into the container before it starts.
    /// Skipped when empty, so a service without any hashes as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileMount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Secret,
    Config,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Secret => "secret",
            FileKind::Config => "config",
        }
    }
}

/// Where a top-level secret or config gets its content. `file:` is only a
/// name here: what it means (a Secrets-tab value, or a repo file) depends
/// on the stack, and is decided when values are resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FileSource {
    File {
        path: String,
    },
    Environment {
        var: String,
    },
    /// Configs only; already interpolated.
    Content {
        text: String,
    },
}

/// One secret or config attached to a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMount {
    pub kind: FileKind,
    /// The top-level key.
    pub source: String,
    /// Absolute path in the container.
    pub target: String,
    pub uid: String,
    pub gid: String,
    pub mode: u32,
    /// Keyed digest of the content, filled in when values are resolved, so
    /// the config hash changes with the value. Empty after parsing.
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependsCondition {
    Started,
    Healthy,
    CompletedSuccessfully,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    Always,
    #[default]
    Missing,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub target: u16,
    pub published: Option<u16>,
    pub host_ip: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Mount {
    /// `source` is the Compose volume key; empty for an anonymous volume.
    Volume {
        source: String,
        target: String,
        read_only: bool,
    },
    Bind {
        source: String,
        target: String,
        read_only: bool,
    },
    Tmpfs {
        target: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceNetwork {
    pub aliases: Vec<String>,
    pub ipv4_address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Healthcheck {
    pub test: Vec<String>,
    pub interval_secs: i64,
    pub timeout_secs: i64,
    pub retries: u32,
    pub start_period_secs: i64,
    pub disable: bool,
}

impl Default for Healthcheck {
    fn default() -> Self {
        Self {
            test: vec![],
            interval_secs: 30,
            timeout_secs: 30,
            retries: 3,
            start_period_secs: 0,
            disable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logging {
    pub driver: String,
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub external: bool,
    /// The Docker network name.
    pub name: String,
    pub driver: Option<String>,
    pub driver_opts: BTreeMap<String, String>,
    pub internal: bool,
    pub attachable: bool,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub external: bool,
    /// The Docker volume name.
    pub name: String,
    pub driver: Option<String>,
    pub driver_opts: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}
