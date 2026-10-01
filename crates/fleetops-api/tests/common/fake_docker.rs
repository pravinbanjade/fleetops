//! In-memory stand-in for a Docker daemon, used by the API tests.

use async_trait::async_trait;
use fleetops_core::docker::*;
use fleetops_core::env::{EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion, Environment};
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
    /// What `recent_events` replays.
    pub past_events: Vec<DockerEvent>,
    next_id: usize,
}

#[derive(Default)]
pub struct FakeDocker {
    pub state: Mutex<FakeState>,
    /// When false, the environment reports Down and `docker()` is None.
    pub reachable: AtomicBool,
    pub streams: Mutex<FakeStreams>,
    pub hold_pulls: AtomicBool,
    pub release_pulls: Arc<Notify>,
    /// Notified to say reachability changed (Environment::wait_change).
    pub changed: Notify,
    /// Every spec the factory was asked to build, oldest first.
    pub built: Mutex<Vec<EnvSpec>>,
    /// The cluster behind every Kubernetes environment the factory builds.
    pub kube: Arc<super::fake_kube::FakeKube>,
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
    /// The specs built so far.
    pub fn built(&self) -> Vec<EnvSpec> {
        self.built.lock().unwrap().clone()
    }

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
                version: EnvVersion::Docker(self.docker.version().await.unwrap()),
            }
        } else {
            EnvStatus::Down {
                reason: "fake is unreachable".into(),
            }
        }
    }
    async fn wait_change(&self) {
        self.docker.changed.notified().await
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
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        self.0.built.lock().unwrap().push(spec.clone());
        if spec.kind.is_kube() {
            return Ok(Arc::new(super::fake_kube::FakeKubeEnv {
                id: spec.id,
                name: spec.name.clone(),
                kind: spec.kind,
                kube: self.0.kube.clone(),
            }));
        }
        Ok(Arc::new(FakeEnv {
            id: spec.id,
            name: spec.name.clone(),
            docker: self.0.clone(),
        }))
    }
}

// ---- streams ---------------------------------------------------------------

use fleetops_core::streams::{
    BoxStream, BuildContext, BuildSpec, DockerEvent, ExecOptions, ExecSession, LogLine, LogOptions,
    LogStream, StatsSample, StreamOps,
};
use futures_util::{stream, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{mpsc, Notify};

type Tx<T> = mpsc::UnboundedSender<Result<T>>;

/// Hand-driven streams: tests push events, stats and log lines and can end
/// streams to simulate the daemon going away.
#[derive(Default)]
pub struct FakeStreams {
    events: Vec<Tx<DockerEvent>>,
    stats: Vec<(String, Tx<StatsSample>)>,
    logs: HashMap<String, Vec<LogLine>>,
    follow: Vec<(String, Tx<LogLine>)>,
    pub resizes: Vec<(String, u16, u16)>,
    pub builds: Vec<BuildSpec>,
    pub pulls: Vec<String>,
    execs: usize,
}

fn rx_stream<T: Send + 'static>(rx: mpsc::UnboundedReceiver<Result<T>>) -> BoxStream<T> {
    Box::pin(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|v| (v, rx))
    }))
}

pub fn event(id: &str, action: &str) -> DockerEvent {
    DockerEvent {
        kind: "container".into(),
        action: action.into(),
        id: id.into(),
        name: Some(format!("{id}-name")),
        attributes: Default::default(),
        time: 1,
    }
}

pub fn sample(cpu: f64) -> StatsSample {
    StatsSample {
        time: 1,
        cpu_percent: cpu,
        memory_used: 10,
        memory_limit: 100,
        net_rx: 0,
        net_tx: 0,
        block_read: 0,
        block_write: 0,
        pids: 1,
    }
}

impl FakeDocker {
    fn streams(&self) -> std::sync::MutexGuard<'_, FakeStreams> {
        self.streams.lock().unwrap()
    }

    pub fn emit_event(&self, e: DockerEvent) {
        self.streams()
            .events
            .retain(|tx| tx.send(Ok(e.clone())).is_ok());
    }

    /// Event streams currently open (not yet dropped by their consumer).
    pub fn open_event_streams(&self) -> usize {
        let mut s = self.streams();
        s.events.retain(|tx| !tx.is_closed());
        s.events.len()
    }

    /// Ends every open events stream, as a daemon restart would.
    pub fn end_event_streams(&self) {
        self.streams().events.clear();
    }

    pub fn emit_stats(&self, id: &str, s: StatsSample) {
        self.streams()
            .stats
            .retain(|(i, tx)| i != id || tx.send(Ok(s.clone())).is_ok());
    }

    pub fn open_stats(&self, id: &str) -> usize {
        let mut s = self.streams();
        s.stats.retain(|(_, tx)| !tx.is_closed());
        s.stats.iter().filter(|(i, _)| i == id).count()
    }

    pub fn end_stats(&self, id: &str) {
        self.streams().stats.retain(|(i, _)| i != id);
    }

    pub fn set_logs(&self, id: &str, lines: Vec<LogLine>) {
        self.streams().logs.insert(id.into(), lines);
    }

    pub fn emit_log(&self, id: &str, line: LogLine) {
        self.streams()
            .follow
            .retain(|(i, tx)| i != id || tx.send(Ok(line.clone())).is_ok());
    }
}

