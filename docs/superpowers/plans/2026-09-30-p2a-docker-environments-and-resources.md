# FleetOps P2a — Docker Environments & REST Resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an authenticated user manage a real Docker (or Podman) daemon through `/api/v1/envs/{env}/...`. That covers containers (create, lifecycle, recreate/duplicate, file browsing), images, volumes, networks and read-only Compose projects. Environments are stored in SQLite, "local" is auto-registered on first start, every mutation is authorized and audited, and Docker's own errors are passed through.

**Architecture:**
- `fleetops-core` gains Docker domain types and five small operation traits (`SystemOps`, `ContainerOps`, `ImageOps`, `VolumeOps`, `NetworkOps`, bundled as `DockerOps`), plus the `Environment`/`EnvFactory` traits and the recreate-with-rollback algorithm.
- A new `fleetops-docker` crate implements them with bollard 0.21. Conversions go through Docker-API JSON, so the create form's "advanced" JSON override is deep-merged before deserialising into bollard's types.
- `fleetops-api` gets an `EnvRegistry` (name → live `Environment`), a single `authorize()` decision point, and one handler module per resource. Handlers only ever see `Arc<dyn DockerOps>`.
- API tests use an in-memory `FakeDocker`. `fleetops-docker` has integration tests against the real daemon behind a `docker-it` feature.

**Tech Stack:** Rust ≥1.86 (trait upcasting), bollard 0.21, async-trait 0.1, tar 0.4, futures-util 0.3, plus P1's axum 0.8 / sqlx 0.8 / tokio.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`

**Plan series for sub-project 1:**
- P1 (done): backend foundation.
- **P2a (this plan):** environments and REST resources.
- **P2b:** websocket hub (events/stats/logs/exec/task), image pull/build background tasks, and utoipa OpenAPI for every handler.
- P3: web UI, e2e tests, image build, CI.

## Global Constraints

- Handlers never import bollard. They use `fleetops_core::docker` traits through `EnvCtx.ops: Arc<dyn DockerOps>` (spec §2.2).
- Error mapping (spec §6):
  - Docker `DockerResponseServerError{status_code,message}` becomes `Error::Upstream{status,message}`, and the HTTP status and message are passed through verbatim.
  - bollard JSON errors become `Internal`.
  - Every other bollard error (transport, socket, timeout) becomes `EnvUnavailable`, which is 503 `env_unavailable`.
- Every Docker mutation writes an `audit_log` row with `environment_id` set. Results are `success`, or `failure` with `detail = {"error": "<message>"}`.
- `authorize(user, env_id, action)` in `fleetops-api::authz` is the only authorization decision.
  - Admins may do everything.
  - `user` may do everything except `EnvManage` (create/update/delete environments).
- Environment names follow `[A-Za-z0-9_.-]{1,64}` and are the `{env}` path segment. Environment URLs must be `unix://`, `tcp://`, `http://` or `https://`.
- First start with no stored environments: if `--docker-host`/`DOCKER_HOST` answers, it is registered as `local`. Otherwise a warning is logged and nothing is registered.
- File browsing caps:
  - A directory listing downloads at most 64 MiB (`LIST_MAX_BYTES`) and returns at most 5,000 entries (`LIST_MAX_ENTRIES`), setting `truncated: true` beyond either.
  - A file download is at most 100 MiB (`MAX_DOWNLOAD_BYTES`).
  - Paths must be absolute.
- Environment status checks time out after 5 s and report `down`, never hang.
- All P1 constraints (sessions, CSRF on every non-GET, JSON error body, request ids) still apply to the new routes.
- **Spec deltas (intentional):**
  - `Environment::docker()` returns `Option<Arc<dyn DockerOps>>`, not `Option<&dyn DockerOps>`, so the lazy reconnecting implementation can hand out owned handles.
  - Environment mutations are addressed by name (`PATCH/DELETE /envs/{env}`), because axum cannot mix `{id}` and `{env}` at the same path position.
  - Image prune takes `?all=true` rather than a body.
  - `GET /containers/{id}/spec` is added so the UI can pre-fill the recreate form.
  - Image pull/build (`202 {task_id}`) moves to P2b together with the websocket `task` topic.

## Review Focus

1. **The daemon disappears mid-session** (socket gone, Docker stopped). Every resource route returns 503 `env_unavailable` and `/status` reports `down`. There are no 500s, and the environment recovers without a restart once the socket is back. Pinned in Task 6 (`missing_socket_is_down_not_an_error`, `transport_errors_mean_unavailable`) and Task 8 (`status_reports_up_and_down`).
2. **Recreate where the replacement fails to start.** The replacement is removed, and the original gets its name back and is restarted if it was running. Pinned in Task 2 (`start_failure_rolls_back_and_restarts_old`, `rename_failure_restarts_old`).
3. **File browser on `/` or a huge directory.** The download is bounded, a partial listing comes back with `truncated: true`, and the server doesn't buffer gigabytes. Pinned in Task 5 (`truncated_stream_returns_partial_listing`, `entry_cap_marks_truncated`, `lists_root_directory`).
4. **File names with quotes or control characters** don't break the `Content-Disposition` header. Pinned in Task 9 (`files_list_and_download` requests `ho"sts` and expects `filename="hosts"`).
5. **Renaming or deleting an environment while clients still use the old name.** Old-name routes return 404 immediately, and the registry and database never disagree. Pinned in Task 8 (`admin_manages_environments`).

---

## File Structure

```
Cargo.toml                                        (modify) rust-version 1.86; async-trait, bollard, futures-util, tar
Makefile                                          (modify) test-docker target; dev-server uses the docker context
crates/fleetops-core/
  Cargo.toml                                      (modify) + async-trait, serde_json (normal); tokio (dev)
  src/lib.rs                                      (modify) + docker, env, recreate modules
  src/docker.rs                                   domain types + SystemOps/ContainerOps/ImageOps/VolumeOps/NetworkOps/DockerOps
  src/env.rs                                      EnvKind, EnvStatus, Environment, EnvFactory, name/url validation
  src/recreate.rs                                 recreate-with-rollback over &dyn ContainerOps
  tests/docker_types.rs  tests/recreate.rs
crates/fleetops-store/
  src/environments.rs                             EnvironmentRecord + CRUD
  src/lib.rs                                      (modify) module + re-export
  tests/environments.rs
crates/fleetops-docker/                           NEW
  Cargo.toml
  src/lib.rs                                      LocalDocker::connect
  src/convert.rs                                  spec⇄Docker JSON, bollard→domain summaries (pure)
  src/files.rs                                    tar listing/extraction (pure) + caps
  src/error.rs                                    bollard error → core Error
  src/containers.rs                               impl ContainerOps
  src/resources.rs                                impl SystemOps/ImageOps/VolumeOps/NetworkOps
  src/env.rs                                      LocalDockerEnv (lazy reconnect), DockerEnvFactory
  tests/docker_it.rs                              real-daemon tests (feature docker-it)
crates/fleetops-api/
  Cargo.toml                                      (modify) + async-trait (dev)
  src/lib.rs                                      (modify) + authz, envs, compose modules
  src/state.rs                                    (modify) AppState.envs; new(store, config, factory)
  src/envs.rs                                     EnvRegistry + bootstrap_local
  src/authz.rs                                    Action + authorize()
  src/compose.rs                                  group_projects / project_members (pure)
  src/routes/mod.rs                               (modify) env_routes()
  src/routes/envs.rs                              environment CRUD + status
  src/routes/docker/mod.rs                        EnvCtx (ctx + audit), ForceQuery
  src/routes/docker/{system,containers,images,volumes,networks,compose}.rs
  tests/common/mod.rs                             (modify) FakeFactory-backed TestApp, add_env, with_local
  tests/common/fake_docker.rs                     in-memory FakeDocker/FakeEnv/FakeFactory
  tests/envs.rs  tests/containers.rs  tests/resources.rs  tests/compose.rs
crates/fleetops-server/
  Cargo.toml                                      (modify) + fleetops-docker
  src/lib.rs                                      (modify) DockerEnvFactory, load, bootstrap_local
  tests/static_files.rs                           (modify) AppState::new third argument
```

---

### Task 1: Core Docker domain types, operation traits and environment traits

**Files:**
- Modify: `Cargo.toml`, `crates/fleetops-core/Cargo.toml`, `crates/fleetops-core/src/lib.rs`
- Create: `crates/fleetops-core/src/docker.rs`, `crates/fleetops-core/src/env.rs`
- Test: `crates/fleetops-core/tests/docker_types.rs`

**Interfaces:**
- Consumes: `fleetops_core::{Error, Result}` (P1).
- Produces:
  - Types:
    - `EngineVersion{version, api_version, os, arch}`
    - `PortMapping`
    - `ContainerSummary{id, name, image, image_id, state, status, created, ports, labels, compose_project, compose_service, volumes}`
    - `PortBinding{container_port, protocol="tcp", host_ip, host_port}`
    - `ContainerSpec{name, image, cmd, entrypoint, env, ports, binds, network, restart_policy, labels, memory_bytes, nano_cpus, cap_add, privileged, advanced}` with `validate()`
    - `validate_container_name(&str)`
    - `ContainerAction{Start,Stop,Restart,Kill,Pause,Unpause}` with `as_str` and `FromStr` (unknown gives `NotFound`)
    - `ImageSummary`, `VolumeSummary`, `VolumeSpec`, `NetworkSummary`, `NetworkSpec`, `PruneReport`
    - `EntryKind`, `DirEntry`, `DirListing{path, entries, truncated}`
  - Constants: `COMPOSE_PROJECT_LABEL`, `COMPOSE_SERVICE_LABEL`.
  - Traits: `SystemOps`, `ContainerOps`, `ImageOps`, `VolumeOps`, `NetworkOps`, and `DockerOps` (blanket-implemented).
  - Environment types in `fleetops_core::env`:
    - `EnvKind{DockerLocal}` ("docker-local")
    - `EnvStatus{Up{version}, Down{reason}}`, serialized with tag `state`
    - `trait Environment{id, name, kind, status, docker() -> Option<Arc<dyn DockerOps>>}`
    - `trait EnvFactory{build(id, name, kind, url) -> Result<Arc<dyn Environment>>}`
    - `validate_env_name`, `validate_env_url`

- [ ] **Step 1: Update manifests**

In the root `Cargo.toml`:
- Change `rust-version = "1.85"` to `rust-version = "1.86"`, because `fleetops-api` upcasts `&dyn DockerOps` to `&dyn ContainerOps`.
- Add to `[workspace.dependencies]`, keeping alphabetical order:

```toml
async-trait = "0.1"
bollard = "0.21"
futures-util = "0.3"
tar = "0.4"
```

Replace the dependency sections of `crates/fleetops-core/Cargo.toml` with:

```toml
[dependencies]
async-trait.workspace = true
serde_json.workspace = true
serde.workspace = true
thiserror.workspace = true

[dev-dependencies]
tokio.workspace = true
```

- [ ] **Step 2: Write the failing test**

```rust
use fleetops_core::docker::{ContainerAction, ContainerSpec, EngineVersion, PortBinding};
use fleetops_core::env::{validate_env_name, validate_env_url, EnvKind, EnvStatus};
use fleetops_core::Error;
use serde_json::json;

#[test]
fn spec_deserializes_with_defaults() {
    let spec: ContainerSpec = serde_json::from_value(json!({
        "image": "nginx:1.27",
        "ports": [{"container_port": 80, "host_port": 8080}]
    }))
    .unwrap();
    assert_eq!(spec.image, "nginx:1.27");
    assert_eq!(spec.name, None);
    assert!(spec.env.is_empty());
    assert_eq!(
        spec.ports,
        vec![PortBinding {
            container_port: 80,
            protocol: "tcp".into(),
            host_ip: None,
            host_port: Some(8080)
        }]
    );
    assert!(!spec.privileged);
}

#[test]
fn spec_validation() {
    let ok = ContainerSpec {
        image: "alpine".into(),
        ..Default::default()
    };
    assert!(ok.validate().is_ok());
    let cases = [
        ContainerSpec {
            image: " ".into(),
            ..Default::default()
        },
        ContainerSpec {
            name: Some("-bad".into()),
            ..ok.clone()
        },
        ContainerSpec {
            name: Some("has space".into()),
            ..ok.clone()
        },
        ContainerSpec {
            restart_policy: Some("sometimes".into()),
            ..ok.clone()
        },
        ContainerSpec {
            advanced: Some(json!([1])),
            ..ok.clone()
        },
        ContainerSpec {
            ports: vec![PortBinding {
                container_port: 80,
                protocol: "icmp".into(),
                host_ip: None,
                host_port: None,
            }],
            ..ok.clone()
        },
        ContainerSpec {
            ports: vec![PortBinding {
                container_port: 0,
                protocol: "tcp".into(),
                host_ip: None,
                host_port: None,
            }],
            ..ok.clone()
        },
    ];
    for c in cases {
        assert!(matches!(c.validate(), Err(Error::Invalid(_))), "{c:?}");
    }
    let named = ContainerSpec {
        name: Some("web_1.a-b".into()),
        restart_policy: Some("unless-stopped".into()),
        ..ok
    };
    assert!(named.validate().is_ok());
}

#[test]
fn container_action_parse() {
    for a in ["start", "stop", "restart", "kill", "pause", "unpause"] {
        assert_eq!(a.parse::<ContainerAction>().unwrap().as_str(), a);
    }
    assert!(matches!(
        "explode".parse::<ContainerAction>(),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn env_kind_and_status_serialization() {
    assert_eq!(
        "docker-local".parse::<EnvKind>().unwrap(),
        EnvKind::DockerLocal
    );
    assert!("k8s".parse::<EnvKind>().is_err());
    let up = EnvStatus::Up {
        version: EngineVersion {
            version: "29.0".into(),
            api_version: "1.54".into(),
            os: "linux".into(),
            arch: "arm64".into(),
        },
    };
    assert_eq!(serde_json::to_value(&up).unwrap()["state"], "up");
    assert_eq!(
        serde_json::to_value(&up).unwrap()["version"]["api_version"],
        "1.54"
    );
    let down = EnvStatus::Down {
        reason: "socket missing".into(),
    };
    assert_eq!(
        serde_json::to_value(&down).unwrap(),
        json!({"state": "down", "reason": "socket missing"})
    );
}

#[test]
fn env_name_and_url_rules() {
    assert!(validate_env_name("local").is_ok());
    assert!(validate_env_name("prod-eu.1").is_ok());
    assert!(validate_env_name("").is_err());
    assert!(validate_env_name("a/b").is_err());
    assert!(validate_env_name(&"x".repeat(65)).is_err());
    for ok in [
        "unix:///var/run/docker.sock",
        "tcp://10.0.0.5:2375",
        "http://h:2375",
        "https://h:2376",
    ] {
        assert!(validate_env_url(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "unix://",
        "/var/run/docker.sock",
        "ssh://host",
        "ftp://x",
    ] {
        assert!(validate_env_url(bad).is_err(), "{bad}");
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p fleetops-core --test docker_types`
Expected: FAIL to compile with `unresolved import fleetops_core::docker` / `fleetops_core::env`.

- [ ] **Step 4: Implement**

Add to `crates/fleetops-core/src/lib.rs`, above `pub mod error;`:

```rust
pub mod docker;
pub mod env;
```

`crates/fleetops-core/src/docker.rs`:

```rust
//! Docker domain types and operation traits. Implementations (bollard in
//! `fleetops-docker`, fakes in tests) never leak their own types past here.
//! Inspect-style payloads are passed through as raw JSON because the UI shows
//! them verbatim.

use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, str::FromStr};

pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
pub const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineVersion {
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMapping {
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub ip: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSummary {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
    pub in_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSummary {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub in_use: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolumeSpec {
    pub name: String,
    pub driver: Option<String>,
    pub driver_opts: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    pub deleted: Vec<String>,
    pub space_reclaimed: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mode: u32,
    pub modified: i64,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub trait DockerOps: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps {}

impl<T: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps> DockerOps for T {}
```

`crates/fleetops-core/src/env.rs`:

```rust
//! Environments are the things fleetops manages. Sub-project 1 has one kind
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-core && make lint`
Expected: `docker_types` 5 PASS, P1 core tests still PASS, lint clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-core
git commit -m "feat(core): Docker domain types, operation traits and environment traits"
```

---

### Task 2: Recreate-with-rollback

**Files:**
- Create: `crates/fleetops-core/src/recreate.rs`
- Modify: `crates/fleetops-core/src/lib.rs` (add `pub mod recreate;`)
- Test: `crates/fleetops-core/tests/recreate.rs`

**Interfaces:**
- Consumes: `ContainerOps`, `ContainerSpec`, `ContainerAction` (Task 1).
- Produces:
  - `recreate(ops: &dyn ContainerOps, id: &str, spec: &ContainerSpec, now: i64) -> Result<RecreateOutcome>`
  - `RecreateOutcome{new_id, old_removed: bool, backup_name}`
  - The backup name is `<old>-fleetops-old-<now>`.

- [ ] **Step 1: Write the failing test**

```rust
use async_trait::async_trait;
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::recreate::recreate;
use fleetops_core::{Error, Result};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Records every call; `fail_on` makes the first matching call fail.
#[derive(Default)]
struct Fake {
    running: bool,
    calls: Mutex<Vec<String>>,
    fail_on: Option<&'static str>,
}

impl Fake {
    fn new(running: bool, fail_on: Option<&'static str>) -> Self {
        Fake {
            running,
            fail_on,
            ..Default::default()
        }
    }

