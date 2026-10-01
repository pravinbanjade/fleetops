//! A scripted cluster for API tests.
#![allow(dead_code)]

use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion, Environment};
use fleetops_core::kube::*;
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{Error, Result};
use std::collections::{BTreeMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::{mpsc, oneshot};

/// The test's side of a fake exec.
pub struct ExecHandle {
    pub target: String,
    pub opts: ExecOptions,
    pub output: mpsc::Sender<Result<Vec<u8>>>,
    pub input: mpsc::Receiver<Vec<u8>>,
    pub resize: mpsc::Receiver<(u16, u16)>,
    pub exit: oneshot::Sender<Option<i64>>,
}

#[derive(Default)]
pub struct FakeKube {
    objects: Mutex<BTreeMap<(KubeKind, String, String), KubeObject>>,
    forbidden: Mutex<HashSet<KubeKind>>,
    truncated: AtomicBool,
    pub unreachable: AtomicBool,
    calls: Mutex<Vec<String>>,
    logs: Mutex<Vec<String>>,
    pub log_opts: Mutex<Vec<LogOptions>>,
    execs: Mutex<Vec<ExecHandle>>,
}

pub fn row(name: &str, ns: Option<&str>) -> KubeRow {
    KubeRow {
        name: name.into(),
        namespace: ns.map(str::to_string),
        created: 1,
        status: "Running".into(),
        health: RowHealth::Ok,
        columns: BTreeMap::from([("ready".to_string(), "1/1".to_string())]),
    }
}

impl FakeKube {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn object(&self, kind: KubeKind, ns: &str, name: &str) {
        let ns_opt = (!kind.cluster_scoped()).then_some(ns);
        let obj = KubeObject {
            row: row(name, ns_opt),
            yaml: format!("kind: {}\nmetadata:\n  name: {name}\n", kind.kind_name()),
            conditions: vec![],
            owners: vec![],
            containers: if kind == KubeKind::Pods {
                vec!["main".into()]
            } else {
                vec![]
            },
        };
        self.objects
            .lock()
            .unwrap()
            .insert((kind, ns.into(), name.into()), obj);
    }

    pub fn set_forbidden(&self, kind: KubeKind) {
        self.forbidden.lock().unwrap().insert(kind);
    }
    pub fn set_truncated(&self) {
        self.truncated.store(true, Ordering::SeqCst);
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn set_logs(&self, lines: &[&str]) {
        *self.logs.lock().unwrap() = lines.iter().map(|s| s.to_string()).collect();
    }
    pub fn take_exec(&self) -> Option<ExecHandle> {
        self.execs.lock().unwrap().pop()
    }
    fn call(&self, c: String) {
        self.calls.lock().unwrap().push(c);
    }
    fn gate(&self, kind: KubeKind) -> Result<()> {
        if self.forbidden.lock().unwrap().contains(&kind) {
            return Err(Error::Forbidden(format!(
                "{} is forbidden: cannot list resource \"{}\"",
                kind.as_str(),
                kind.as_str()
            )));
        }
        Ok(())
    }
    fn find(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        self.objects
            .lock()
            .unwrap()
            .get(&(kind, ns.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{} \"{name}\" not found", kind.as_str())))
    }
}

#[async_trait]
impl KubeOps for FakeKube {
    async fn version(&self) -> Result<KubeVersion> {
        Ok(KubeVersion {
            git_version: "v1.32.0".into(),
            platform: "linux/amd64".into(),
            nodes: 3,
            nodes_ready: 3,
        })
    }
    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList> {
        self.gate(kind)?;
        self.call(format!("list {} {}", kind.as_str(), ns.unwrap_or("*")));
        let rows = self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter(|((k, n, _), _)| {
                *k == kind && (kind.cluster_scoped() || ns.is_none_or(|ns| ns == n))
            })
            .map(|(_, o)| o.row.clone())
            .collect();
        Ok(KubeList {
            rows,
            truncated: self.truncated.load(Ordering::SeqCst),
        })
    }
    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        self.gate(kind)?;
        self.find(kind, ns, name)
    }
    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        self.find(kind, ns, name)?;
        Ok(vec![KubeEvent {
            type_: "Normal".into(),
            reason: "Started".into(),
            message: "started".into(),
            count: 1,
            last_seen: Some(1),
            source: Some("kubelet".into()),
        }])
    }
    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        self.find(kind, ns, name)?;
        self.objects
            .lock()
            .unwrap()
            .remove(&(kind, ns.into(), name.into()));
        self.call(format!("delete {}/{ns}/{name}", kind.as_str()));
        Ok(())
    }
    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()> {
        self.find(kind, ns, name)?;
        self.call(format!("scale {}/{ns}/{name} {replicas}", kind.as_str()));
        Ok(())
    }
    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        self.find(kind, ns, name)?;
        self.call(format!("restart {}/{ns}/{name}", kind.as_str()));
        Ok(())
    }
    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        self.log_opts.lock().unwrap().push(opts.clone());
        self.call(format!("logs {ns}/{pod}"));
        let lines: Vec<Result<LogLine>> = self
            .logs
            .lock()
            .unwrap()
            .iter()
            .map(|t| {
                Ok(LogLine {
                    stream: LogStream::Stdout,
                    text: t.clone(),
                })
            })
            .collect();
        Box::pin(futures_util::stream::iter(lines))
    }
    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec> {
        self.find(KubeKind::Pods, ns, pod)?;
        let (out_tx, out_rx) = mpsc::channel(16);
        let (in_tx, in_rx) = mpsc::channel(16);
        let (rs_tx, rs_rx) = mpsc::channel(16);
        let (exit_tx, exit_rx) = oneshot::channel();
        self.execs.lock().unwrap().push(ExecHandle {
            target: format!("{ns}/{pod}"),
            opts: opts.clone(),
            output: out_tx,
            input: in_rx,
            resize: rs_rx,
            exit: exit_tx,
        });
        let output = futures_util::stream::unfold(out_rx, |mut rx| async move {
            rx.recv().await.map(|v| (v, rx))
        });
        Ok(KubeExec {
            output: Box::pin(output),
            input: in_tx,
            resize: rs_tx,
            exit_code: exit_rx,
        })
    }
}

pub struct FakeKubeEnv {
    pub id: i64,
    pub name: String,
    pub kind: EnvKind,
    pub kube: Arc<FakeKube>,
}

#[async_trait]
impl Environment for FakeKubeEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        self.kind
    }
    async fn status(&self) -> EnvStatus {
        if self.kube.unreachable.load(Ordering::SeqCst) {
            return EnvStatus::Down {
                reason: "cannot reach cluster: test".into(),
            };
        }
        EnvStatus::Up {
            version: EnvVersion::Kube(self.kube.version().await.unwrap()),
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        (!self.kube.unreachable.load(Ordering::SeqCst))
            .then(|| self.kube.clone() as Arc<dyn KubeOps>)
    }
}