pub fn line(stream: LogStream, text: &str) -> LogLine {
    LogLine {
        stream,
        text: text.into(),
    }
}

/// Chunks pulled from the `flood` exec so far, across all sessions.
pub static FLOOD_CHUNKS: AtomicUsize = AtomicUsize::new(0);
pub const FLOOD_CHUNK: usize = 16 * 1024;

#[async_trait]
impl StreamOps for FakeDocker {
    fn events(&self) -> BoxStream<DockerEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams().events.push(tx);
        rx_stream(rx)
    }

    async fn recent_events(&self, since: i64) -> Result<Vec<DockerEvent>> {
        let st = self.state.lock().unwrap();
        Ok(st
            .past_events
            .iter()
            .filter(|e| e.time >= since)
            .cloned()
            .collect())
    }

    fn stats(&self, id: &str) -> BoxStream<StatsSample> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams().stats.push((id.into(), tx));
        rx_stream(rx)
    }

    fn logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        let mut preset: Vec<LogLine> = self
            .streams()
            .logs
            .get(id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|l| match l.stream {
                LogStream::Stdout => opts.stdout,
                LogStream::Stderr => opts.stderr,
            })
            .collect();
        if let Some(n) = opts.tail {
            let skip = preset.len().saturating_sub(n as usize);
            preset.drain(..skip);
        }
        let head = stream::iter(preset.into_iter().map(Ok));
        if opts.follow {
            let (tx, rx) = mpsc::unbounded_channel();
            self.streams().follow.push((id.into(), tx));
            Box::pin(head.chain(rx_stream(rx)))
        } else {
            Box::pin(head)
        }
    }

    /// Echoes input back as `echo:<input>`; `exit\n` ends the session with
    /// code 0. The command `flood` streams output forever, lazily.
    async fn exec_start(&self, id: &str, opts: &ExecOptions) -> Result<ExecSession> {
        self.container(id)
            .ok_or_else(|| not_found("container", id))?;
        let exec_id = {
            let mut s = self.streams();
            s.execs += 1;
            format!("exec{}", s.execs)
        };
        self.call(format!("exec {id} {}", opts.command().join(" ")));
        let (input, mut input_rx) = mpsc::channel::<Vec<u8>>(64);
        if opts.cmd.as_deref() == Some(&["flood".to_string()][..]) {
            let output = stream::repeat_with(|| {
                FLOOD_CHUNKS.fetch_add(1, Ordering::SeqCst);
                Ok(vec![b'x'; FLOOD_CHUNK])
            });
            return Ok(ExecSession {
                exec_id,
                output: Box::pin(output),
                input,
            });
        }
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let _ = out_tx.send(Ok(b"$ ".to_vec()));
        tokio::spawn(async move {
            while let Some(buf) = input_rx.recv().await {
                if buf == b"exit\n" {
                    break;
                }
                let mut echo = b"echo:".to_vec();
                echo.extend(buf);
                if out_tx.send(Ok(echo)).is_err() {
                    break;
                }
            }
        });
        Ok(ExecSession {
            exec_id,
            output: rx_stream(out_rx),
            input,
        })
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.streams().resizes.push((exec_id.into(), cols, rows));
        Ok(())
    }

    async fn exec_exit_code(&self, _exec_id: &str) -> Result<Option<i64>> {
        Ok(Some(0))
    }

    /// `missing` fails with Docker's 404. While `hold_pulls` is set, a pull
    /// sends its first message and then waits for `release_pulls`.
    fn pull_image(&self, image: &str, tag: &str) -> BoxStream<Value> {
        let reference = format!("{image}:{tag}");
        self.streams().pulls.push(reference.clone());
        let hold = self.hold_pulls.load(Ordering::SeqCst);
        let release = self.release_pulls.clone();
        let missing = image == "missing";
        Box::pin(
            stream::iter([Ok(json!({"status": format!("Pulling from {image}")}))]).chain(
                stream::once(async move {
                    if hold {
                        release.notified().await;
                    }
                    if missing {
                        vec![Err(Error::Upstream {
                            status: 404,
                            message: "manifest unknown".into(),
                        })]
                    } else {
                        vec![
                            Ok(json!({"status": "Download complete", "id": "layer1"})),
                            Ok(json!({"status": format!("Status: Downloaded newer image for {reference}")})),
                        ]
                    }
                })
                .flat_map(stream::iter),
            ),
        )
    }

    /// A Dockerfile containing `fail` fails the way a failing RUN step does.
    fn build_image(&self, spec: BuildSpec) -> BoxStream<Value> {
        let fails = matches!(&spec.context, BuildContext::Dockerfile(d) if String::from_utf8_lossy(d).contains("fail"));
        self.streams().builds.push(spec);
        let tail = if fails {
            Ok(
                json!({"errorDetail": {"message": "The command '/bin/sh -c fail' returned a non-zero code: 1"}}),
            )
        } else {
            Ok(json!({"aux": {"ID": "sha256:built"}}))
        };
        Box::pin(stream::iter([
            Ok(json!({"stream": "Step 1/1 : FROM alpine\n"})),
            tail,
        ]))
    }
}