    fn record(&self, call: String) -> Result<()> {
        let fail = self.fail_on.is_some_and(|f| call.starts_with(f));
        self.calls.lock().unwrap().push(call);
        if fail {
            Err(Error::Upstream {
                status: 500,
                message: "boom".into(),
            })
        } else {
            Ok(())
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ContainerOps for Fake {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        unimplemented!()
    }
    async fn inspect_container(&self, _id: &str) -> Result<Value> {
        Ok(json!({"Name": "/web", "State": {"Running": self.running}}))
    }
    async fn container_spec(&self, _id: &str) -> Result<ContainerSpec> {
        unimplemented!()
    }
    async fn create_container(&self, spec: &ContainerSpec) -> Result<String> {
        self.record(format!("create {}", spec.name.as_deref().unwrap_or("")))?;
        Ok("new".into())
    }
    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        self.record(format!("{} {id}", action.as_str()))
    }
    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        self.record(format!("rename {id} {new_name}"))
    }
    async fn remove_container(&self, id: &str, _force: bool, _volumes: bool) -> Result<()> {
        self.record(format!("remove {id}"))
    }
    async fn prune_containers(&self) -> Result<PruneReport> {
        unimplemented!()
    }
    async fn list_dir(&self, _id: &str, _path: &str) -> Result<DirListing> {
        unimplemented!()
    }
    async fn read_file(&self, _id: &str, _path: &str, _max: u64) -> Result<Vec<u8>> {
        unimplemented!()
    }
}

