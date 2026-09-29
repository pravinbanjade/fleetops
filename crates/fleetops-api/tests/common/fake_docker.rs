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
