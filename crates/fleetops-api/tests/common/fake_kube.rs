//! A scripted cluster for API tests.
#![allow(dead_code)]

use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion, Environment};
use fleetops_core::helm::*;
use fleetops_core::kube::*;
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};
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

/// One recorded write: what, the YAML, the mode, and the namespaces it
/// was limited to.
pub type Applied = (String, String, ApplyMode, Option<BTreeSet<String>>);

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
    watches: Mutex<Vec<WatchTx>>,
    pub applied: Mutex<Vec<Applied>>,
    pub helm_releases: Mutex<Vec<HelmRelease>>,
    /// Returned by rollback and uninstall; `status` and `revision` are set on commit.
    pub helm_plan: Mutex<Option<HelmPlan>>,
    /// (namespaced, namespace) of each object a rollback or uninstall touches.
    pub helm_objects: Mutex<Vec<(bool, Option<String>)>>,
}

type WatchTx = (
    KubeKind,
    Option<String>,
    mpsc::UnboundedSender<Result<WatchEvent>>,
);

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
    pub fn helm_release(&self, ns: &str, name: &str, revision: u32, status: &str) {
        self.helm_releases.lock().unwrap().push(HelmRelease {
            name: name.into(),
            namespace: ns.into(),
            revision,
            status: status.into(),
            chart: "demo".into(),
            chart_version: "0.2.0".into(),
            app_version: "1.1.0".into(),
            updated: 1,
            error: (status == UNREADABLE).then(|| "not gzip".into()),
        });
    }
    /// Checks each scripted object against `allowed`, then returns the
    /// scripted plan, with the outcome filled in on a commit.
    fn helm_write(
        &self,
        dry_run: bool,
        expected: u32,
        allowed: Option<&BTreeSet<String>>,
        revision: Option<u32>,
    ) -> Result<HelmPlan> {
        for (namespaced, ns) in self.helm_objects.lock().unwrap().iter() {
            fleetops_core::access::check_doc(allowed, *namespaced, ns.as_deref())?;
        }
        let mut plan = self
            .helm_plan
            .lock()
            .unwrap()
            .clone()
            .expect("script helm_plan first");
        if !dry_run {
            if expected != plan.current {
                return Err(Error::Conflict(CHANGED.into()));
            }
            plan.revision = revision;
            plan.status.get_or_insert_with(|| {
                if revision.is_some() {
                    "deployed".into()
                } else {
                    "uninstalled".into()
                }
            });
        }
        Ok(plan)
    }
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
    /// Sends `ev` to every open watch of `kind`.
    pub fn emit_watch(&self, kind: KubeKind, ev: Result<WatchEvent>) {
        let mut w = self.watches.lock().unwrap();
        w.retain(|(_, _, tx)| !tx.is_closed());
        for (k, _, tx) in w.iter() {
            if *k == kind {
                let _ = tx.send(match &ev {
                    Ok(e) => Ok(e.clone()),
                    Err(e) => Err(Error::Forbidden(e.detail())),
                });
            }
        }
    }
    /// Upstream watches still open.
    pub fn open_watches(&self) -> usize {
        self.watches
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, tx)| !tx.is_closed())
            .count()
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
            nodes: Some(3),
            nodes_ready: Some(3),
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
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent> {
        self.call(format!("watch {} {}", kind.as_str(), ns.unwrap_or("*")));
        let (tx, rx) = mpsc::unbounded_channel();
        self.watches
            .lock()
            .unwrap()
            .push((kind, ns.map(str::to_string), tx));
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|v| (v, rx))
        }))
    }
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject> {
        let o = self.find(kind, ns, name)?;
        Ok(EditableObject {
            yaml: o.yaml,
            resource_version: "1".into(),
        })
    }
    /// One result per `---`-separated document; a ClusterRole is forbidden,
    /// as on a narrow agent.
    async fn apply(
        &self,
        yaml: &str,
        default_ns: &str,
        mode: ApplyMode,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<Vec<ApplyResult>> {
        self.applied
            .lock()
            .unwrap()
            .push(("apply".into(), yaml.into(), mode, allowed.cloned()));
        let field = |doc: &str, key: &str| {
            doc.lines()
                .find_map(|l| l.trim().strip_prefix(key))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        Ok(yaml
            .split("\n---\n")
            .filter(|d| !d.trim().is_empty())
            .enumerate()
            .map(|(index, doc)| {
                let kind = field(doc, "kind:");
                let namespaced = !matches!(
                    kind.as_str(),
                    "Namespace"
                        | "Node"
                        | "ClusterRole"
                        | "ClusterRoleBinding"
                        | "CustomResourceDefinition"
                );
                let ns = Some(field(doc, "namespace:"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| default_ns.to_string());
                // As the real client: refused before anything is sent.
                let refused = fleetops_core::access::check_doc(
                    allowed,
                    namespaced,
                    namespaced.then_some(ns.as_str()),
                )
                .err()
                .map(|e| ApplyError::from(&e));
                let error = refused.or_else(|| {
                    (kind == "ClusterRole").then(|| ApplyError {
                        code: "forbidden".into(),
                        message: "clusterroles.rbac.authorization.k8s.io is forbidden".into(),
                    })
                });
                let ok = error.is_none();
                ApplyResult {
                    index,
                    api_version: field(doc, "apiVersion:"),
                    namespace: ok.then(|| ns.clone()),
                    name: field(doc, "name:"),
                    action: ok.then_some(ApplyAction::Created),
                    before: None,
                    after: ok.then(|| doc.to_string()),
                    error,
                    kind,
                }
            })
            .collect())
    }
    async fn replace(
        &self,
        kind: KubeKind,
        ns: &str,
        name: &str,
        yaml: &str,
        rv: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult> {
        self.find(kind, ns, name)?;
        if rv != "1" {
            return Err(Error::Conflict("changed since you opened it".into()));
        }
        self.applied.lock().unwrap().push((
            format!("replace {}/{ns}/{name}", kind.as_str()),
            yaml.into(),
            mode,
            None,
        ));
        Ok(ApplyResult {
            index: 0,
            api_version: "v1".into(),
            kind: kind.kind_name().into(),
            namespace: Some(ns.into()),
            name: name.into(),
            action: Some(ApplyAction::Configured),
            before: Some(String::new()),
            after: Some(yaml.into()),
            error: None,
        })
    }
    async fn helm_list(&self, ns: Option<&str>) -> Result<Vec<HelmRelease>> {
        self.call(format!("helm_list {}", ns.unwrap_or("*")));
        Ok(self
            .helm_releases
            .lock()
            .unwrap()
            .iter()
            .filter(|r| ns.is_none_or(|n| r.namespace == n))
            .cloned()
            .collect())
    }
    async fn helm_get(&self, ns: &str, name: &str, _: Option<u32>) -> Result<ReleaseDetail> {
        let release = self
            .helm_releases
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.namespace == ns && r.name == name)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("release '{name}' not found")))?;
        Ok(ReleaseDetail {
            release,
            description: "Upgrade complete".into(),
            notes: "hello".into(),
            values: format!("db:\n  password: {MASK}\n"),
            manifest: "---\nkind: ConfigMap\n".into(),
        })
    }
    async fn helm_history(&self, ns: &str, name: &str) -> Result<Vec<HelmRevision>> {
        let r = self.helm_get(ns, name, None).await?.release;
        Ok((1..=r.revision)
            .map(|v| HelmRevision {
                revision: v,
                status: if v == r.revision {
                    r.status.clone()
                } else {
                    "superseded".into()
                },
                chart_version: r.chart_version.clone(),
                app_version: r.app_version.clone(),
                description: String::new(),
                updated: 0,
            })
            .collect())
    }
    async fn helm_rollback(
        &self,
        ns: &str,
        name: &str,
        revision: u32,
        expected: u32,
        dry_run: bool,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<HelmPlan> {
        self.call(format!(
            "helm_rollback {ns}/{name} to {revision} at {expected} dry={dry_run}"
        ));
        self.helm_write(dry_run, expected, allowed, Some(expected + 1))
    }
    async fn helm_uninstall(
        &self,
        ns: &str,
        name: &str,
        expected: u32,
        dry_run: bool,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<HelmPlan> {
        self.call(format!(
            "helm_uninstall {ns}/{name} at {expected} dry={dry_run}"
        ));
        self.helm_write(dry_run, expected, allowed, None)
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