fn spec() -> ContainerSpec {
    ContainerSpec {
        image: "nginx:2".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn happy_path_keeps_name_and_removes_old() {
    let f = Fake::new(true, None);
    let out = recreate(&f, "old", &spec(), 42).await.unwrap();
    assert_eq!(out.new_id, "new");
    assert!(out.old_removed);
    assert_eq!(out.backup_name, "web-fleetops-old-42");
    assert_eq!(
        f.calls(),
        vec![
            "stop old",
            "rename old web-fleetops-old-42",
            "create web",
            "start new",
            "remove old"
        ]
    );
}

#[tokio::test]
async fn stopped_container_is_not_stopped_or_restarted() {
    let f = Fake::new(false, Some("create"));
    assert!(recreate(&f, "old", &spec(), 1).await.is_err());
    assert_eq!(
        f.calls(),
        vec![
            "rename old web-fleetops-old-1",
            "create web",
            "rename old web"
        ]
    );
}

#[tokio::test]
async fn start_failure_rolls_back_and_restarts_old() {
    let f = Fake::new(true, Some("start new"));
    let err = recreate(&f, "old", &spec(), 7).await.unwrap_err();
    assert!(matches!(err, Error::Upstream { .. }));
    assert_eq!(
        f.calls(),
        vec![
            "stop old",
            "rename old web-fleetops-old-7",
            "create web",
            "start new",
            "remove new",
            "rename old web",
            "start old",
        ]
    );
}

#[tokio::test]
async fn new_name_is_used_when_given() {
    let f = Fake::new(false, None);
    let s = ContainerSpec {
        name: Some("web2".into()),
        ..spec()
    };
    recreate(&f, "old", &s, 1).await.unwrap();
    assert!(f.calls().contains(&"create web2".to_string()));
}

#[tokio::test]
async fn old_removal_failure_is_reported_not_fatal() {
    let f = Fake::new(false, Some("remove old"));
    let out = recreate(&f, "old", &spec(), 3).await.unwrap();
    assert!(!out.old_removed);
}

#[tokio::test]
async fn invalid_spec_touches_nothing() {
    let f = Fake::new(true, None);
    let bad = ContainerSpec {
        image: String::new(),
        ..Default::default()
    };
    assert!(matches!(
        recreate(&f, "old", &bad, 1).await,
        Err(Error::Invalid(_))
    ));
    assert!(f.calls().is_empty());
}

#[tokio::test]
async fn rename_failure_restarts_old() {
    let f = Fake::new(true, Some("rename"));
    assert!(recreate(&f, "old", &spec(), 5).await.is_err());
    assert_eq!(
        f.calls(),
        vec!["stop old", "rename old web-fleetops-old-5", "start old"]
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleetops-core --test recreate`
Expected: FAIL to compile with `unresolved import fleetops_core::recreate`.

- [ ] **Step 3: Implement**

Add `pub mod recreate;` to `crates/fleetops-core/src/lib.rs` (after `pub mod error;`).

`crates/fleetops-core/src/recreate.rs`:

```rust
//! "Recreate with changes": replace a container with one built from a new
//! spec, rolling back to the original if the replacement cannot start.

use crate::{
    docker::{ContainerAction, ContainerOps, ContainerSpec},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecreateOutcome {
    pub new_id: String,
    /// False when the replacement is running but the old container could
    /// not be deleted; it is left renamed as `backup_name`.
    pub old_removed: bool,
    pub backup_name: String,
}

/// Steps:
/// 1. stop the old container if it is running;
/// 2. rename it to `<name>-fleetops-old-<now>`;
/// 3. create and start the replacement under the target name;
/// 4. remove the old container.
///
/// If step 3 fails, the replacement is removed, the old container gets its
/// name back and is restarted if it was running, and the step-3 error is
/// returned.
pub async fn recreate(
    ops: &dyn ContainerOps,
    id: &str,
    spec: &ContainerSpec,
    now: i64,
) -> Result<RecreateOutcome> {
    spec.validate()?;
    let inspect = ops.inspect_container(id).await?;
    let old_name = inspect["Name"]
        .as_str()
        .map(|n| n.trim_start_matches('/').to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Internal("container inspect has no Name".into()))?;
    let was_running = inspect["State"]["Running"].as_bool().unwrap_or(false);
    let backup_name = format!("{old_name}-fleetops-old-{now}");

    if was_running {
        ops.container_action(id, ContainerAction::Stop).await?;
    }
    if let Err(e) = ops.rename_container(id, &backup_name).await {
        if was_running {
            let _ = ops.container_action(id, ContainerAction::Start).await;
        }
        return Err(e);
    }

    let mut new_spec = spec.clone();
    new_spec.name = Some(spec.name.clone().unwrap_or_else(|| old_name.clone()));

    match create_and_start(ops, &new_spec).await {
        Ok(new_id) => {
            let old_removed = ops.remove_container(id, true, false).await.is_ok();
            Ok(RecreateOutcome {
                new_id,
                old_removed,
                backup_name,
            })
        }
        Err(e) => {
            let _ = ops.rename_container(id, &old_name).await;
            if was_running {
                let _ = ops.container_action(id, ContainerAction::Start).await;
            }
            Err(e)
        }
    }
}

async fn create_and_start(ops: &dyn ContainerOps, spec: &ContainerSpec) -> Result<String> {
    let new_id = ops.create_container(spec).await?;
    if let Err(e) = ops.container_action(&new_id, ContainerAction::Start).await {
        let _ = ops.remove_container(&new_id, true, false).await;
        return Err(e);
    }
    Ok(new_id)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-core && make lint`
Expected: `recreate` 7 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-core
git commit -m "feat(core): recreate-with-changes with rollback on failure"
```

---

### Task 3: Store environments repository

**Files:**
- Create: `crates/fleetops-store/src/environments.rs`
- Modify: `crates/fleetops-store/src/lib.rs`
- Test: `crates/fleetops-store/tests/environments.rs`

**Interfaces:**
- Consumes: `Store`, `db_err` (P1). The `environments` table already exists from P1's migration.
- Produces:
  - `EnvironmentRecord{id, name, kind: String, url, created_at, updated_at}` (Serialize, FromRow).
  - `Store` methods:
    - `create_environment(name, kind, url, now) -> Result<EnvironmentRecord>`: duplicate name gives `Conflict("environment '<name>' already exists")`.
    - `list_environments() -> Result<Vec<EnvironmentRecord>>`, ordered by id.
    - `get_environment(id) -> Result<EnvironmentRecord>`.
    - `update_environment(id, name, url, now) -> Result<EnvironmentRecord>`.
    - `delete_environment(id) -> Result<()>`.
  - A missing id gives `NotFound`.

- [ ] **Step 1: Write the failing test**

```rust
use fleetops_core::Error;
use fleetops_store::{NewAudit, Store};

#[tokio::test]
async fn crud_roundtrip() {
    let s = Store::in_memory().await.unwrap();
    let e = s
        .create_environment("local", "docker-local", "unix:///var/run/docker.sock", 10)
        .await
        .unwrap();
    assert_eq!(e.name, "local");
    assert_eq!(e.kind, "docker-local");
    assert_eq!(s.get_environment(e.id).await.unwrap(), e);

    let u = s
        .update_environment(e.id, "home", "unix:///run/podman.sock", 20)
        .await
        .unwrap();
    assert_eq!(
        (u.name.as_str(), u.url.as_str(), u.updated_at, u.created_at),
        ("home", "unix:///run/podman.sock", 20, 10)
    );
    assert_eq!(s.list_environments().await.unwrap(), vec![u.clone()]);

    s.delete_environment(e.id).await.unwrap();
    assert!(s.list_environments().await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_names_conflict() {
    let s = Store::in_memory().await.unwrap();
    s.create_environment("local", "docker-local", "u", 0)
        .await
        .unwrap();
    let other = s
        .create_environment("other", "docker-local", "u", 0)
        .await
        .unwrap();
    assert!(
        matches!(s.create_environment("local", "docker-local", "u", 0).await, Err(Error::Conflict(m)) if m.contains("local"))
    );
    assert!(matches!(
        s.update_environment(other.id, "local", "u", 1).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn missing_environment_is_not_found() {
    let s = Store::in_memory().await.unwrap();
    assert!(matches!(
        s.get_environment(9).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.update_environment(9, "x", "u", 0).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        s.delete_environment(9).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn deleting_environment_keeps_audit_rows() {
    let s = Store::in_memory().await.unwrap();
    let e = s
        .create_environment("local", "docker-local", "u", 0)
        .await
        .unwrap();
    s.insert_audit(&NewAudit {
        ts: 1,
        user_id: None,
        environment_id: Some(e.id),
        action: "container.start",
        target_type: Some("container"),
        target_id: Some("abc"),
        result: "success",
        detail: None,
    })
    .await
    .unwrap();
    s.delete_environment(e.id).await.unwrap();
    let rows = s.list_audit(10, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].environment_id, None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleetops-store --test environments`
Expected: FAIL to compile with `no method named create_environment`.

- [ ] **Step 3: Implement**

`crates/fleetops-store/src/environments.rs`:

```rust
use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct EnvironmentRecord {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLS: &str = "id, name, kind, url, created_at, updated_at";

impl Store {
    pub async fn create_environment(
        &self,
        name: &str,
        kind: &str,
        url: &str,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        sqlx::query_as(&format!(
            "INSERT INTO environments (name, kind, url, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?) RETURNING {COLS}"
        ))
        .bind(name)
        .bind(kind)
        .bind(url)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
            other => other,
        })
    }

    pub async fn list_environments(&self) -> Result<Vec<EnvironmentRecord>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM environments ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn get_environment(&self, id: i64) -> Result<EnvironmentRecord> {
        sqlx::query_as::<_, EnvironmentRecord>(&format!(
            "SELECT {COLS} FROM environments WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("environment {id}")))
    }

    pub async fn update_environment(
        &self,
        id: i64,
        name: &str,
        url: &str,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        sqlx::query_as::<_, EnvironmentRecord>(&format!(
            "UPDATE environments SET name = ?, url = ?, updated_at = ? WHERE id = ? RETURNING {COLS}"
        ))
        .bind(name)
        .bind(url)
        .bind(now)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
            other => other,
        })?
        .ok_or_else(|| Error::NotFound(format!("environment {id}")))
    }

    pub async fn delete_environment(&self, id: i64) -> Result<()> {
        let res = sqlx::query("DELETE FROM environments WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("environment {id}")));
        }
        Ok(())
    }
}
```

In `crates/fleetops-store/src/lib.rs`, add `mod environments;` after `mod audit;`, and `pub use environments::EnvironmentRecord;` after `pub use audit::{AuditEntry, NewAudit};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-store && make lint`
Expected: `environments` 4 PASS, all P1 store tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-store
git commit -m "feat(store): environments repository"
```

---

### Task 4: `fleetops-docker` crate and pure conversions

**Files:**
- Modify: `Cargo.toml` (workspace dependency `fleetops-docker`)
- Create: `crates/fleetops-docker/Cargo.toml`, `crates/fleetops-docker/src/lib.rs`, `crates/fleetops-docker/src/convert.rs`

**Interfaces:**
- Consumes: Task 1 domain types; `bollard::models::{ContainerSummary, Volume, Network, ContainerCreateBody}`.
- Produces (all `pub` in `fleetops_docker::convert`):
  - `spec_to_body(&ContainerSpec) -> serde_json::Value`: a Docker `ContainerCreate` JSON with `advanced` deep-merged last.
  - `merge(&mut Value, &Value)`.
  - `inspect_to_spec(&Value) -> ContainerSpec`: normalises `NetworkMode "default"`, an empty restart policy and zero limits to `None`.
  - `container_summary(bollard::models::ContainerSummary) -> ContainerSummary`.
  - `volume_summary(bollard::models::Volume, &HashSet<String>) -> VolumeSummary`.
  - `network_summary(bollard::models::Network) -> NetworkSummary`.

- [ ] **Step 1: Create the crate with its full manifest**

Add to `[workspace.dependencies]` in the root `Cargo.toml`, next to the other path dependencies:

```toml
fleetops-docker = { path = "crates/fleetops-docker" }
```

`crates/fleetops-docker/Cargo.toml` (all dependencies the crate will need across Tasks 4–6):

```toml
[package]
name = "fleetops-docker"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[features]
# Integration tests against a real Docker daemon (see `make test-docker`).
docker-it = []

[dependencies]
fleetops-core.workspace = true
async-trait.workspace = true
bollard.workspace = true
futures-util.workspace = true
serde.workspace = true
serde_json.workspace = true
tar.workspace = true
tokio.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

`crates/fleetops-docker/src/lib.rs` (temporary; Task 5 adds `files` and Task 6 replaces this file):

```rust
//! bollard-backed implementation of fleetops's Docker traits.

pub mod convert;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/fleetops-docker/src/convert.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn full_spec() -> ContainerSpec {
        ContainerSpec {
            name: Some("web".into()),
            image: "nginx:1.27".into(),
            cmd: Some(vec!["nginx".into(), "-g".into(), "daemon off;".into()]),
            entrypoint: None,
            env: vec!["A=1".into()],
            ports: vec![
                PortBinding {
                    container_port: 80,
                    protocol: "tcp".into(),
                    host_ip: None,
                    host_port: Some(8080),
                },
                PortBinding {
                    container_port: 53,
                    protocol: "udp".into(),
                    host_ip: Some("127.0.0.1".into()),
                    host_port: None,
                },
            ],
            binds: vec!["data:/data".into()],
            network: Some("backend".into()),
            restart_policy: Some("unless-stopped".into()),
            labels: BTreeMap::from([("team".into(), "a".into())]),
            memory_bytes: Some(1 << 28),
            nano_cpus: Some(500_000_000),
            cap_add: vec!["NET_ADMIN".into()],
            privileged: false,
            advanced: None,
        }
    }

    #[test]
    fn spec_to_body_maps_every_field() {
        let b = spec_to_body(&full_spec());
        assert_eq!(b["Image"], "nginx:1.27");
        assert_eq!(b["Cmd"], json!(["nginx", "-g", "daemon off;"]));
        assert!(b.get("Entrypoint").is_none());
        assert_eq!(b["Env"], json!(["A=1"]));
        assert_eq!(b["Labels"], json!({"team": "a"}));
        assert_eq!(b["ExposedPorts"], json!({"80/tcp": {}, "53/udp": {}}));
        let h = &b["HostConfig"];
        assert_eq!(
            h["PortBindings"]["80/tcp"],
            json!([{"HostIp": "", "HostPort": "8080"}])
        );
        assert_eq!(
            h["PortBindings"]["53/udp"],
            json!([{"HostIp": "127.0.0.1", "HostPort": ""}])
        );
        assert_eq!(h["Binds"], json!(["data:/data"]));
        assert_eq!(h["NetworkMode"], "backend");
        assert_eq!(h["RestartPolicy"], json!({"Name": "unless-stopped"}));
        assert_eq!(h["Memory"], json!(1 << 28));
        assert_eq!(h["NanoCpus"], json!(500_000_000));
        assert_eq!(h["CapAdd"], json!(["NET_ADMIN"]));
        assert_eq!(h["Privileged"], false);
    }

    #[test]
    fn advanced_is_deep_merged_last() {
        let spec = ContainerSpec {
            advanced: Some(
                json!({"HostConfig": {"Privileged": true, "ShmSize": 64}, "WorkingDir": "/app"}),
            ),
            ..full_spec()
        };
        let b = spec_to_body(&spec);
        assert_eq!(b["HostConfig"]["Privileged"], true);
        assert_eq!(b["HostConfig"]["ShmSize"], 64);
        assert_eq!(
            b["HostConfig"]["NetworkMode"], "backend",
            "untouched keys survive"
        );
        assert_eq!(b["WorkingDir"], "/app");
    }

    #[test]
    fn body_deserializes_into_bollard_create_body() {
        let body: bollard::models::ContainerCreateBody =
            serde_json::from_value(spec_to_body(&full_spec())).unwrap();
        assert_eq!(body.image.as_deref(), Some("nginx:1.27"));
        let host = body.host_config.unwrap();
        assert_eq!(host.binds, Some(vec!["data:/data".to_string()]));
        assert!(body.exposed_ports.unwrap().contains(&"80/tcp".to_string()));
    }

    #[test]
    fn inspect_roundtrips_through_spec() {
        let spec = full_spec();
        let body = spec_to_body(&spec);
        // Shape an inspect response the way Docker does: name with a slash,
        // Config holding container fields, HostConfig alongside.
        let inspect = json!({
            "Name": "/web",
            "Config": {"Image": body["Image"], "Cmd": body["Cmd"], "Entrypoint": null, "Env": body["Env"], "Labels": body["Labels"]},
            "HostConfig": body["HostConfig"],
        });
        let mut back = inspect_to_spec(&inspect);
        let mut expected = spec;
        expected
            .ports
            .sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
        back.ports
            .sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
        assert_eq!(back, expected);
    }

    #[test]
    fn inspect_defaults_are_normalised() {
        let spec = inspect_to_spec(&json!({
            "Name": "/x",
            "Config": {"Image": "alpine", "Cmd": null},
            "HostConfig": {"NetworkMode": "default", "RestartPolicy": {"Name": ""}, "Memory": 0, "PortBindings": {}}
        }));
        assert_eq!(spec.network, None);
        assert_eq!(spec.restart_policy, None);
        assert_eq!(spec.memory_bytes, None);
        assert_eq!(spec.cmd, None);
        assert!(spec.ports.is_empty());
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p fleetops-docker --lib convert`
Expected: FAIL to compile with `cannot find function spec_to_body` / `inspect_to_spec`.

- [ ] **Step 4: Implement**

Prepend to `crates/fleetops-docker/src/convert.rs`:

```rust
//! Pure conversions between fleetops's domain types and Docker API JSON.
//! Kept free of I/O so they can be unit-tested without a daemon.

use fleetops_core::docker::{
    ContainerSpec, ContainerSummary, NetworkSummary, PortBinding, PortMapping, VolumeSummary,
    COMPOSE_PROJECT_LABEL, COMPOSE_SERVICE_LABEL,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

/// Builds a Docker `ContainerCreate` body. `spec.advanced` is deep-merged
/// last, so power users can set any field the form does not cover.
pub fn spec_to_body(spec: &ContainerSpec) -> Value {
    let mut exposed = Map::new();
    let mut bindings = Map::new();
    for p in &spec.ports {
        let key = format!("{}/{}", p.container_port, p.protocol);
        exposed.insert(key.clone(), json!({}));
        let entry = bindings.entry(key).or_insert_with(|| json!([]));
        if let Value::Array(list) = entry {
            list.push(json!({
                "HostIp": p.host_ip.clone().unwrap_or_default(),
                "HostPort": p.host_port.map(|n| n.to_string()).unwrap_or_default(),
            }));
        }
    }
    let mut host = json!({
        "Binds": spec.binds,
        "PortBindings": bindings,
        "Privileged": spec.privileged,
    });
    if let Some(policy) = &spec.restart_policy {
        host["RestartPolicy"] = json!({ "Name": policy });
    }
    if let Some(m) = spec.memory_bytes {
        host["Memory"] = json!(m);
    }
    if let Some(n) = spec.nano_cpus {
        host["NanoCpus"] = json!(n);
    }
    if !spec.cap_add.is_empty() {
        host["CapAdd"] = json!(spec.cap_add);
    }
    if let Some(net) = &spec.network {
        host["NetworkMode"] = json!(net);
    }
    let mut body = json!({
        "Image": spec.image,
        "Env": spec.env,
        "Labels": spec.labels,
        "ExposedPorts": exposed,
        "HostConfig": host,
    });
    if let Some(cmd) = &spec.cmd {
        body["Cmd"] = json!(cmd);
    }
    if let Some(ep) = &spec.entrypoint {
        body["Entrypoint"] = json!(ep);
    }
    if let Some(adv) = &spec.advanced {
        merge(&mut body, adv);
    }
    body
}

/// Deep-merges `over` into `base`: objects merge key by key, anything else
/// replaces.
pub fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn opt_strings(v: &Value) -> Option<Vec<String>> {
    v.is_array().then(|| strings(v))
}

fn positive(v: &Value) -> Option<i64> {
    v.as_i64().filter(|n| *n > 0)
}

/// The create-form view of a container, from its inspect JSON.
pub fn inspect_to_spec(v: &Value) -> ContainerSpec {
    let config = &v["Config"];
    let host = &v["HostConfig"];
    let mut ports = Vec::new();
    if let Some(map) = host["PortBindings"].as_object() {
        for (key, list) in map {
            let (port, proto) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
            let Ok(container_port) = port.parse::<u16>() else {
                continue;
            };
            for b in list.as_array().into_iter().flatten() {
                ports.push(PortBinding {
                    container_port,
                    protocol: proto.to_string(),
                    host_ip: b["HostIp"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    host_port: b["HostPort"].as_str().and_then(|s| s.parse().ok()),
                });
            }
        }
    }
    ports.sort_by(|a, b| (a.container_port, &a.protocol).cmp(&(b.container_port, &b.protocol)));
    let labels = config["Labels"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    ContainerSpec {
        name: v["Name"]
            .as_str()
            .map(|n| n.trim_start_matches('/').to_string()),
        image: config["Image"].as_str().unwrap_or_default().to_string(),
        cmd: opt_strings(&config["Cmd"]),
        entrypoint: opt_strings(&config["Entrypoint"]),
        env: strings(&config["Env"]),
        ports,
        binds: strings(&host["Binds"]),
        network: host["NetworkMode"]
            .as_str()
            .filter(|n| *n != "default" && !n.is_empty())
            .map(str::to_owned),
        restart_policy: host["RestartPolicy"]["Name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
        labels,
        memory_bytes: positive(&host["Memory"]),
        nano_cpus: positive(&host["NanoCpus"]),
        cap_add: strings(&host["CapAdd"]),
        privileged: host["Privileged"].as_bool().unwrap_or(false),
        advanced: None,
    }
}

pub fn container_summary(c: bollard::models::ContainerSummary) -> ContainerSummary {
    let labels: BTreeMap<String, String> = c.labels.unwrap_or_default().into_iter().collect();
    ContainerSummary {
        id: c.id.unwrap_or_default(),
        name: c
            .names
            .and_then(|n| n.into_iter().next())
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default(),
        image: c.image.unwrap_or_default(),
        image_id: c.image_id.unwrap_or_default(),
        state: c.state.map(|s| s.to_string()).unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        created: c.created.unwrap_or_default(),
        ports: c
            .ports
            .unwrap_or_default()
            .into_iter()
            .map(|p| PortMapping {
                private_port: p.private_port,
                public_port: p.public_port,
                ip: p.ip,
                protocol: p
                    .typ
                    .map(|t| t.to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "tcp".into()),
            })
            .collect(),
        compose_project: labels.get(COMPOSE_PROJECT_LABEL).cloned(),
        compose_service: labels.get(COMPOSE_SERVICE_LABEL).cloned(),
        labels,
        volumes: c
            .mounts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.name)
            .collect(),
    }
}

pub fn volume_summary(v: bollard::models::Volume, in_use: &HashSet<String>) -> VolumeSummary {
    VolumeSummary {
        in_use: in_use.contains(&v.name),
        name: v.name,
        driver: v.driver,
        mountpoint: v.mountpoint,
        created_at: v.created_at,
        labels: v.labels.into_iter().collect(),
    }
}

pub fn network_summary(n: bollard::models::Network) -> NetworkSummary {
    let ipam = n
        .ipam
        .and_then(|i| i.config)
        .and_then(|c| c.into_iter().next());
    NetworkSummary {
        id: n.id.unwrap_or_default(),
        name: n.name.unwrap_or_default(),
        driver: n.driver.unwrap_or_default(),
        scope: n.scope.unwrap_or_default(),
        internal: n.internal.unwrap_or(false),
        attachable: n.attachable.unwrap_or(false),
        subnet: ipam.as_ref().and_then(|c| c.subnet.clone()),
        gateway: ipam.and_then(|c| c.gateway),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-docker && make lint`
Expected: 5 convert tests PASS. `body_deserializes_into_bollard_create_body` proves the JSON shape matches bollard 0.21's serde, including `ExposedPorts` map → Vec.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-docker
git commit -m "feat(docker): crate scaffold and spec/inspect/summary conversions"
```

---

### Task 5: Container file browsing over the tar archive API

**Files:**
- Create: `crates/fleetops-docker/src/files.rs`
- Modify: `crates/fleetops-docker/src/lib.rs` (add `pub mod files;`)

**Interfaces:**
- Consumes: `DirEntry`, `DirListing`, `EntryKind` (Task 1).
- Produces:
  - `files::LIST_MAX_BYTES` (64 MiB) and `files::LIST_MAX_ENTRIES` (5000).
  - `check_path(&str) -> Result<()>`: the path must be absolute with no NUL.
  - `parse_listing(tar: &[u8], path: &str, stream_truncated: bool) -> Result<DirListing>`: direct children only, directories first then by name. A file path gives `Invalid`.
  - `extract_file(tar: &[u8], path: &str) -> Result<Vec<u8>>`: directories and non-regular files give `Invalid`.
- Docker archive naming (verified against a live daemon):
  - `/etc` produces entries `etc/`, `etc/passwd`, …
  - `/` produces absolute names `/`, `/bin/`, …
  - a file produces just its basename.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleetops-docker/src/files.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tar_of(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, kind, data, link) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(1_700_000_000);
            if let Some(l) = link {
                h.set_link_name(l).unwrap();
            }
            // Write the name bytes directly: Docker emits absolute names
            // (e.g. "/bin/") for an archive of "/", which set_path rejects.
            let name = &mut h.as_gnu_mut().unwrap().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    use tar::EntryType::{Directory as D, Regular as F, Symlink as L};

    #[test]
    fn lists_direct_children_dirs_first() {
        let t = tar_of(&[
            ("etc/", D, b"", None),
            ("etc/passwd", F, b"root", None),
            ("etc/apk/", D, b"", None),
            ("etc/apk/arch", F, b"x", None),
            ("etc/localtime", L, b"", Some("/usr/share/zoneinfo/UTC")),
        ]);
        let l = parse_listing(&t, "/etc", false).unwrap();
        let names: Vec<_> = l
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect();
        assert_eq!(
            names,
            vec![
                ("apk", EntryKind::Dir),
                ("localtime", EntryKind::Symlink),
                ("passwd", EntryKind::File)
            ]
        );
        assert_eq!(l.entries[2].size, 4);
        assert_eq!(
            l.entries[1].link_target.as_deref(),
            Some("/usr/share/zoneinfo/UTC")
        );
        assert!(!l.truncated);
    }

    #[test]
    fn lists_root_directory() {
        let t = tar_of(&[
            ("/", D, b"", None),
            ("/.dockerenv", F, b"", None),
            ("/bin/", D, b"", None),
            ("/bin/sh", F, b"", None),
        ]);
        let l = parse_listing(&t, "/", false).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bin", ".dockerenv"]);
    }

    #[test]
    fn trailing_slash_in_request_is_ignored() {
        let t = tar_of(&[("etc/", D, b"", None), ("etc/hosts", F, b"", None)]);
        assert_eq!(parse_listing(&t, "/etc/", false).unwrap().entries.len(), 1);
    }

    #[test]
    fn listing_a_file_is_invalid() {
        let t = tar_of(&[("hostname", F, b"abc", None)]);
        assert!(matches!(
            parse_listing(&t, "/etc/hostname", false),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn truncated_stream_returns_partial_listing() {
        let mut t = tar_of(&[
            ("etc/", D, b"", None),
            ("etc/a", F, b"1", None),
            ("etc/b", F, &[0u8; 4096], None),
        ]);
        t.truncate(t.len() - 3000);
        let l = parse_listing(&t, "/etc", true).unwrap();
        assert!(l.truncated);
        assert!(l.entries.iter().any(|e| e.name == "a"));
    }

    #[test]
    fn entry_cap_marks_truncated() {
        let names: Vec<String> = (0..LIST_MAX_ENTRIES + 5)
            .map(|i| format!("d/f{i}"))
            .collect();
        let mut specs: Vec<(&str, tar::EntryType, &[u8], Option<&str>)> =
            vec![("d/", D, b"", None)];
        specs.extend(names.iter().map(|n| (n.as_str(), F, &b""[..], None)));
        let l = parse_listing(&tar_of(&specs), "/d", false).unwrap();
        assert_eq!(l.entries.len(), LIST_MAX_ENTRIES);
        assert!(l.truncated);
    }

    #[test]
    fn extract_file_reads_contents_and_rejects_dirs() {
        assert_eq!(
            extract_file(&tar_of(&[("hostname", F, b"box\n", None)]), "/etc/hostname").unwrap(),
            b"box\n"
        );
        assert!(matches!(
            extract_file(&tar_of(&[("etc/", D, b"", None)]), "/etc"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            extract_file(&tar_of(&[("l", L, b"", Some("/x"))]), "/l"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn paths_must_be_absolute() {
        assert!(check_path("/etc").is_ok());
        assert!(check_path("etc").is_err());
        assert!(check_path("/a\0b").is_err());
    }
}
```

Add `pub mod files;` to `crates/fleetops-docker/src/lib.rs`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-docker --lib files`
Expected: FAIL to compile with `cannot find function parse_listing` / `extract_file` / `check_path`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-docker/src/files.rs`:

```rust
//! Read-only container file browsing via Docker's archive (tar) API.

use fleetops_core::docker::{DirEntry, DirListing, EntryKind};
use fleetops_core::{Error, Result};
use std::io::Read;

/// A directory listing downloads the whole subtree as a tar stream, so the
/// download is capped; beyond this the listing is marked truncated.
pub const LIST_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const LIST_MAX_ENTRIES: usize = 5_000;

pub fn check_path(path: &str) -> Result<()> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(Error::Invalid(format!("path must be absolute: '{path}'")));
    }
    Ok(())
}

fn normalise(raw: &str) -> &str {
    raw.trim_start_matches("./").trim_start_matches('/')
}

/// Name of the top-level tar entry Docker uses for `path`: the last path
/// component, or "" for `/`.
fn root_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn kind_of(t: tar::EntryType) -> EntryKind {
    match t {
        tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::Link => {
            EntryKind::File
        }
        tar::EntryType::Directory => EntryKind::Dir,
        tar::EntryType::Symlink => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

/// Lists the direct children of `path` from the archive Docker returned.
/// `stream_truncated` means the download stopped at `LIST_MAX_BYTES`, so a
/// parse error at the tail is expected and not fatal.
pub fn parse_listing(tar_bytes: &[u8], path: &str, stream_truncated: bool) -> Result<DirListing> {
    let root = root_name(path);
    let mut entries = Vec::new();
    let mut truncated = stream_truncated;
    let mut archive = tar::Archive::new(tar_bytes);
    let iter = archive
        .entries()
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    for (i, entry) in iter.enumerate() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) if stream_truncated => break,
            Err(e) => return Err(Error::Internal(format!("reading archive: {e}"))),
        };
        let header = entry.header();
        let raw = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let norm = normalise(&raw);
        if i == 0
            && header.entry_type() != tar::EntryType::Directory
            && norm.trim_end_matches('/') == root
        {
            return Err(Error::Invalid(format!("'{path}' is not a directory")));
        }
        let rel = if root.is_empty() {
            norm
        } else {
            match norm.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
                Some(r) => r,
                None => continue,
            }
        };
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() || rel.contains('/') {
            continue;
        }
        if entries.len() >= LIST_MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(DirEntry {
            name: rel.to_string(),
            kind: kind_of(header.entry_type()),
            size: header.size().unwrap_or(0),
            mode: header.mode().unwrap_or(0),
            modified: header.mtime().unwrap_or(0) as i64,
            link_target: entry
                .link_name()
                .ok()
                .flatten()
                .map(|l| l.to_string_lossy().into_owned()),
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Dir, &a.name).cmp(&(b.kind != EntryKind::Dir, &b.name))
    });
    Ok(DirListing {
        path: path.to_string(),
        entries,
        truncated,
    })
}

/// Extracts the single file Docker returned for `path`.
pub fn extract_file(tar_bytes: &[u8], path: &str) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(tar_bytes);
    let mut entries = archive
        .entries()
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    let mut entry = entries
        .next()
        .ok_or_else(|| Error::NotFound(format!("'{path}' not found")))?
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    match entry.header().entry_type() {
        tar::EntryType::Regular | tar::EntryType::Continuous => {}
        tar::EntryType::Directory => {
            return Err(Error::Invalid(format!("'{path}' is a directory")))
        }
        _ => return Err(Error::Invalid(format!("'{path}' is not a regular file"))),
    }
    let mut out = Vec::new();
    entry
        .read_to_end(&mut out)
        .map_err(|e| Error::Internal(format!("reading archive: {e}")))?;
    Ok(out)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-docker && make lint`
Expected: 8 files tests + 5 convert tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-docker
git commit -m "feat(docker): bounded tar-based container file listing and extraction"
```

---

### Task 6: `LocalDocker` operations, lazy-reconnecting environment, factory, daemon tests

**Files:**
- Modify: `crates/fleetops-docker/src/lib.rs` (replace)
- Create: `crates/fleetops-docker/src/error.rs`, `src/containers.rs`, `src/resources.rs`, `src/env.rs`
- Test: `crates/fleetops-docker/tests/docker_it.rs`
- Modify: `Makefile` (add `test-docker`)

**Interfaces:**
- Consumes: Tasks 1, 4, 5.
- Produces:
  - `LocalDocker::connect(url) -> Result<LocalDocker>`: a missing unix socket gives `EnvUnavailable`.
  - `LocalDocker` implements all five op traits, and therefore `DockerOps`.
  - `LocalDockerEnv::new(id, name, url)`:
    - Connects lazily and retries until it succeeds.
    - `status()` has a 5 s timeout.
    - `docker()` returns `None` while unreachable.
  - `DockerEnvFactory: EnvFactory`.
  - `error::map_err(bollard::errors::Error) -> Error` (crate-private).
- Verified bollard 0.21 facts the code relies on:
  - Options are built with `*OptionsBuilder::new()…build()` from `bollard::query_parameters`.
  - Models live in `bollard::models`.
  - `Docker::connect_with_host` accepts `unix://`/`tcp://`/`http://`.
  - Enum `Display` gives the lowercase Docker strings.
  - `BollardDate` is `String` with default features.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-docker/src/error.rs`, containing only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_keep_status_and_message() {
        let e = map_err(bollard::errors::Error::DockerResponseServerError {
            status_code: 409,
            message: "container is running".into(),
        });
        assert!(
            matches!(e, Error::Upstream { status: 409, ref message } if message == "container is running")
        );
    }

    #[test]
    fn transport_errors_mean_unavailable() {
        let e = map_err(bollard::errors::Error::SocketNotFoundError(
            "/nope.sock".into(),
        ));
        assert!(matches!(e, Error::EnvUnavailable(m) if m.contains("/nope.sock")));
        assert!(matches!(
            map_err(bollard::errors::Error::RequestTimeoutError),
            Error::EnvUnavailable(_)
        ));
    }
}
```

`crates/fleetops-docker/src/env.rs`, containing only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_socket_is_down_not_an_error() {
        let env = DockerEnvFactory
            .build(
                1,
                "local",
                EnvKind::DockerLocal,
                "unix:///nonexistent/docker.sock",
            )
            .unwrap();
        assert_eq!(env.name(), "local");
        assert!(env.docker().is_none());
        match env.status().await {
            EnvStatus::Down { reason } => {
                assert!(reason.contains("/nonexistent/docker.sock"), "{reason}")
            }
            other => panic!("expected down, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refused_tcp_endpoint_is_down() {
        let env = LocalDockerEnv::new(1, "tcp", "tcp://127.0.0.1:1");
        assert!(matches!(env.status().await, EnvStatus::Down { .. }));
    }
}
```

`crates/fleetops-docker/tests/docker_it.rs`:

```rust
//! Runs against a real Docker daemon. Enable with `--features docker-it`;
//! the endpoint comes from DOCKER_HOST (see `make test-docker`).
#![cfg(feature = "docker-it")]

use bollard::query_parameters::CreateImageOptionsBuilder;
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, EntryKind, ImageOps, NetworkOps, NetworkSpec,
    PortBinding, SystemOps, VolumeOps, VolumeSpec,
};
use fleetops_core::env::{EnvFactory, EnvKind, EnvStatus};
use fleetops_core::Error;
use fleetops_docker::{DockerEnvFactory, LocalDocker};
use futures_util::StreamExt;
use std::collections::BTreeMap;

const IMAGE: &str = "alpine:3.20";

fn host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into())
}

fn unique(name: &str) -> String {
    format!("dkr-it-{}-{name}", std::process::id())
}

async fn docker() -> LocalDocker {
    let raw = bollard::Docker::connect_with_host(&host()).unwrap();
    let mut pull = raw.create_image(
        Some(
            CreateImageOptionsBuilder::new()
                .from_image("alpine")
                .tag("3.20")
                .build(),
        ),
        None,
        None,
    );
    while let Some(step) = pull.next().await {
        step.expect("pull alpine:3.20");
    }
    LocalDocker::connect(&host()).unwrap()
}

fn sleeper(name: &str) -> ContainerSpec {
    ContainerSpec {
        name: Some(name.to_string()),
        image: IMAGE.into(),
        cmd: Some(vec!["sleep".into(), "300".into()]),
        env: vec!["FLEETOPS_IT=1".into()],
        ports: vec![PortBinding {
            container_port: 8080,
            protocol: "tcp".into(),
            host_ip: None,
            host_port: None,
        }],
        labels: BTreeMap::from([("fleetops.it".into(), "1".into())]),
        restart_policy: Some("no".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn environment_status_is_up() {
    let env = DockerEnvFactory
        .build(1, "local", EnvKind::DockerLocal, &host())
        .unwrap();
    match env.status().await {
        EnvStatus::Up { version } => assert!(!version.api_version.is_empty()),
        other => panic!("expected up, got {other:?}"),
    }
    assert!(env.docker().is_some());
}

#[tokio::test]
async fn system_info_and_disk_usage() {
    let d = docker().await;
    assert!(d.info().await.unwrap()["NCPU"].as_i64().unwrap() >= 1);
    assert!(d.disk_usage().await.unwrap().is_object());
}

#[tokio::test]
async fn container_lifecycle() {
    let d = docker().await;
    let name = unique("life");
    let id = d.create_container(&sleeper(&name)).await.unwrap();
    d.container_action(&id, ContainerAction::Start)
        .await
        .unwrap();

    let listed = d
        .list_containers()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    assert_eq!(listed.name, name);
    assert_eq!(listed.state, "running");
    assert_eq!(
        listed.labels.get("fleetops.it").map(String::as_str),
        Some("1")
    );
    assert!(listed
        .ports
        .iter()
        .any(|p| p.private_port == 8080 && p.public_port.is_some()));

    let spec = d.container_spec(&id).await.unwrap();
    assert_eq!(spec.image, IMAGE);
    assert_eq!(spec.cmd, Some(vec!["sleep".into(), "300".into()]));
    assert!(spec.env.contains(&"FLEETOPS_IT=1".to_string()));
    assert_eq!(spec.ports[0].container_port, 8080);

    d.container_action(&id, ContainerAction::Pause)
        .await
        .unwrap();
    d.container_action(&id, ContainerAction::Unpause)
        .await
        .unwrap();
    d.container_action(&id, ContainerAction::Kill)
        .await
        .unwrap();
    let renamed = unique("life2");
    d.rename_container(&id, &renamed).await.unwrap();
    assert_eq!(
        d.inspect_container(&id).await.unwrap()["Name"],
        format!("/{renamed}")
    );
    d.remove_container(&id, true, false).await.unwrap();

    let err = d.inspect_container(&id).await.unwrap_err();
    assert!(
        matches!(err, Error::Upstream { status: 404, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn advanced_override_reaches_docker() {
    let d = docker().await;
    let spec = ContainerSpec {
        advanced: Some(serde_json::json!({"WorkingDir": "/tmp"})),
        ..sleeper(&unique("adv"))
    };
    let id = d.create_container(&spec).await.unwrap();
    assert_eq!(
        d.inspect_container(&id).await.unwrap()["Config"]["WorkingDir"],
        "/tmp"
    );
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn duplicate_name_is_upstream_conflict() {
    let d = docker().await;
    let name = unique("dup");
    let id = d.create_container(&sleeper(&name)).await.unwrap();
    let err = d.create_container(&sleeper(&name)).await.unwrap_err();
    assert!(
        matches!(err, Error::Upstream { status: 409, .. }),
        "{err:?}"
    );
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn browse_and_read_files() {
    let d = docker().await;
    let id = d
        .create_container(&sleeper(&unique("files")))
        .await
        .unwrap();
    let etc = d.list_dir(&id, "/etc").await.unwrap();
    assert!(etc
        .entries
        .iter()
        .any(|e| e.name == "passwd" && e.kind == EntryKind::File));
    assert!(etc
        .entries
        .iter()
        .any(|e| e.name == "apk" && e.kind == EntryKind::Dir));
    let root = d.list_dir(&id, "/").await.unwrap();
    assert!(root.entries.iter().any(|e| e.name == "bin"));
    let release = d.read_file(&id, "/etc/alpine-release", 1024).await.unwrap();
    assert!(String::from_utf8_lossy(&release).starts_with("3.20"));
    assert!(matches!(
        d.read_file(&id, "/etc/passwd", 4).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.read_file(&id, "/etc", 1 << 20).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.list_dir(&id, "/nope").await,
        Err(Error::Upstream { status: 404, .. })
    ));
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn images_list_tag_and_remove() {
    let d = docker().await;
    let images = d.list_images().await.unwrap();
    let alpine = images
        .iter()
        .find(|i| i.repo_tags.iter().any(|t| t == IMAGE))
        .unwrap();
    assert!(alpine.size > 0);
    assert!(d.inspect_image(IMAGE).await.unwrap()["Id"].is_string());
    assert!(d
        .image_history(IMAGE)
        .await
        .unwrap()
        .as_array()
        .is_some_and(|h| !h.is_empty()));
    let repo = unique("tag");
    d.tag_image(IMAGE, &repo, "it").await.unwrap();
    let removed = d.remove_image(&format!("{repo}:it"), false).await.unwrap();
    assert_eq!(removed, vec![format!("{repo}:it")]);
}

#[tokio::test]
async fn volumes_create_list_remove() {
    let d = docker().await;
    let name = unique("vol");
    let v = d
        .create_volume(&VolumeSpec {
            name: name.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(v.name, name);
    let listed = d
        .list_volumes()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == name)
        .unwrap();
    assert!(!listed.in_use);

    let spec = ContainerSpec {
        binds: vec![format!("{name}:/data")],
        ..sleeper(&unique("volc"))
    };
    let cid = d.create_container(&spec).await.unwrap();
    assert!(
        d.list_volumes()
            .await
            .unwrap()
            .into_iter()
            .find(|v| v.name == name)
            .unwrap()
            .in_use
    );
    assert!(matches!(
        d.remove_volume(&name, false).await,
        Err(Error::Upstream { status: 409, .. })
    ));
    d.remove_container(&cid, true, false).await.unwrap();
    d.remove_volume(&name, false).await.unwrap();
}

#[tokio::test]
async fn networks_create_connect_remove() {
    let d = docker().await;
    let name = unique("net");
    let id = d
        .create_network(&NetworkSpec {
            name: name.clone(),
            subnet: Some("10.231.0.0/24".into()),
            attachable: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let listed = d
        .list_networks()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.id == id)
        .unwrap();
    assert_eq!(listed.subnet.as_deref(), Some("10.231.0.0/24"));
    assert!(listed.attachable);

    let cid = d.create_container(&sleeper(&unique("netc"))).await.unwrap();
    d.container_action(&cid, ContainerAction::Start)
        .await
        .unwrap();
    d.connect_network(&id, &cid).await.unwrap();
    assert!(d.inspect_network(&id).await.unwrap()["Containers"]
        .as_object()
        .is_some_and(|m| m.contains_key(&cid)));
    d.disconnect_network(&id, &cid, true).await.unwrap();
    d.remove_container(&cid, true, false).await.unwrap();
    d.remove_network(&id).await.unwrap();
}
```

Replace `crates/fleetops-docker/src/lib.rs` with:

```rust
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
```

Add to `Makefile`: add `test-docker` to `.PHONY`, and add this target after `test:`. Recipe lines use a TAB.

```make
# Integration tests against the Docker daemon of the current docker context.
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		cargo test -p fleetops-docker --features docker-it --test docker_it
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-docker`
Expected: FAIL to compile with `file not found for module containers` / `resources`, and `cannot find function map_err` / `DockerEnvFactory`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-docker/src/error.rs`:

```rust
use fleetops_core::Error;

/// Docker's own error responses keep their status and message; JSON errors
/// are bugs; everything else is a transport problem, i.e. the environment
/// is unreachable.
pub(crate) fn map_err(e: bollard::errors::Error) -> Error {
    use bollard::errors::Error as B;
    match e {
        B::DockerResponseServerError {
            status_code,
            message,
        } => Error::Upstream {
            status: status_code,
            message,
        },
        e @ (B::JsonDataError { .. } | B::JsonSerdeError { .. }) => {
            Error::Internal(format!("unexpected Docker response: {e}"))
        }
        other => Error::EnvUnavailable(other.to_string()),
    }
}
```

`crates/fleetops-docker/src/containers.rs`:

```rust
use crate::{convert, error::map_err, files, LocalDocker};
use async_trait::async_trait;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder, InspectContainerOptions,
    KillContainerOptions, ListContainersOptionsBuilder, PruneContainersOptions,
    RemoveContainerOptionsBuilder, RenameContainerOptionsBuilder, RestartContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::{Error, Result};
use futures_util::StreamExt;
use serde_json::Value;

impl LocalDocker {
    /// Downloads `path` as a tar stream, stopping after `limit` bytes.
    /// Returns the bytes and whether the limit was hit.
    async fn download(&self, id: &str, path: &str, limit: usize) -> Result<(Vec<u8>, bool)> {
        let opts = DownloadFromContainerOptionsBuilder::new()
            .path(path)
            .build();
        let mut stream = self.client.download_from_container(id, Some(opts));
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_err)?;
            buf.extend_from_slice(&chunk);
            if buf.len() > limit {
                return Ok((buf, true));
            }
        }
        Ok((buf, false))
    }
}

#[async_trait]
impl ContainerOps for LocalDocker {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        let opts = ListContainersOptionsBuilder::new().all(true).build();
        let list = self
            .client
            .list_containers(Some(opts))
            .await
            .map_err(map_err)?;
        Ok(list.into_iter().map(convert::container_summary).collect())
    }

    async fn inspect_container(&self, id: &str) -> Result<Value> {
        let res = self
            .client
            .inspect_container(id, None::<InspectContainerOptions>)
            .await
            .map_err(map_err)?;
        serde_json::to_value(res).map_err(|e| Error::Internal(e.to_string()))
    }

    async fn container_spec(&self, id: &str) -> Result<ContainerSpec> {
        Ok(convert::inspect_to_spec(&self.inspect_container(id).await?))
    }

    async fn create_container(&self, spec: &ContainerSpec) -> Result<String> {
        spec.validate()?;
        let body = serde_json::from_value(convert::spec_to_body(spec))
            .map_err(|e| Error::Invalid(format!("invalid container configuration: {e}")))?;
        let mut opts = CreateContainerOptionsBuilder::new();
        if let Some(name) = &spec.name {
            opts = opts.name(name);
        }
        let res = self
            .client
            .create_container(Some(opts.build()), body)
            .await
            .map_err(map_err)?;
        Ok(res.id)
    }

    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        let c = &self.client;
        match action {
            ContainerAction::Start => c.start_container(id, None::<StartContainerOptions>).await,
            ContainerAction::Stop => c.stop_container(id, None::<StopContainerOptions>).await,
            ContainerAction::Restart => {
                c.restart_container(id, None::<RestartContainerOptions>)
                    .await
            }
            ContainerAction::Kill => c.kill_container(id, None::<KillContainerOptions>).await,
            ContainerAction::Pause => c.pause_container(id).await,
            ContainerAction::Unpause => c.unpause_container(id).await,
        }
        .map_err(map_err)
    }

    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        fleetops_core::docker::validate_container_name(new_name)?;
        let opts = RenameContainerOptionsBuilder::new().name(new_name).build();
        self.client
            .rename_container(id, opts)
            .await
            .map_err(map_err)
    }

    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> Result<()> {
        let opts = RemoveContainerOptionsBuilder::new()
            .force(force)
            .v(volumes)
            .build();
        self.client
            .remove_container(id, Some(opts))
            .await
            .map_err(map_err)
    }

    async fn prune_containers(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_containers(None::<PruneContainersOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.containers_deleted.unwrap_or_default(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }

    async fn list_dir(&self, id: &str, path: &str) -> Result<DirListing> {
        files::check_path(path)?;
        let (bytes, truncated) = self.download(id, path, files::LIST_MAX_BYTES).await?;
        files::parse_listing(&bytes, path, truncated)
    }

    async fn read_file(&self, id: &str, path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        files::check_path(path)?;
        // Allow for tar headers and padding on top of the file itself.
        let limit = (max_bytes as usize).saturating_add(64 * 1024);
        let (bytes, over) = self.download(id, path, limit).await?;
        if over {
            return Err(Error::Invalid(format!(
                "file is larger than {max_bytes} bytes"
            )));
        }
        let data = files::extract_file(&bytes, path)?;
        if data.len() as u64 > max_bytes {
            return Err(Error::Invalid(format!(
                "file is larger than {max_bytes} bytes"
            )));
        }
        Ok(data)
    }
}
```

`crates/fleetops-docker/src/resources.rs`:

```rust
//! System, image, volume and network operations.

use crate::{convert, error::map_err, LocalDocker};
use async_trait::async_trait;
use bollard::models::{
    Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
    VolumeCreateRequest,
};
use bollard::query_parameters::{
    DataUsageOptions, InspectNetworkOptions, ListImagesOptionsBuilder, ListNetworksOptions,
    ListVolumesOptions, PruneImagesOptionsBuilder, PruneNetworksOptions, PruneVolumesOptions,
    RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder, TagImageOptionsBuilder,
};
use fleetops_core::docker::{
    ContainerOps, EngineVersion, ImageOps, ImageSummary, NetworkOps, NetworkSpec, NetworkSummary,
    PruneReport, SystemOps, VolumeOps, VolumeSpec, VolumeSummary,
};
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn to_json<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}

#[async_trait]
impl SystemOps for LocalDocker {
    async fn version(&self) -> Result<EngineVersion> {
        let v = self.client.version().await.map_err(map_err)?;
        Ok(EngineVersion {
            version: v.version.unwrap_or_default(),
            api_version: v.api_version.unwrap_or_default(),
            os: v.os.unwrap_or_default(),
            arch: v.arch.unwrap_or_default(),
        })
    }

    async fn info(&self) -> Result<Value> {
        to_json(self.client.info().await.map_err(map_err)?)
    }

    async fn disk_usage(&self) -> Result<Value> {
        to_json(
            self.client
                .df(None::<DataUsageOptions>)
                .await
                .map_err(map_err)?,
        )
    }
}

#[async_trait]
impl ImageOps for LocalDocker {
    async fn list_images(&self) -> Result<Vec<ImageSummary>> {
        let opts = ListImagesOptionsBuilder::new().all(false).build();
        let images = self.client.list_images(Some(opts)).await.map_err(map_err)?;
        let used: HashSet<String> = self
            .list_containers()
            .await?
            .into_iter()
            .map(|c| c.image_id)
            .collect();
        Ok(images
            .into_iter()
            .map(|i| ImageSummary {
                in_use: used.contains(&i.id),
                id: i.id,
                repo_tags: i.repo_tags,
                size: i.size,
                created: i.created,
            })
            .collect())
    }

    async fn inspect_image(&self, id: &str) -> Result<Value> {
        to_json(self.client.inspect_image(id).await.map_err(map_err)?)
    }

    async fn image_history(&self, id: &str) -> Result<Value> {
        to_json(self.client.image_history(id).await.map_err(map_err)?)
    }

    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()> {
        let opts = TagImageOptionsBuilder::new().repo(repo).tag(tag).build();
        self.client.tag_image(id, Some(opts)).await.map_err(map_err)
    }

    async fn remove_image(&self, id: &str, force: bool) -> Result<Vec<String>> {
        let opts = RemoveImageOptionsBuilder::new().force(force).build();
        let items = self
            .client
            .remove_image(id, Some(opts), None)
            .await
            .map_err(map_err)?;
        Ok(items
            .into_iter()
            .filter_map(|i| i.untagged.or(i.deleted))
            .collect())
    }

    async fn prune_images(&self, all: bool) -> Result<PruneReport> {
        let mut opts = PruneImagesOptionsBuilder::new();
        if all {
            opts = opts.filters(&HashMap::from([("dangling", vec!["false"])]));
        }
        let res = self
            .client
            .prune_images(Some(opts.build()))
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res
                .images_deleted
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| i.deleted.or(i.untagged))
                .collect(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }
}

#[async_trait]
impl VolumeOps for LocalDocker {
    async fn list_volumes(&self) -> Result<Vec<VolumeSummary>> {
        let res = self
            .client
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(map_err)?;
        let used: HashSet<String> = self
            .list_containers()
            .await?
            .into_iter()
            .flat_map(|c| c.volumes)
            .collect();
        Ok(res
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| convert::volume_summary(v, &used))
            .collect())
    }

    async fn inspect_volume(&self, name: &str) -> Result<Value> {
        to_json(self.client.inspect_volume(name).await.map_err(map_err)?)
    }

    async fn create_volume(&self, spec: &VolumeSpec) -> Result<VolumeSummary> {
        let req = VolumeCreateRequest {
            name: Some(spec.name.clone()).filter(|n| !n.is_empty()),
            driver: spec.driver.clone(),
            driver_opts: Some(spec.driver_opts.clone().into_iter().collect()),
            labels: Some(spec.labels.clone().into_iter().collect()),
            ..Default::default()
        };
        let v = self.client.create_volume(req).await.map_err(map_err)?;
        Ok(convert::volume_summary(v, &HashSet::new()))
    }

    async fn remove_volume(&self, name: &str, force: bool) -> Result<()> {
        let opts = RemoveVolumeOptionsBuilder::new().force(force).build();
        self.client
            .remove_volume(name, Some(opts))
            .await
            .map_err(map_err)
    }

    async fn prune_volumes(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_volumes(None::<PruneVolumesOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.volumes_deleted.unwrap_or_default(),
            space_reclaimed: res.space_reclaimed.unwrap_or(0),
        })
    }
}

#[async_trait]
impl NetworkOps for LocalDocker {
    async fn list_networks(&self) -> Result<Vec<NetworkSummary>> {
        let list = self
            .client
            .list_networks(None::<ListNetworksOptions>)
            .await
            .map_err(map_err)?;
        Ok(list.into_iter().map(convert::network_summary).collect())
    }

    async fn inspect_network(&self, id: &str) -> Result<Value> {
        to_json(
            self.client
                .inspect_network(id, None::<InspectNetworkOptions>)
                .await
                .map_err(map_err)?,
        )
    }

    async fn create_network(&self, spec: &NetworkSpec) -> Result<String> {
        if spec.name.trim().is_empty() {
            return Err(Error::Invalid("network name is required".into()));
        }
        let ipam = (spec.subnet.is_some() || spec.gateway.is_some()).then(|| Ipam {
            config: Some(vec![IpamConfig {
                subnet: spec.subnet.clone(),
                gateway: spec.gateway.clone(),
                ..Default::default()
            }]),
            ..Default::default()
        });
        let req = NetworkCreateRequest {
            name: spec.name.clone(),
            driver: spec.driver.clone(),
            internal: Some(spec.internal),
            attachable: Some(spec.attachable),
            ipam,
            labels: Some(spec.labels.clone().into_iter().collect()),
            ..Default::default()
        };
        Ok(self.client.create_network(req).await.map_err(map_err)?.id)
    }

    async fn remove_network(&self, id: &str) -> Result<()> {
        self.client.remove_network(id).await.map_err(map_err)
    }

    async fn connect_network(&self, network: &str, container: &str) -> Result<()> {
        let req = NetworkConnectRequest {
            container: container.to_string(),
            endpoint_config: None,
        };
        self.client
            .connect_network(network, req)
            .await
            .map_err(map_err)
    }

    async fn disconnect_network(&self, network: &str, container: &str, force: bool) -> Result<()> {
        let req = NetworkDisconnectRequest {
            container: container.to_string(),
            force: Some(force),
        };
        self.client
            .disconnect_network(network, req)
            .await
            .map_err(map_err)
    }

    async fn prune_networks(&self) -> Result<PruneReport> {
        let res = self
            .client
            .prune_networks(None::<PruneNetworksOptions>)
            .await
            .map_err(map_err)?;
        Ok(PruneReport {
            deleted: res.networks_deleted.unwrap_or_default(),
            space_reclaimed: 0,
        })
    }
}
```

Prepend to `crates/fleetops-docker/src/env.rs`:

```rust
use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvFactory, EnvKind, EnvStatus, Environment};
use fleetops_core::Result;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// A Docker endpoint registered as an environment. The connection is made
/// lazily and retried on each use until it succeeds, so an environment whose
/// socket appears later (Docker restarted) recovers on its own.
pub struct LocalDockerEnv {
    id: i64,
    name: String,
    url: String,
    client: Mutex<Option<Arc<LocalDocker>>>,
}

impl LocalDockerEnv {
    pub fn new(id: i64, name: &str, url: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            url: url.to_string(),
            client: Mutex::new(None),
        }
    }

    fn connect(&self) -> Result<Arc<LocalDocker>> {
        let mut slot = self.client.lock().unwrap();
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let c = Arc::new(LocalDocker::connect(&self.url)?);
        *slot = Some(c.clone());
        Ok(c)
    }
}

#[async_trait]
impl Environment for LocalDockerEnv {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::DockerLocal
    }

    async fn status(&self) -> EnvStatus {
        let client = match self.connect() {
            Ok(c) => c,
            Err(e) => {
                return EnvStatus::Down {
                    reason: e.to_string(),
                }
            }
        };
        match tokio::time::timeout(STATUS_TIMEOUT, client.version()).await {
            Ok(Ok(version)) => EnvStatus::Up { version },
            Ok(Err(e)) => EnvStatus::Down {
                reason: e.to_string(),
            },
            Err(_) => EnvStatus::Down {
                reason: format!("no response from {} within {STATUS_TIMEOUT:?}", self.url),
            },
        }
    }

    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        self.connect().ok().map(|c| c as Arc<dyn DockerOps>)
    }
}

pub struct DockerEnvFactory;

impl EnvFactory for DockerEnvFactory {
    fn build(&self, id: i64, name: &str, kind: EnvKind, url: &str) -> Result<Arc<dyn Environment>> {
        match kind {
            EnvKind::DockerLocal => Ok(Arc::new(LocalDockerEnv::new(id, name, url))),
        }
    }
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-docker && make lint`
Expected: 17 unit tests PASS. `docker_it` compiles to 0 tests without the feature.

- [ ] **Step 5: Run the daemon integration tests**

Run: `make test-docker && cargo clippy -p fleetops-docker --features docker-it --all-targets -- -D warnings`
Expected:
- 9 PASS: `environment_status_is_up`, `system_info_and_disk_usage`, `container_lifecycle`, `advanced_override_reaches_docker`, `duplicate_name_is_upstream_conflict`, `browse_and_read_files`, `images_list_tag_and_remove`, `volumes_create_list_remove`, `networks_create_connect_remove`.
- Clippy clean.
- `docker ps -a --filter name=dkr-it` shows nothing left behind.

The first run pulls `alpine:3.20`. If there is no Docker daemon, record that in the ledger and continue; the unit tests are the gate.

- [ ] **Step 6: Commit**

```bash
git add Makefile crates/fleetops-docker
git commit -m "feat(docker): bollard-backed Docker operations and lazy-reconnecting environment"
```

---

### Task 7: API environment registry, authorization, bootstrap and fake Docker

**Files:**
- Create: `crates/fleetops-api/src/envs.rs`, `crates/fleetops-api/src/authz.rs`, `crates/fleetops-api/tests/common/fake_docker.rs`
- Modify: `crates/fleetops-api/Cargo.toml`, `crates/fleetops-api/src/lib.rs`, `crates/fleetops-api/src/state.rs`, `crates/fleetops-api/tests/common/mod.rs`
- Modify: `crates/fleetops-server/Cargo.toml`, `crates/fleetops-server/src/lib.rs`, `crates/fleetops-server/tests/static_files.rs`
- Test: `crates/fleetops-api/tests/envs.rs` (bootstrap part), plus the unit test in `authz.rs`

**Interfaces:**
- Consumes: `EnvFactory`, `Environment`, `EnvStatus`, `EnvKind` (Task 1); `EnvironmentRecord` and the store environment CRUD (Task 3); `DockerEnvFactory` (Task 6).
- Produces:
  - `fleetops_api::envs::EnvRegistry` with `new(Arc<dyn EnvFactory>)`, `load(&Store)`, `insert(&EnvironmentRecord) -> Result<Arc<dyn Environment>>`, `remove(name)`, `get(name) -> Result<Arc<dyn Environment>>` (`NotFound("environment '<name>'")`).
  - `envs::bootstrap_local(&Store, &EnvRegistry, url) -> Result<Option<EnvironmentRecord>>` and `envs::LOCAL_ENV_NAME = "local"`.
  - `fleetops_api::authz::{Action{EnvRead,EnvManage,DockerRead,DockerWrite}, authorize(&User, Option<i64>, Action) -> Result<()>}`.
  - `AppState.envs: Arc<EnvRegistry>`. **Breaking:** `AppState::new(store, config, factory: Arc<dyn EnvFactory>)`.
  - Test support:
    - `TestApp{router, store, state, fake: Arc<FakeDocker>}`, with `add_env(&admin, name)` and `with_local() -> Session`.
    - `fake_docker::{FakeDocker, FakeFactory, summary(id, name, state, project)}`.
    - `FakeDocker.reachable: AtomicBool` toggles Up/Down.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleetops-api/src/authz.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: Role) -> User {
        User {
            id: 1,
            username: "u".into(),
            role,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn admins_can_do_everything_users_cannot_manage_envs() {
        for a in [
            Action::EnvRead,
            Action::EnvManage,
            Action::DockerRead,
            Action::DockerWrite,
        ] {
            assert!(authorize(&user(Role::Admin), Some(1), a).is_ok());
        }
        assert!(authorize(&user(Role::User), Some(1), Action::DockerWrite).is_ok());
        assert!(authorize(&user(Role::User), None, Action::EnvRead).is_ok());
        assert!(matches!(
            authorize(&user(Role::User), None, Action::EnvManage),
            Err(Error::Forbidden(_))
        ));
    }
}
```

`crates/fleetops-api/tests/envs.rs` (P2a Task 7 part; Task 8 extends it):

```rust
mod common;

use common::*;
use fleetops_api::envs::bootstrap_local;
use std::sync::atomic::Ordering;
```

```rust
#[tokio::test]
async fn bootstrap_registers_reachable_local_once() {
    let app = app().await;
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert_eq!(rec.unwrap().name, "local");
    assert!(app.state.envs.get("local").is_ok());
    let again = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(again.is_none());
    assert_eq!(app.store.list_environments().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bootstrap_skips_unreachable_endpoint() {
    let app = app().await;
    app.fake.reachable.store(false, Ordering::SeqCst);
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(rec.is_none());
    assert!(app.store.list_environments().await.unwrap().is_empty());
}
```

`crates/fleetops-api/tests/common/fake_docker.rs`:

```rust
//! In-memory stand-in for a Docker daemon, used by the API tests.

use async_trait::async_trait;
use fleetops_core::docker::*;
use fleetops_core::env::{EnvFactory, EnvKind, EnvStatus, Environment};
use fleetops_core::{Error, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
pub struct FakeState {
    pub containers: Vec<ContainerSummary>,
    pub volumes: Vec<VolumeSummary>,
    pub networks: Vec<NetworkSummary>,
    pub calls: Vec<String>,
    next_id: usize,
}

#[derive(Default)]
pub struct FakeDocker {
    pub state: Mutex<FakeState>,
    /// When false, the environment reports Down and `docker()` is None.
    pub reachable: AtomicBool,
}

fn not_found(kind: &str, id: &str) -> Error {
    Error::Upstream {
        status: 404,
        message: format!("No such {kind}: {id}"),
    }
}

pub fn summary(id: &str, name: &str, state: &str, project: Option<&str>) -> ContainerSummary {
    ContainerSummary {
        id: id.into(),
        name: name.into(),
        image: "alpine:3.20".into(),
        image_id: "sha256:alpine".into(),
        state: state.into(),
        status: String::new(),
        created: 0,
        ports: vec![],
        labels: BTreeMap::new(),
        compose_project: project.map(Into::into),
        compose_service: project.map(|_| name.to_string()),
        volumes: vec![],
    }
}

impl FakeDocker {
    pub fn new() -> Arc<Self> {
        let f = FakeDocker::default();
        f.reachable.store(true, Ordering::SeqCst);
        Arc::new(f)
    }

    pub fn add(&self, c: ContainerSummary) {
        self.state.lock().unwrap().containers.push(c);
    }

    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn container(&self, id: &str) -> Option<ContainerSummary> {
        self.state
            .lock()
            .unwrap()
            .containers
            .iter()
            .find(|c| c.id == id || c.name == id)
            .cloned()
    }

    fn call(&self, s: String) {
        self.state.lock().unwrap().calls.push(s);
    }
}

#[async_trait]
impl SystemOps for FakeDocker {
    async fn version(&self) -> Result<EngineVersion> {
        Ok(EngineVersion {
            version: "29.0.0".into(),
            api_version: "1.54".into(),
            os: "linux".into(),
            arch: "arm64".into(),
        })
    }
    async fn info(&self) -> Result<Value> {
        Ok(json!({"NCPU": 2, "Name": "fake"}))
    }
    async fn disk_usage(&self) -> Result<Value> {
        Ok(json!({"ImageUsage": {}}))
    }
}

#[async_trait]
impl ContainerOps for FakeDocker {
    async fn list_containers(&self) -> Result<Vec<ContainerSummary>> {
        Ok(self.state.lock().unwrap().containers.clone())
    }
    async fn inspect_container(&self, id: &str) -> Result<Value> {
        let c = self
            .container(id)
            .ok_or_else(|| not_found("container", id))?;
        Ok(
            json!({"Id": c.id, "Name": format!("/{}", c.name), "State": {"Running": c.state == "running"}}),
        )
    }
    async fn container_spec(&self, id: &str) -> Result<ContainerSpec> {
        let c = self
            .container(id)
            .ok_or_else(|| not_found("container", id))?;
        Ok(ContainerSpec {
            name: Some(c.name),
            image: c.image,
            ..Default::default()
        })
    }
    async fn create_container(&self, spec: &ContainerSpec) -> Result<String> {
        self.call(format!("create {}", spec.name.as_deref().unwrap_or("")));
        let mut st = self.state.lock().unwrap();
        if let Some(name) = &spec.name {
            if st.containers.iter().any(|c| &c.name == name) {
                return Err(Error::Upstream {
                    status: 409,
                    message: format!("Conflict. The container name \"/{name}\" is already in use"),
                });
            }
        }
        st.next_id += 1;
        let id = format!("new{}", st.next_id);
        let mut c = summary(&id, spec.name.as_deref().unwrap_or(&id), "created", None);
        c.image = spec.image.clone();
        c.labels = spec.labels.clone();
        st.containers.push(c);
        Ok(id)
    }
    async fn container_action(&self, id: &str, action: ContainerAction) -> Result<()> {
        self.call(format!("{} {id}", action.as_str()));
        let mut st = self.state.lock().unwrap();
        let c = st
            .containers
            .iter_mut()
            .find(|c| c.id == id || c.name == id)
            .ok_or_else(|| not_found("container", id))?;
        c.state = match action {
            ContainerAction::Start | ContainerAction::Restart | ContainerAction::Unpause => {
                "running"
            }
            ContainerAction::Stop | ContainerAction::Kill => "exited",
            ContainerAction::Pause => "paused",
        }
        .into();
        Ok(())
    }
    async fn rename_container(&self, id: &str, new_name: &str) -> Result<()> {
        self.call(format!("rename {id} {new_name}"));
        let mut st = self.state.lock().unwrap();
        let c = st
            .containers
            .iter_mut()
            .find(|c| c.id == id || c.name == id)
            .ok_or_else(|| not_found("container", id))?;
        c.name = new_name.into();
        Ok(())
    }
    async fn remove_container(&self, id: &str, force: bool, _volumes: bool) -> Result<()> {
        self.call(format!("remove {id}"));
        let mut st = self.state.lock().unwrap();
        let idx = st
            .containers
            .iter()
            .position(|c| c.id == id || c.name == id)
            .ok_or_else(|| not_found("container", id))?;
        if st.containers[idx].state == "running" && !force {
            return Err(Error::Upstream {
                status: 409,
                message: "cannot remove a running container".into(),
            });
        }
        st.containers.remove(idx);
        Ok(())
    }
    async fn prune_containers(&self) -> Result<PruneReport> {
        let mut st = self.state.lock().unwrap();
        let deleted: Vec<String> = st
            .containers
            .iter()
            .filter(|c| c.state == "exited")
            .map(|c| c.id.clone())
            .collect();
        st.containers.retain(|c| c.state != "exited");
        Ok(PruneReport {
            deleted,
            space_reclaimed: 0,
        })
    }
    async fn list_dir(&self, id: &str, path: &str) -> Result<DirListing> {
        self.container(id)
            .ok_or_else(|| not_found("container", id))?;
        Ok(DirListing {
            path: path.into(),
            entries: vec![DirEntry {
                name: "hosts".into(),
                kind: EntryKind::File,
                size: 5,
                mode: 0o644,
                modified: 0,
                link_target: None,
            }],
            truncated: false,
        })
    }
    async fn read_file(&self, id: &str, _path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        self.container(id)
            .ok_or_else(|| not_found("container", id))?;
        let data = b"hello".to_vec();
        if data.len() as u64 > max_bytes {
            return Err(Error::Invalid("too big".into()));
        }
        Ok(data)
    }
}

#[async_trait]
impl ImageOps for FakeDocker {
    async fn list_images(&self) -> Result<Vec<ImageSummary>> {
        Ok(vec![ImageSummary {
            id: "sha256:alpine".into(),
            repo_tags: vec!["alpine:3.20".into()],
            size: 1,
            created: 0,
            in_use: true,
        }])
    }
    async fn inspect_image(&self, id: &str) -> Result<Value> {
        if id != "sha256:alpine" {
            return Err(not_found("image", id));
        }
        Ok(json!({"Id": id}))
    }
    async fn image_history(&self, id: &str) -> Result<Value> {
        Ok(json!([{"Id": id}]))
    }
    async fn tag_image(&self, id: &str, repo: &str, tag: &str) -> Result<()> {
        self.call(format!("tag {id} {repo}:{tag}"));
        Ok(())
    }
    async fn remove_image(&self, id: &str, _force: bool) -> Result<Vec<String>> {
        self.call(format!("rmi {id}"));
        Ok(vec![id.to_string()])
    }
    async fn prune_images(&self, all: bool) -> Result<PruneReport> {
        self.call(format!("prune images all={all}"));
        Ok(PruneReport::default())
    }
}

#[async_trait]
impl VolumeOps for FakeDocker {
    async fn list_volumes(&self) -> Result<Vec<VolumeSummary>> {
        Ok(self.state.lock().unwrap().volumes.clone())
    }
    async fn inspect_volume(&self, name: &str) -> Result<Value> {
        let st = self.state.lock().unwrap();
        let v = st
            .volumes
            .iter()
            .find(|v| v.name == name)
            .ok_or_else(|| not_found("volume", name))?;
        Ok(json!({"Name": v.name}))
    }
    async fn create_volume(&self, spec: &VolumeSpec) -> Result<VolumeSummary> {
        let v = VolumeSummary {
            name: spec.name.clone(),
            driver: spec.driver.clone().unwrap_or_else(|| "local".into()),
            mountpoint: format!("/var/lib/docker/volumes/{}/_data", spec.name),
            created_at: None,
            labels: spec.labels.clone(),
            in_use: false,
        };
        self.state.lock().unwrap().volumes.push(v.clone());
        Ok(v)
    }
    async fn remove_volume(&self, name: &str, _force: bool) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let before = st.volumes.len();
        st.volumes.retain(|v| v.name != name);
        if st.volumes.len() == before {
            return Err(not_found("volume", name));
        }
        Ok(())
    }
    async fn prune_volumes(&self) -> Result<PruneReport> {
        Ok(PruneReport::default())
    }
}

#[async_trait]
impl NetworkOps for FakeDocker {
    async fn list_networks(&self) -> Result<Vec<NetworkSummary>> {
        Ok(self.state.lock().unwrap().networks.clone())
    }
    async fn inspect_network(&self, id: &str) -> Result<Value> {
        let st = self.state.lock().unwrap();
        let n = st
            .networks
            .iter()
            .find(|n| n.id == id || n.name == id)
            .ok_or_else(|| not_found("network", id))?;
        Ok(json!({"Id": n.id, "Name": n.name}))
    }
    async fn create_network(&self, spec: &NetworkSpec) -> Result<String> {
        let mut st = self.state.lock().unwrap();
        let id = format!("n{}", st.networks.len() + 1);
        st.networks.push(NetworkSummary {
            id: id.clone(),
            name: spec.name.clone(),
            driver: spec.driver.clone().unwrap_or_else(|| "bridge".into()),
            scope: "local".into(),
            internal: spec.internal,
            attachable: spec.attachable,
            subnet: spec.subnet.clone(),
            gateway: spec.gateway.clone(),
        });
        Ok(id)
    }
    async fn remove_network(&self, id: &str) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let before = st.networks.len();
        st.networks.retain(|n| n.id != id && n.name != id);
        if st.networks.len() == before {
            return Err(not_found("network", id));
        }
        Ok(())
    }
    async fn connect_network(&self, network: &str, container: &str) -> Result<()> {
        self.call(format!("connect {network} {container}"));
        Ok(())
    }
    async fn disconnect_network(&self, network: &str, container: &str, force: bool) -> Result<()> {
        self.call(format!("disconnect {network} {container} force={force}"));
        Ok(())
    }
    async fn prune_networks(&self) -> Result<PruneReport> {
        Ok(PruneReport::default())
    }
}

pub struct FakeEnv {
    id: i64,
    name: String,
    docker: Arc<FakeDocker>,
}

#[async_trait]
impl Environment for FakeEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        EnvKind::DockerLocal
    }
    async fn status(&self) -> EnvStatus {
        if self.docker.reachable.load(Ordering::SeqCst) {
            EnvStatus::Up {
                version: self.docker.version().await.unwrap(),
            }
        } else {
            EnvStatus::Down {
                reason: "fake is unreachable".into(),
            }
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        self.docker
            .reachable
            .load(Ordering::SeqCst)
            .then(|| self.docker.clone() as Arc<dyn DockerOps>)
    }
}

/// Every environment built by this factory shares one FakeDocker.
pub struct FakeFactory(pub Arc<FakeDocker>);

impl EnvFactory for FakeFactory {
    fn build(
        &self,
        id: i64,
        name: &str,
        _kind: EnvKind,
        _url: &str,
    ) -> Result<Arc<dyn Environment>> {
        Ok(Arc::new(FakeEnv {
            id,
            name: name.into(),
            docker: self.0.clone(),
        }))
    }
}
```

Update `crates/fleetops-api/tests/common/mod.rs`:
- Add `pub mod fake_docker;` right after `#![allow(dead_code)]`.
- Add `use fake_docker::{FakeDocker, FakeFactory};`.
- Change `use std::net::SocketAddr;` to `use std::{net::SocketAddr, sync::Arc};`.
- Then make `TestApp`/`app_with` and the new helpers match:

```rust
pub struct TestApp {
    pub router: Router,
    pub store: Store,
    pub state: AppState,
    pub fake: Arc<FakeDocker>,
}
```

```rust
pub async fn app_with(config: ApiConfig) -> TestApp {
    let store = Store::in_memory().await.unwrap();
    let fake = FakeDocker::new();
    let state = AppState::new(store.clone(), config, Arc::new(FakeFactory(fake.clone())));
    let router = fleetops_api::app(state.clone(), Router::new());
    TestApp {
        router,
        store,
        state,
        fake,
    }
}
```

Inside `impl TestApp`, before `create_user`:

```rust
    /// Registers environment `name` (backed by `self.fake`) as `admin`.
    pub async fn add_env(&self, admin: &Session, name: &str) {
        let r = self
            .send(req(
                "POST",
                "/api/v1/envs",
                Some(json!({"name": name, "kind": "docker-local", "url": "unix:///fake.sock"})),
                Some(admin),
            ))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    }

    /// An admin session with environment "local" registered.
    pub async fn with_local(&self) -> Session {
        let admin = self.setup_admin().await;
        self.add_env(&admin, "local").await;
        admin
    }
```

Add `async-trait.workspace = true` under `[dev-dependencies]` in `crates/fleetops-api/Cargo.toml`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --lib authz && cargo test -p fleetops-api --test envs`
Expected: FAIL to compile with `cannot find type Action`, `unresolved import fleetops_api::envs`, and `AppState::new takes 2 arguments`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-api/src/authz.rs`:

```rust
//! The single authorization decision point. Sub-project 5 replaces the body
//! with per-environment role bindings; handlers do not change.

use fleetops_core::{Error, Result, Role, User};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// List environments and read their status.
    EnvRead,
    /// Create, change or delete environments.
    EnvManage,
    /// Read Docker resources in an environment.
    DockerRead,
    /// Change Docker resources in an environment.
    DockerWrite,
}

pub fn authorize(user: &User, _env_id: Option<i64>, action: Action) -> Result<()> {
    match (user.role, action) {
        (Role::Admin, _) => Ok(()),
        (Role::User, Action::EnvManage) => Err(Error::Forbidden("admin role required".into())),
        (Role::User, _) => Ok(()),
    }
}
```

`crates/fleetops-api/src/envs.rs`:

```rust
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
}

impl EnvRegistry {
    pub fn new(factory: Arc<dyn EnvFactory>) -> Self {
        Self {
            factory,
            envs: RwLock::new(HashMap::new()),
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
```

In `crates/fleetops-api/src/lib.rs`, add after `pub mod auth;`:

```rust
pub mod authz;
pub mod envs;
```

Replace `crates/fleetops-api/src/state.rs` with:

```rust
use crate::{
    auth::{limiter::LoginLimiter, setup::SetupGuard},
    envs::EnvRegistry,
};
use fleetops_core::env::EnvFactory;
use fleetops_store::Store;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub const LOGIN_MAX_FAILURES: u32 = 5;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// How long after process start first-run setup stays open.
    pub setup_window: Duration,
    /// Send HSTS on TLS responses. Off by default because the HTTP listener
    /// is always on and HSTS would break it for browsers.
    pub hsts: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            setup_window: Duration::from_secs(600),
            hsts: false,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: Arc<ApiConfig>,
    pub limiter: Arc<LoginLimiter>,
    pub setup: Arc<SetupGuard>,
    pub envs: Arc<EnvRegistry>,
}

impl AppState {
    /// Call `envs.load(&store)` afterwards to build stored environments.
    pub fn new(store: Store, config: ApiConfig, factory: Arc<dyn EnvFactory>) -> Self {
        let setup = SetupGuard::new(Instant::now(), config.setup_window);
        Self {
            store,
            config: Arc::new(config),
            limiter: Arc::new(LoginLimiter::new(LOGIN_MAX_FAILURES, LOGIN_WINDOW)),
            setup: Arc::new(setup),
            envs: Arc::new(EnvRegistry::new(factory)),
        }
    }
}
```

Server wiring. Add `fleetops-docker.workspace = true` after `fleetops-api.workspace = true` in `crates/fleetops-server/Cargo.toml`. Then in `crates/fleetops-server/src/lib.rs`:
- Change the imports to:

```rust
use fleetops_api::{envs::bootstrap_local, ApiConfig, AppState, IsTls};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use std::{net::SocketAddr, sync::Arc, time::Duration};
```

- Replace `run` with:

```rust
pub async fn run(config: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data)?;
    let store = Store::open(&config.data.join("fleetops.db")).await?;
    let state = AppState::new(
        store.clone(),
        ApiConfig {
            hsts: config.hsts,
            ..ApiConfig::default()
        },
        Arc::new(DockerEnvFactory),
    );
    state.envs.load(&store).await?;
    if let Some(env) = bootstrap_local(&store, &state.envs, &config.docker_host).await? {
        tracing::info!(url = %env.url, "registered local Docker environment");
    }
    let app = fleetops_api::app(state, static_files::router());
    tokio::spawn(maintenance::run(store));
    serve(&config, app).await
}
```

In `crates/fleetops-server/tests/static_files.rs`, change `AppState::new(store, ApiConfig::default())` to:

```rust
        AppState::new(
            store,
            ApiConfig::default(),
            std::sync::Arc::new(fleetops_docker::DockerEnvFactory),
        ),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test --workspace && make lint`
Expected: the `authz` unit test and 2 `envs` tests PASS, and every P1 test still passes. The helper change keeps P1 tests source-compatible.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api crates/fleetops-server Cargo.lock
git commit -m "feat(api): environment registry, authorize() and local bootstrap"
```

---

### Task 8: Environment routes

**Files:**
- Create: `crates/fleetops-api/src/routes/envs.rs`
- Modify: `crates/fleetops-api/src/routes/mod.rs`
- Test: `crates/fleetops-api/tests/envs.rs` (extend)

**Interfaces:**
- Consumes: `EnvRegistry`, `authorize` (Task 7); store environment CRUD (Task 3); `ApiJson`/`ApiPath` (P1).
- Produces:
  - `GET /envs` → `[EnvironmentRecord]` (EnvRead).
  - `POST /envs {name, kind, url}` → 201 (EnvManage). Duplicate gives 409; bad name/url gives 400; unknown kind gives 422.
  - `PATCH /envs/{env} {name?, url?}` → record. The registry entry is rebuilt under the new name.
  - `DELETE /envs/{env}` → 204.
  - `GET /envs/{env}/status` → `EnvStatus`.
  - Audit actions: `env.create`, `env.update`, `env.delete` (target = env name).

- [ ] **Step 1: Write the failing tests**

Replace the header of `crates/fleetops-api/tests/envs.rs` (everything above the first test) with:

```rust
mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_api::envs::bootstrap_local;
use serde_json::json;
use std::sync::atomic::Ordering;
```

Append:

```rust
#[tokio::test]
async fn admin_manages_environments() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.add_env(&admin, "local").await;

    let list = app
        .send(req("GET", "/api/v1/envs", None, Some(&admin)))
        .await;
    assert_eq!(list.json[0]["name"], "local");
    assert_eq!(list.json[0]["kind"], "docker-local");

    let r = app
        .send(req(
            "PATCH",
            "/api/v1/envs/local",
            Some(json!({"name": "home", "url": "tcp://10.0.0.2:2375"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["url"], "tcp://10.0.0.2:2375");
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/home/status", None, Some(&admin)))
            .await
            .status,
        StatusCode::OK
    );

    let r = app
        .send(req("DELETE", "/api/v1/envs/home", None, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/home/containers",
            None,
            Some(&admin)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(&actions[..3], &["env.delete", "env.update", "env.create"]);
}

#[tokio::test]
async fn environment_input_is_validated() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let bad_name = json!({"name": "a/b", "kind": "docker-local", "url": "unix:///x.sock"});
    let bad_url = json!({"name": "x", "kind": "docker-local", "url": "ssh://host"});
    let bad_kind = json!({"name": "x", "kind": "k8s", "url": "unix:///x.sock"});
    for (body, status) in [(bad_name, 400), (bad_url, 400), (bad_kind, 422)] {
        let r = app
            .send(req("POST", "/api/v1/envs", Some(body), Some(&admin)))
            .await;
        assert_eq!(r.status.as_u16(), status, "{:?}", r.json);
    }
    app.add_env(&admin, "local").await;
    let dup = app
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "local", "kind": "docker-local", "url": "unix:///y.sock"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn users_can_read_but_not_manage_environments() {
    let app = app().await;
    let admin = app.with_local().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    assert_eq!(
        app.send(req("GET", "/api/v1/envs", None, Some(&bob)))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/status", None, Some(&bob)))
            .await
            .status,
        StatusCode::OK
    );
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "x", "kind": "docker-local", "url": "unix:///x.sock"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(req("DELETE", "/api/v1/envs/local", None, Some(&bob)))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn status_reports_up_and_down() {
    let app = app().await;
    let admin = app.with_local().await;
    let r = app
        .send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
        .await;
    assert_eq!(r.json["state"], "up");
    assert_eq!(r.json["version"]["api_version"], "1.54");

    app.fake.reachable.store(false, Ordering::SeqCst);
    let r = app
        .send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
        .await;
    assert_eq!(r.json["state"], "down");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.json["code"], "env_unavailable");
}

#[tokio::test]
async fn environments_survive_restart() {
    let app = app().await;
    let admin = app.with_local().await;
    drop(admin);
    // A fresh registry over the same store sees the stored environment.
    let state = fleetops_api::AppState::new(
        app.store.clone(),
        fleetops_api::ApiConfig::default(),
        std::sync::Arc::new(common::fake_docker::FakeFactory(app.fake.clone())),
    );
    assert!(state.envs.get("local").is_err());
    state.envs.load(&app.store).await.unwrap();
    assert_eq!(state.envs.get("local").unwrap().name(), "local");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test envs`
Expected: the 5 new tests FAIL because `add_env` asserts 201 but gets 404.

Note: `status_reports_up_and_down` also calls `/envs/local/containers`, which arrives in Task 9. Mark it `#[ignore = "needs Task 9"]` now and remove the attribute in Task 9 Step 4.

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/routes/envs.rs`:

```rust
use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    extract::{ApiJson, ApiPath},
    routes::audit,
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::env::{validate_env_name, validate_env_url, EnvKind, EnvStatus};
use fleetops_core::now_unix;
use fleetops_store::EnvironmentRecord;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateEnv {
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
}

#[derive(Deserialize)]
pub struct UpdateEnv {
    pub name: Option<String>,
    pub url: Option<String>,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
) -> ApiResult<Json<Vec<EnvironmentRecord>>> {
    authorize(&cu.user, None, Action::EnvRead)?;
    Ok(Json(s.store.list_environments().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiJson(body): ApiJson<CreateEnv>,
) -> ApiResult<(StatusCode, Json<EnvironmentRecord>)> {
    authorize(&cu.user, None, Action::EnvManage)?;
    validate_env_name(&body.name)?;
    validate_env_url(&body.url)?;
    let rec = s
        .store
        .create_environment(&body.name, body.kind.as_str(), &body.url, now_unix())
        .await?;
    s.envs.insert(&rec)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.create",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok((StatusCode::CREATED, Json(rec)))
}

async fn find(s: &AppState, name: &str) -> ApiResult<EnvironmentRecord> {
    let env = s.envs.get(name)?;
    Ok(s.store.get_environment(env.id()).await?)
}

pub async fn update(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<UpdateEnv>,
) -> ApiResult<Json<EnvironmentRecord>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let current = find(&s, &name).await?;
    let new_name = body.name.unwrap_or(current.name.clone());
    let new_url = body.url.unwrap_or(current.url.clone());
    validate_env_name(&new_name)?;
    validate_env_url(&new_url)?;
    let rec = s
        .store
        .update_environment(current.id, &new_name, &new_url, now_unix())
        .await?;
    s.envs.remove(&current.name);
    s.envs.insert(&rec)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.update",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok(Json(rec))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<StatusCode> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let current = find(&s, &name).await?;
    s.store.delete_environment(current.id).await?;
    s.envs.remove(&current.name);
    audit(
        &s,
        Some(cu.user.id),
        "env.delete",
        "environment",
        &current.name,
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn status(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<EnvStatus>> {
    let env = s.envs.get(&name)?;
    authorize(&cu.user, Some(env.id()), Action::EnvRead)?;
    Ok(Json(env.status().await))
}
```

In `crates/fleetops-api/src/routes/mod.rs`:
- Add `pub mod envs;` (keep the module list alphabetical).
- Add `.merge(env_routes())` right after the `.route("/audit", …)` line in `api()`.
- Add this function below `api()`:

```rust
fn env_routes() -> Router<AppState> {
    Router::new()
        .route("/envs", get(envs::list).post(envs::create))
        .route("/envs/{env}", patch(envs::update).delete(envs::remove))
        .route("/envs/{env}/status", get(envs::status))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `envs` 6 PASS, 1 ignored.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): environment CRUD and status routes"
```

---

### Task 9: Docker handler context, system info and container routes

**Files:**
- Create: `crates/fleetops-api/src/routes/docker/mod.rs`, `crates/fleetops-api/src/routes/docker/system.rs`, `crates/fleetops-api/src/routes/docker/containers.rs`
- Modify: `crates/fleetops-api/src/routes/mod.rs`, `crates/fleetops-api/tests/envs.rs` (un-ignore)
- Test: `crates/fleetops-api/tests/containers.rs`, `crates/fleetops-api/tests/resources.rs` (system part)

**Interfaces:**
- Consumes: `EnvRegistry`, `authorize` (Task 7); `DockerOps` (Task 1); `recreate` (Task 2).
- Produces:
  - `routes::docker::ctx(&AppState, CurrentUser, env_name, Action) -> ApiResult<EnvCtx>`. An unknown env gives 404; an unreachable env gives 503 `env_unavailable`.
  - `EnvCtx{user, env, ops}.audit(&s, action, target_type, target_id, &Result<T>)`.
  - `ForceQuery{force, volumes}`.
  - Routes:

```
GET    /envs/{env}/info | /envs/{env}/df
GET    /envs/{env}/containers                 POST {ContainerSpec…, start=true} → 201 {id}
POST   /envs/{env}/containers/prune           → PruneReport
GET    /envs/{env}/containers/{id}            DELETE ?force&volumes → 204
GET    /envs/{env}/containers/{id}/spec
POST   /envs/{env}/containers/{id}/rename     {name} → 204
POST   /envs/{env}/containers/{id}/recreate   ContainerSpec → {id, old_removed, backup_name}
POST   /envs/{env}/containers/{id}/duplicate  {name} → 201 {id} (started)
GET    /envs/{env}/containers/{id}/files?path=           → DirListing
GET    /envs/{env}/containers/{id}/files/download?path=  → octet-stream attachment (≤100 MiB)
POST   /envs/{env}/containers/{id}/{start|stop|restart|kill|pause|unpause} → 204
```

- Audit actions: `container.{create,start,stop,restart,kill,pause,unpause,rename,remove,prune,recreate,duplicate}`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-api/tests/containers.rs`:

```rust
mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;

#[tokio::test]
async fn create_start_list_and_remove() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"name": "web", "image": "nginx:1.27"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let id = r.json["id"].as_str().unwrap().to_string();
    assert_eq!(app.fake.container(&id).unwrap().state, "running");

    let list = app
        .send(req("GET", "/api/v1/envs/local/containers", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["name"], "web");

    let running = app
        .send(req(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(
        running.status,
        StatusCode::CONFLICT,
        "docker's 409 passes through"
    );
    assert_eq!(running.json["message"], "cannot remove a running container");
    let r = app
        .send(req(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}?force=true"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn create_without_start_and_validation() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine", "start": false})),
            Some(&s),
        ))
        .await;
    assert_eq!(
        app.fake
            .container(r.json["id"].as_str().unwrap())
            .unwrap()
            .state,
        "created"
    );
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": ""})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "a", "name": "bad name"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lifecycle_actions_and_unknown_action() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c9", "db", "exited", None));
    for (action, state) in [
        ("start", "running"),
        ("pause", "paused"),
        ("unpause", "running"),
        ("restart", "running"),
        ("stop", "exited"),
        ("kill", "exited"),
    ] {
        let r = app
            .send(req(
                "POST",
                &format!("/api/v1/envs/local/containers/c9/{action}"),
                None,
                Some(&s),
            ))
            .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{action}");
        assert_eq!(app.fake.container("c9").unwrap().state, state, "{action}");
    }
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c9/explode",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/missing/start",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json["code"], "upstream");
}

#[tokio::test]
async fn mutations_are_audited_with_environment_and_failures() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "exited", None));
    app.send(req(
        "POST",
        "/api/v1/envs/local/containers/c1/start",
        None,
        Some(&s),
    ))
    .await;
    app.send(req(
        "POST",
        "/api/v1/envs/local/containers/missing/stop",
        None,
        Some(&s),
    ))
    .await;
    let rows = app.store.list_audit(2, None).await.unwrap();
    assert_eq!(
        (rows[0].action.as_str(), rows[0].result.as_str()),
        ("container.stop", "failure")
    );
    assert!(rows[0].detail.as_ref().unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("No such container"));
    assert_eq!(
        (rows[1].action.as_str(), rows[1].result.as_str()),
        ("container.start", "success")
    );
    assert!(rows[1].environment_id.is_some());
    assert_eq!(rows[1].target_id.as_deref(), Some("c1"));
}

#[tokio::test]
async fn inspect_spec_rename() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["Name"], "/web");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/spec",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["name"], "web");
    assert_eq!(r.json["image"], "alpine:3.20");
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/rename",
            Some(json!({"name": "web2"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.fake.container("c1").unwrap().name, "web2");
}

#[tokio::test]
async fn recreate_replaces_container() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/recreate",
            Some(json!({"image": "nginx:2"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let new_id = r.json["id"].as_str().unwrap();
    assert_eq!(r.json["old_removed"], true);
    let new = app.fake.container(new_id).unwrap();
    assert_eq!(
        (new.name.as_str(), new.image.as_str(), new.state.as_str()),
        ("web", "nginx:2", "running")
    );
    assert!(app.fake.container("c1").is_none());
}

#[tokio::test]
async fn duplicate_creates_and_starts_copy() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/duplicate",
            Some(json!({"name": "web-copy"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let copy = app.fake.container(r.json["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        (copy.name.as_str(), copy.state.as_str()),
        ("web-copy", "running")
    );
    let dup = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/c1/duplicate",
            Some(json!({"name": "web-copy"})),
            Some(&s),
        ))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn prune_removes_stopped() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "a", "exited", None));
    app.fake.add(summary("c2", "b", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers/prune",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["deleted"], json!(["c1"]));
    assert!(app.fake.container("c2").is_some());
}

#[tokio::test]
async fn files_list_and_download() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/files?path=/etc",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["entries"][0]["name"], "hosts");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers/c1/files/download?path=/etc/ho%22sts",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json, json!("hello"));
    assert_eq!(
        r.headers.get("content-disposition").unwrap(),
        "attachment; filename=\"hosts\""
    );
    assert_eq!(
        r.headers.get("content-type").unwrap(),
        "application/octet-stream"
    );
}

#[tokio::test]
async fn users_can_manage_containers_unauthenticated_cannot() {
    let app = app().await;
    let admin = app.with_local().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = app
        .send(req("GET", "/api/v1/envs/local/containers", None, None))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let no_csrf = Session {
        csrf: String::new(),
        ..bob
    };
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({"image": "alpine"})),
            Some(&no_csrf),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unknown_environment_is_404() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req("GET", "/api/v1/envs/nope/containers", None, Some(&s)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.json["message"].as_str().unwrap().contains("nope"));
}
```

`crates/fleetops-api/tests/resources.rs` (system part; Task 10 extends it):

```rust
mod common;

use common::*;
```

```rust
#[tokio::test]
async fn system_info_and_df() {
    let app = app().await;
    let s = app.with_local().await;
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/info", None, Some(&s)))
            .await
            .json["NCPU"],
        2
    );
    assert!(app
        .send(req("GET", "/api/v1/envs/local/df", None, Some(&s)))
        .await
        .json
        .is_object());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test containers --test resources`
Expected: all FAIL with 404s from the missing routes (e.g. `create_start_list_and_remove`: left 404, right 201).

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/routes/docker/mod.rs`. In this task the module list holds only `containers` and `system`; Tasks 10 and 11 add the rest.

```rust
//! Docker resource endpoints under `/envs/{env}/...`.

pub mod containers;
pub mod system;

use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    ApiResult, AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// Everything a Docker handler needs: the authorised user, the environment
/// and its Docker operations.
pub struct EnvCtx {
    pub user: CurrentUser,
    pub env: Arc<dyn Environment>,
    pub ops: Arc<dyn DockerOps>,
}

pub async fn ctx(
    s: &AppState,
    user: CurrentUser,
    env_name: &str,
    action: Action,
) -> ApiResult<EnvCtx> {
    let env = s.envs.get(env_name)?;
    authorize(&user.user, Some(env.id()), action)?;
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{env_name}' is unreachable")))?;
    Ok(EnvCtx { user, env, ops })
}

impl EnvCtx {
    /// Writes an audit row for a Docker mutation, recording failures with
    /// the error message. Audit failures are logged, never surfaced.
    pub async fn audit<T>(
        &self,
        s: &AppState,
        action: &str,
        target_type: &str,
        target_id: &str,
        outcome: &Result<T>,
    ) {
        let (result, detail) = match outcome {
            Ok(_) => ("success", None),
            Err(e) => ("failure", Some(json!({ "error": e.to_string() }))),
        };
        let entry = NewAudit {
            ts: now_unix(),
            user_id: Some(self.user.user.id),
            environment_id: Some(self.env.id()),
            action,
            target_type: Some(target_type),
            target_id: Some(target_id),
            result,
            detail,
        };
        if let Err(e) = s.store.insert_audit(&entry).await {
            tracing::error!(error = %e, action, "failed to write audit log");
        }
    }
}

#[derive(Deserialize, Default)]
pub struct ForceQuery {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub volumes: bool,
}
```

`crates/fleetops-api/src/routes/docker/system.rs`:

```rust
use super::ctx;
use crate::{auth::extract::CurrentUser, authz::Action, extract::ApiPath, ApiResult, AppState};
use axum::{extract::State, Json};
use serde_json::Value;

pub async fn info(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.info().await?))
}

pub async fn disk_usage(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.disk_usage().await?))
}
```

`crates/fleetops-api/src/routes/docker/containers.rs`:

```rust
use super::{ctx, ForceQuery};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fleetops_core::docker::{
    ContainerAction, ContainerSpec, ContainerSummary, DirListing, PruneReport,
};
use fleetops_core::recreate::recreate;
use fleetops_core::{now_unix, Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest file the browser will download through the API.
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(flatten)]
    pub spec: ContainerSpec,
    /// Start the container after creating it (default true).
    #[serde(default = "yes")]
    pub start: bool,
}

#[derive(Serialize)]
pub struct Created {
    pub id: String,
}

#[derive(Deserialize)]
pub struct NameBody {
    pub name: String,
}

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct RecreateResult {
    pub id: String,
    pub old_removed: bool,
    pub backup_name: String,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ContainerSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_containers().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    body.spec.validate()?;
    let target = body
        .spec
        .name
        .clone()
        .unwrap_or_else(|| body.spec.image.clone());
    let res = c.ops.create_container(&body.spec).await;
    c.audit(&s, "container.create", "container", &target, &res)
        .await;
    let id = res?;
    if body.start {
        let started = c.ops.container_action(&id, ContainerAction::Start).await;
        c.audit(&s, "container.start", "container", &id, &started)
            .await;
        started?;
    }
    Ok((StatusCode::CREATED, Json(Created { id })))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_container(&id).await?))
}

pub async fn spec(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<ContainerSpec>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.container_spec(&id).await?))
}

pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id, action)): ApiPath<(String, String, String)>,
) -> ApiResult<StatusCode> {
    let action: ContainerAction = action.parse()?;
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.container_action(&id, action).await;
    c.audit(
        &s,
        &format!("container.{}", action.as_str()),
        "container",
        &id,
        &res,
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rename(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.rename_container(&id, &body.name).await;
    c.audit(&s, "container.rename", "container", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_container(&id, q.force, q.volumes).await;
    c.audit(&s, "container.remove", "container", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_containers().await;
    c.audit(&s, "container.prune", "container", "*", &res).await;
    Ok(Json(res?))
}

pub async fn recreate_container(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(spec): ApiJson<ContainerSpec>,
) -> ApiResult<Json<RecreateResult>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = recreate(&*c.ops, &id, &spec, now_unix()).await;
    c.audit(&s, "container.recreate", "container", &id, &res)
        .await;
    let out = res?;
    Ok(Json(RecreateResult {
        id: out.new_id,
        old_removed: out.old_removed,
        backup_name: out.backup_name,
    }))
}

pub async fn duplicate(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NameBody>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let mut spec = c.ops.container_spec(&id).await?;
    spec.name = Some(body.name);
    spec.validate()?;
    let res = async {
        let new_id = c.ops.create_container(&spec).await?;
        c.ops
            .container_action(&new_id, ContainerAction::Start)
            .await?;
        Ok::<_, Error>(new_id)
    }
    .await;
    c.audit(&s, "container.duplicate", "container", &id, &res)
        .await;
    Ok((StatusCode::CREATED, Json(Created { id: res? })))
}

pub async fn list_files(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<DirListing>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_dir(&id, &q.path).await?))
}

pub async fn download_file(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Response> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    let data = c.ops.read_file(&id, &q.path, MAX_DOWNLOAD_BYTES).await?;
    let file_name: String = q
        .path
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\')
        .collect();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        if file_name.is_empty() {
            "file"
        } else {
            &file_name
        }
    );
    let mut res = data.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}
```

In `crates/fleetops-api/src/routes/mod.rs`, add `pub mod docker;` and replace `env_routes` with the version below. Static segments (`prune`, `spec`, `rename`, …) take priority over `{action}`/`{id}` in axum 0.8.

```rust
fn env_routes() -> Router<AppState> {
    use docker::{containers as c, system};
    Router::new()
        .route("/envs", get(envs::list).post(envs::create))
        .route("/envs/{env}", patch(envs::update).delete(envs::remove))
        .route("/envs/{env}/status", get(envs::status))
        .route("/envs/{env}/info", get(system::info))
        .route("/envs/{env}/df", get(system::disk_usage))
        .route("/envs/{env}/containers", get(c::list).post(c::create))
        .route("/envs/{env}/containers/prune", post(c::prune))
        .route("/envs/{env}/containers/{id}", get(c::inspect).delete(c::remove))
        .route("/envs/{env}/containers/{id}/spec", get(c::spec))
        .route("/envs/{env}/containers/{id}/rename", post(c::rename))
        .route("/envs/{env}/containers/{id}/recreate", post(c::recreate_container))
        .route("/envs/{env}/containers/{id}/duplicate", post(c::duplicate))
        .route("/envs/{env}/containers/{id}/files", get(c::list_files))
        .route("/envs/{env}/containers/{id}/files/download", get(c::download_file))
        .route("/envs/{env}/containers/{id}/{action}", post(c::action))
}
```

- [ ] **Step 4: Un-ignore the Task 8 test**

Remove `#[ignore = "needs Task 9"]` from `status_reports_up_and_down` in `crates/fleetops-api/tests/envs.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `containers` 11 PASS, `resources` 1 PASS, `envs` 7 PASS with 0 ignored.

- [ ] **Step 6: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): container management, file browsing and system routes"
```

---

### Task 10: Image, volume and network routes

**Files:**
- Create: `crates/fleetops-api/src/routes/docker/images.rs`, `volumes.rs`, `networks.rs`
- Modify: `crates/fleetops-api/src/routes/docker/mod.rs`, `crates/fleetops-api/src/routes/mod.rs`
- Test: `crates/fleetops-api/tests/resources.rs` (extend)

**Interfaces:**
- Consumes: `ctx`, `EnvCtx::audit`, `ForceQuery` (Task 9).
- Produces:

```
GET  /envs/{env}/images                    POST /envs/{env}/images/prune[?all=true] → PruneReport
GET  /envs/{env}/images/{id}               DELETE ?force → {"removed": [refs]}
GET  /envs/{env}/images/{id}/history       POST /envs/{env}/images/{id}/tag {repo, tag="latest"} → 204
GET  /envs/{env}/volumes                   POST VolumeSpec → 201 VolumeSummary
POST /envs/{env}/volumes/prune             GET|DELETE(?force) /envs/{env}/volumes/{name}
GET  /envs/{env}/networks                  POST NetworkSpec → 201 {id}
POST /envs/{env}/networks/prune            GET|DELETE /envs/{env}/networks/{id}
POST /envs/{env}/networks/{id}/connect     {container} → 204
POST /envs/{env}/networks/{id}/disconnect  {container, force=false} → 204
```

- Audit actions: `image.{tag,remove,prune}`, `volume.{create,remove,prune}`, `network.{create,remove,connect,disconnect,prune}`.
- Image routes take an image **ID** (e.g. `sha256:…`). References with `/` cannot be a single path segment.

- [ ] **Step 1: Write the failing tests**

Replace the header of `crates/fleetops-api/tests/resources.rs` with:

```rust
mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;
```

Append:

```rust
#[tokio::test]
async fn images_routes() {
    let app = app().await;
    let s = app.with_local().await;
    let list = app
        .send(req("GET", "/api/v1/envs/local/images", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["repo_tags"], json!(["alpine:3.20"]));
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:nope",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert!(app
        .send(req(
            "GET",
            "/api/v1/envs/local/images/sha256:alpine/history",
            None,
            Some(&s)
        ))
        .await
        .json
        .is_array());
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/images/sha256:alpine/tag",
            Some(json!({"repo": "me/alpine"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = app
        .send(req(
            "DELETE",
            "/api/v1/envs/local/images/sha256:alpine?force=true",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["removed"], json!(["sha256:alpine"]));
    app.send(req(
        "POST",
        "/api/v1/envs/local/images/prune?all=true",
        None,
        Some(&s),
    ))
    .await;
    app.send(req(
        "POST",
        "/api/v1/envs/local/images/prune",
        None,
        Some(&s),
    ))
    .await;
    let calls = app.fake.calls();
    assert!(calls.contains(&"tag sha256:alpine me/alpine:latest".to_string()));
    assert!(calls.contains(&"prune images all=true".to_string()));
    assert!(calls.contains(&"prune images all=false".to_string()));
}

#[tokio::test]
async fn volumes_routes() {
    let app = app().await;
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/volumes",
            Some(json!({"name": "data", "labels": {"a": "b"}})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    assert_eq!(r.json["driver"], "local");
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/volumes", None, Some(&s)))
            .await
            .json[0]["name"],
        "data"
    );
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .json["Name"],
        "data"
    );
    assert_eq!(
        app.send(req(
            "DELETE",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(req(
            "DELETE",
            "/api/v1/envs/local/volumes/data",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/volumes/prune",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn networks_routes() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake.add(summary("c1", "web", "running", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/networks",
            Some(json!({"name": "backend", "subnet": "10.1.0.0/24", "attachable": true})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let id = r.json["id"].as_str().unwrap().to_string();
    let list = app
        .send(req("GET", "/api/v1/envs/local/networks", None, Some(&s)))
        .await;
    assert_eq!(list.json[0]["subnet"], "10.1.0.0/24");
    assert_eq!(
        app.send(req(
            "GET",
            &format!("/api/v1/envs/local/networks/{id}"),
            None,
            Some(&s)
        ))
        .await
        .json["Name"],
        "backend"
    );
    let r = app
        .send(req(
            "POST",
            &format!("/api/v1/envs/local/networks/{id}/connect"),
            Some(json!({"container": "c1"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = app
        .send(req(
            "POST",
            &format!("/api/v1/envs/local/networks/{id}/disconnect"),
            Some(json!({"container": "c1", "force": true})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(app
        .fake
        .calls()
        .contains(&format!("disconnect {id} c1 force=true")));
    assert_eq!(
        app.send(req(
            "DELETE",
            &format!("/api/v1/envs/local/networks/{id}"),
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/networks/prune",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::OK
    );
    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    for a in [
        "network.create",
        "network.connect",
        "network.disconnect",
        "network.remove",
        "network.prune",
    ] {
        assert!(actions.contains(&a.to_string()), "{a} not audited");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test resources`
Expected: the 3 new tests FAIL with 404 statuses or empty lists.

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/routes/docker/images.rs`:

```rust
use super::{ctx, ForceQuery};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::docker::{ImageSummary, PruneReport};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct TagBody {
    pub repo: String,
    #[serde(default = "latest")]
    pub tag: String,
}

fn latest() -> String {
    "latest".into()
}

#[derive(Deserialize, Default)]
pub struct PruneQuery {
    /// Also remove unused tagged images, not just dangling ones.
    #[serde(default)]
    pub all: bool,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ImageSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_images().await?))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_image(&id).await?))
}

pub async fn history(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.image_history(&id).await?))
}

pub async fn tag(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<TagBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.tag_image(&id, &body.repo, &body.tag).await;
    c.audit(&s, "image.tag", "image", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_image(&id, q.force).await;
    c.audit(&s, "image.remove", "image", &id, &res).await;
    Ok(Json(json!({ "removed": res? })))
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    Query(q): Query<PruneQuery>,
) -> ApiResult<Json<PruneReport>> {
    let all = q.all;
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_images(all).await;
    c.audit(
        &s,
        "image.prune",
        "image",
        if all { "all" } else { "dangling" },
        &res,
    )
    .await;
    Ok(Json(res?))
}
```

`crates/fleetops-api/src/routes/docker/volumes.rs`:

```rust
use super::{ctx, ForceQuery};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::docker::{PruneReport, VolumeSpec, VolumeSummary};
use serde_json::Value;

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<VolumeSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_volumes().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(spec): ApiJson<VolumeSpec>,
) -> ApiResult<(StatusCode, Json<VolumeSummary>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.create_volume(&spec).await;
    c.audit(&s, "volume.create", "volume", &spec.name, &res)
        .await;
    Ok((StatusCode::CREATED, Json(res?)))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_volume(&name).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    Query(q): Query<ForceQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_volume(&name, q.force).await;
    c.audit(&s, "volume.remove", "volume", &name, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_volumes().await;
    c.audit(&s, "volume.prune", "volume", "*", &res).await;
    Ok(Json(res?))
}
```

`crates/fleetops-api/src/routes/docker/networks.rs`:

```rust
use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::docker::{NetworkSpec, NetworkSummary, PruneReport};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub struct AttachBody {
    pub container: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct CreatedNetwork {
    pub id: String,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<NetworkSummary>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.list_networks().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(spec): ApiJson<NetworkSpec>,
) -> ApiResult<(StatusCode, Json<CreatedNetwork>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.create_network(&spec).await;
    c.audit(&s, "network.create", "network", &spec.name, &res)
        .await;
    Ok((StatusCode::CREATED, Json(CreatedNetwork { id: res? })))
}

pub async fn inspect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(c.ops.inspect_network(&id).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.remove_network(&id).await;
    c.audit(&s, "network.remove", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn connect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.connect_network(&id, &body.container).await;
    c.audit(&s, "network.connect", "network", &id, &res).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn disconnect(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<AttachBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c
        .ops
        .disconnect_network(&id, &body.container, body.force)
        .await;
    c.audit(&s, "network.disconnect", "network", &id, &res)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn prune(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<PruneReport>> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let res = c.ops.prune_networks().await;
    c.audit(&s, "network.prune", "network", "*", &res).await;
    Ok(Json(res?))
}
```

In `routes/docker/mod.rs`, make the module list:

```rust
pub mod containers;
pub mod images;
pub mod networks;
pub mod system;
pub mod volumes;
```

In `routes/mod.rs` `env_routes`:
- Change the `use` line to `use docker::{containers as c, images as i, networks as n, system, volumes as v};`.
- Append these routes after the last container route:

```rust
        .route("/envs/{env}/images", get(i::list))
        .route("/envs/{env}/images/prune", post(i::prune))
        .route("/envs/{env}/images/{id}", get(i::inspect).delete(i::remove))
        .route("/envs/{env}/images/{id}/history", get(i::history))
        .route("/envs/{env}/images/{id}/tag", post(i::tag))
        .route("/envs/{env}/volumes", get(v::list).post(v::create))
        .route("/envs/{env}/volumes/prune", post(v::prune))
        .route("/envs/{env}/volumes/{name}", get(v::inspect).delete(v::remove))
        .route("/envs/{env}/networks", get(n::list).post(n::create))
        .route("/envs/{env}/networks/prune", post(n::prune))
        .route("/envs/{env}/networks/{id}", get(n::inspect).delete(n::remove))
        .route("/envs/{env}/networks/{id}/connect", post(n::connect))
        .route("/envs/{env}/networks/{id}/disconnect", post(n::disconnect))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `resources` 4 PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): image, volume and network routes"
```

---

### Task 11: Read-only Compose projects

**Files:**
- Create: `crates/fleetops-api/src/compose.rs`, `crates/fleetops-api/src/routes/docker/compose.rs`
- Modify: `crates/fleetops-api/src/lib.rs`, `crates/fleetops-api/src/routes/docker/mod.rs`, `crates/fleetops-api/src/routes/mod.rs`
- Test: `crates/fleetops-api/tests/compose.rs`, plus the unit test in `compose.rs`

**Interfaces:**
- Consumes: `ContainerSummary.compose_project/compose_service` (Task 1); `ctx` (Task 9).
- Produces:
  - `compose::{ComposeProject{name, services, containers, running, status: running|partial|stopped}, group_projects(&[ContainerSummary]), project_members(&[ContainerSummary], &str)}`.
  - `GET /envs/{env}/compose-projects` → `[ComposeProject]`, sorted by name.
  - `POST /envs/{env}/compose-projects/{project}/{start|stop|restart|down}` → `{affected, errors}`. Every member is attempted; `down` force-removes the members.
  - An unknown project or action gives 404.
  - Audit action: `compose.<action>`, target `compose-project`.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleetops-api/src/compose.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, project: Option<&str>, service: &str, state: &str) -> ContainerSummary {
        ContainerSummary {
            id: id.into(),
            name: id.into(),
            image: "img".into(),
            image_id: "sha".into(),
            state: state.into(),
            status: String::new(),
            created: 0,
            ports: vec![],
            labels: BTreeMap::new(),
            compose_project: project.map(Into::into),
            compose_service: Some(service.into()),
            volumes: vec![],
        }
    }

    #[test]
    fn groups_and_summarises_projects() {
        let list = vec![
            c("1", Some("shop"), "web", "running"),
            c("2", Some("shop"), "db", "exited"),
            c("3", Some("blog"), "web", "running"),
            c("4", Some("blog"), "web", "running"),
            c("5", None, "x", "running"),
            c("6", Some("old"), "job", "exited"),
        ];
        let p = group_projects(&list);
        assert_eq!(
            p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["blog", "old", "shop"]
        );
        assert_eq!(p[0].services, vec!["web"]);
        assert_eq!(
            (p[0].containers, p[0].running, p[0].status),
            (2, 2, "running")
        );
        assert_eq!(p[1].status, "stopped");
        assert_eq!(p[2].services, vec!["db", "web"]);
        assert_eq!(p[2].status, "partial");
        assert_eq!(project_members(&list, "shop").len(), 2);
        assert!(project_members(&list, "nope").is_empty());
    }
}
```

Add `pub mod compose;` to `crates/fleetops-api/src/lib.rs` after `pub mod authz;`.

`crates/fleetops-api/tests/compose.rs`:

```rust
mod common;

use axum::http::StatusCode;
use common::fake_docker::summary;
use common::*;
use serde_json::json;

#[tokio::test]
async fn lists_projects() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "running", Some("shop")));
    app.fake
        .add(summary("b", "shop-db-1", "exited", Some("shop")));
    app.fake.add(summary("c", "lonely", "running", None));
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/compose-projects",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json.as_array().unwrap().len(), 1);
    assert_eq!(r.json[0]["name"], "shop");
    assert_eq!(r.json[0]["status"], "partial");
    assert_eq!(r.json[0]["containers"], 2);
}

#[tokio::test]
async fn project_actions_apply_to_members_only() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "exited", Some("shop")));
    app.fake
        .add(summary("b", "shop-db-1", "exited", Some("shop")));
    app.fake.add(summary("c", "other", "exited", None));
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/start",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json, json!({"affected": 2, "errors": []}));
    assert_eq!(app.fake.container("a").unwrap().state, "running");
    assert_eq!(app.fake.container("c").unwrap().state, "exited");

    let r = app
        .send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/down",
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(r.json["affected"], 2);
    assert!(app.fake.container("a").is_none() && app.fake.container("b").is_none());
    assert!(app.fake.container("c").is_some());
    let audit = app.store.list_audit(1, None).await.unwrap();
    assert_eq!(
        (audit[0].action.as_str(), audit[0].target_id.as_deref()),
        ("compose.down", Some("shop"))
    );
}

#[tokio::test]
async fn unknown_project_or_action_is_404() {
    let app = app().await;
    let s = app.with_local().await;
    app.fake
        .add(summary("a", "shop-web-1", "exited", Some("shop")));
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/nope/start",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(req(
            "POST",
            "/api/v1/envs/local/compose-projects/shop/explode",
            None,
            Some(&s)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --lib compose && cargo test -p fleetops-api --test compose`
Expected: the lib test FAILS to compile (`cannot find function group_projects`). After that is fixed, the integration tests FAIL with 404.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-api/src/compose.rs`:

```rust
//! Read-only Compose project view: containers grouped by the
//! `com.docker.compose.project` label.

use fleetops_core::docker::ContainerSummary;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeProject {
    pub name: String,
    pub services: Vec<String>,
    pub containers: usize,
    pub running: usize,
    /// running | partial | stopped
    pub status: &'static str,
}

pub fn group_projects(containers: &[ContainerSummary]) -> Vec<ComposeProject> {
    let mut by_name: BTreeMap<&str, Vec<&ContainerSummary>> = BTreeMap::new();
    for c in containers {
        if let Some(p) = c.compose_project.as_deref() {
            by_name.entry(p).or_default().push(c);
        }
    }
    by_name
        .into_iter()
        .map(|(name, members)| {
            let running = members.iter().filter(|c| c.state == "running").count();
            let mut services: Vec<String> = members
                .iter()
                .filter_map(|c| c.compose_service.clone())
                .collect();
            services.sort();
            services.dedup();
            let status = match running {
                0 => "stopped",
                n if n == members.len() => "running",
                _ => "partial",
            };
            ComposeProject {
                name: name.to_string(),
                services,
                containers: members.len(),
                running,
                status,
            }
        })
        .collect()
}

/// Container ids belonging to `project`.
pub fn project_members<'a>(
    containers: &'a [ContainerSummary],
    project: &str,
) -> Vec<&'a ContainerSummary> {
    containers
        .iter()
        .filter(|c| c.compose_project.as_deref() == Some(project))
        .collect()
}
```

`crates/fleetops-api/src/routes/docker/compose.rs`:

```rust
use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    compose::{group_projects, project_members, ComposeProject},
    extract::ApiPath,
    ApiResult, AppState,
};
use axum::{extract::State, Json};
use fleetops_core::docker::ContainerAction;
use fleetops_core::Error;
use serde_json::{json, Value};

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ComposeProject>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(group_projects(&c.ops.list_containers().await?)))
}

/// start | stop | restart | down (down removes the project's containers).
/// Every member is attempted; per-container failures are reported, not fatal.
pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, project, action)): ApiPath<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let op = match action.as_str() {
        "start" => Some(ContainerAction::Start),
        "stop" => Some(ContainerAction::Stop),
        "restart" => Some(ContainerAction::Restart),
        "down" => None,
        other => return Err(Error::NotFound(format!("unknown compose action '{other}'")).into()),
    };
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let all = c.ops.list_containers().await?;
    let members = project_members(&all, &project);
    if members.is_empty() {
        return Err(Error::NotFound(format!("compose project '{project}'")).into());
    }
    let mut errors = Vec::new();
    for m in &members {
        let res = match op {
            Some(a) => c.ops.container_action(&m.id, a).await,
            None => c.ops.remove_container(&m.id, true, false).await,
        };
        if let Err(e) = res {
            errors.push(format!("{}: {e}", m.name));
        }
    }
    let outcome = if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Upstream {
            status: 500,
            message: errors.join("; "),
        })
    };
    c.audit(
        &s,
        &format!("compose.{action}"),
        "compose-project",
        &project,
        &outcome,
    )
    .await;
    Ok(affected(members.len() - errors.len(), errors))
}

fn affected(n: usize, errors: Vec<String>) -> Json<Value> {
    Json(json!({ "affected": n, "errors": errors }))
}
```

In `routes/docker/mod.rs`, add `pub mod compose;` at the top of the module list.

In `routes/mod.rs` `env_routes`:
- Change the `use` line to `use docker::{compose, containers as c, images as i, networks as n, system, volumes as v};`.
- Append:

```rust
        .route("/envs/{env}/compose-projects", get(compose::list))
        .route("/envs/{env}/compose-projects/{project}/{action}", post(compose::action))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test --workspace && make lint`
Expected: `compose` 1 unit + 3 integration PASS. Whole workspace green (151 tests at the time this plan was verified).

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): read-only compose projects with bulk actions"
```

---

### Task 12: Dev workflow and live end-to-end check

**Files:**
- Modify: `Makefile` (`dev-server` uses the current docker context)

**Interfaces:**
- Consumes: everything above.
- Produces: `make dev-server` works on hosts whose Docker socket is not `/var/run/docker.sock` (e.g. colima). This is a verification task; no new API.

- [ ] **Step 1: Update `dev-server`**

Replace the `dev-server` recipe in `Makefile` (TAB-indented):

```make
dev-server:
	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
		cargo run -p fleetops-server -- --data ./.dev-data --bind 127.0.0.1:9000
```

- [ ] **Step 2: Live smoke test against the real daemon**

Save as `/tmp/fleetops-smoke.sh` and run it with `bash` (zsh does not word-split the header array):

```bash
#!/bin/bash
set -u
rm -rf ./.smoke-data
DOCKER_HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
  ./target/debug/fleetops --data ./.smoke-data --bind 127.0.0.1:9100 > /tmp/fleetops-smoke.log 2>&1 & PID=$!
sleep 2; J=/tmp/fleetops-smoke.cookies; U=localhost:9100/api/v1
CSRF=$(curl -s -c $J -H 'content-type: application/json' -d '{"username":"admin","password":"correct-horse"}' $U/auth/setup | python3 -c 'import sys,json;print(json.load(sys.stdin)["csrf_token"])')
H=(-b $J -H "x-csrf-token: $CSRF" -H 'content-type: application/json')
echo "envs: $(curl -s "${H[@]}" $U/envs)"
echo "status: $(curl -s "${H[@]}" $U/envs/local/status)"
ID=$(curl -s "${H[@]}" -d '{"name":"dkr-smoke","image":"alpine:3.20","cmd":["sleep","60"]}' $U/envs/local/containers | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
curl -s "${H[@]}" "$U/envs/local/containers/$ID/files?path=/etc" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("files", len(d["entries"]), d["truncated"])'
echo "release: $(curl -s "${H[@]}" "$U/envs/local/containers/$ID/files/download?path=/etc/alpine-release")"
curl -s -w " recreate %{http_code}\n" "${H[@]}" -d '{"image":"alpine:3.20","cmd":["sleep","120"]}' "$U/envs/local/containers/$ID/recreate"
curl -s -o /dev/null -w "remove %{http_code}\n" "${H[@]}" -X DELETE "$U/envs/local/containers/dkr-smoke?force=true"
curl -s "${H[@]}" "$U/audit?limit=5" | python3 -c 'import sys,json;print([(e["action"],e["result"]) for e in json.load(sys.stdin)])'
kill $PID; rm -rf ./.smoke-data; docker ps -a --filter name=dkr-smoke --format 'leftover {{.Names}}'
```

Run: `cargo build -p fleetops-server && bash /tmp/fleetops-smoke.sh`
Expected:
- `envs` lists `local` with the docker-context socket URL.
- `status` is `{"state":"up",…}`.
- The listing shows `files <n> False` with n > 0.
- `release: 3.20.x`.
- The recreate JSON contains `"old_removed":true`, and the status line reads `recreate 200`.
- `remove 204`.
- The audit list starts `container.remove`, `container.recreate`, `container.start`, `container.create`, `auth.setup`, all `success`.
- No `leftover` line.

- [ ] **Step 3: Commit**

```bash
git add Makefile
git commit -m "chore: dev-server uses the active docker context"
```

---

## Deferred to P2b (tracked)

- Websocket hub at `/api/v1/ws`:
  - One multiplexed connection per tab with the spec §5.2 protocol.
  - Per-environment `EventHub` over the Docker events stream.
  - Ref-counted shared stats streams.
  - Logs (batched, with `gap` on overflow) and exec (binary frames, resize, true backpressure, audited).
  - `env_status` broadcasts and reconnect with backoff.
- Background tasks: `POST /images/pull` and `POST /images/build` (multipart) → `202 {task_id}`, with progress on the `task` topic.
- utoipa OpenAPI at `/api/v1/openapi.json` covering P1 and P2 handlers, plus the generated TS client (`pnpm gen:api` in P3).
- Container list CPU/mem sparklines, which need the `stats` topic.
- Still-open minors from the P1 review:
  - `__Host-` cookie on TLS
  - rate limit on current-password guesses
  - audit on logout
  - `request_id` on the static `/api/*` 404
