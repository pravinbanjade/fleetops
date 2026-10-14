//! `KubeOps` over a kube-rs client, the same code whether the client
//! talks to the apiserver directly or through an agent tunnel.

use crate::{errors, kinds::api_resource, mask, summaries};
use async_trait::async_trait;
use fleetops_core::kube::{
    ApiKind, ApplyAction, ApplyError, ApplyMode, ApplyResult, EditableObject, KubeEvent, KubeExec,
    KubeFilter, KubeKind, KubeList, KubeObject, KubeOps, KubeRef, KubeVersion, WatchEvent,
    FIELD_MANAGER, LIST_CAP, PAGE_SIZE, SUSPENDED_ANNOTATION,
};
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{now_unix, Error, Result};
use futures_util::{AsyncBufReadExt, SinkExt, StreamExt, TryStreamExt};
use k8s_openapi::api::{
    apps::v1::ReplicaSet,
    batch::v1::{CronJob, Job},
    core::v1::{Event, Node, Pod},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::api::{
    Api, AttachParams, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams,
    PostParams, TerminalSize,
};
use kube::discovery::{ApiResource, Scope};
use serde_json::json;
use std::{
    collections::HashMap,
    future::Future,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

/// Every request but logs and exec gives up after this (spec §5).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Node counts in `version()` are reused for this long.
pub const NODE_CACHE: Duration = Duration::from_secs(30);
/// Discovery answers are reused for this long (spec §2.3).
pub const DISCOVERY_TTL: Duration = Duration::from_secs(300);

/// A kind's resource and whether it is namespaced.
type Resolved = (ApiResource, bool);

pub struct KubeClient {
    pub(crate) client: kube::Client,
    list_cap: usize,
    nodes: Mutex<Option<(Instant, (u32, u32))>>,
    discovery: Mutex<HashMap<(String, String), (Instant, Resolved)>>,
    pub(crate) usage_cache: Mutex<UsageCache>,
    /// Generic kinds, reused for `discovery::DISCOVERY_CACHE`.
    pub(crate) generic: Mutex<Option<(Instant, fleetops_core::kube::Discovery)>>,
}

type UsageCache = HashMap<(KubeKind, Option<String>), (Instant, fleetops_core::kube::KubeUsage)>;

impl KubeClient {
    pub fn new(client: kube::Client) -> Self {
        Self {
            client,
            list_cap: LIST_CAP,
            nodes: Mutex::new(None),
            discovery: Mutex::new(HashMap::new()),
            usage_cache: Mutex::new(HashMap::new()),
            generic: Mutex::new(None),
        }
    }

    /// A lower row cap, so tests can see truncation.
    pub fn with_list_cap(mut self, cap: usize) -> Self {
        self.list_cap = cap;
        self
    }

    fn api(&self, kind: KubeKind, ns: Option<&str>) -> Api<DynamicObject> {
        let ar = api_resource(kind);
        match ns.filter(|_| !kind.cluster_scoped()) {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        }
    }

    fn pods(&self, ns: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), ns)
    }

    async fn node_counts(&self) -> Result<(u32, u32)> {
        if let Some((at, counts)) = *self.nodes.lock().unwrap() {
            if at.elapsed() < NODE_CACHE {
                return Ok(counts);
            }
        }
        let nodes: Api<Node> = Api::all(self.client.clone());
        let list = timed(nodes.list(&ListParams::default())).await?;
        let total = list.items.len() as u32;
        let ready = list
            .items
            .iter()
            .filter(|n| {
                n.status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            })
            .count() as u32;
        *self.nodes.lock().unwrap() = Some((Instant::now(), (total, ready)));
        Ok((total, ready))
    }

    /// The resource for `apiVersion/kind` and whether it is namespaced.
    pub(crate) async fn resolve(&self, api_version: &str, kind: &str) -> Result<Resolved> {
        let k = (api_version.to_string(), kind.to_string());
        if let Some((at, r)) = self.discovery.lock().unwrap().get(&k) {
            if at.elapsed() < DISCOVERY_TTL {
                return Ok(r.clone());
            }
        }
        let gvk = crate::apply::gvk(api_version, kind);
        let found = tokio::time::timeout(
            REQUEST_TIMEOUT,
            kube::discovery::pinned_kind(&self.client, &gvk),
        )
        .await
        .map_err(|_| Error::Upstream {
            status: 504,
            message: "the cluster did not answer discovery in time".into(),
        })?;
        let (ar, caps) = match found {
            Ok(v) => v,
            Err(kube::Error::Discovery(_)) => {
                return Err(Error::Invalid(format!(
                    "the cluster does not serve {api_version}/{kind}"
                )))
            }
            Err(kube::Error::Api(s)) if s.code == 404 => {
                return Err(Error::Invalid(format!(
                    "the cluster does not serve {api_version}/{kind}"
                )))
            }
            Err(e) => return Err(errors::map(e)),
        };
        let r = (ar, caps.scope == Scope::Namespaced);
        self.discovery
            .lock()
            .unwrap()
            .insert(k, (Instant::now(), r.clone()));
        Ok(r)
    }

    /// Server-side applies one document: its action, and the cleaned
    /// before/after YAML.
    async fn apply_doc(
        &self,
        doc: &mut crate::apply::Doc,
        default_ns: &str,
        mode: ApplyMode,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<(ApplyAction, Option<String>, String)> {
        let (ar, namespaced) = self.resolve(&doc.api_version, &doc.kind).await?;
        crate::apply::set_namespace(doc, namespaced, default_ns);
        fleetops_core::access::check_doc(allowed, namespaced, doc.namespace.as_deref())?;
        let api: Api<DynamicObject> = match &doc.namespace {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        };
        let live = timed(api.get_opt(&doc.name))
            .await?
            .map(|o| serde_json::to_value(o).unwrap_or_default());
        crate::mask::unmask(&mut doc.value, live.as_ref()).map_err(Error::Invalid)?;
        let mut pp = PatchParams::apply(FIELD_MANAGER);
        pp.dry_run = mode.dry_run();
        pp.force = mode.force();
        let out = timed(api.patch(&doc.name, &pp, &Patch::Apply(&doc.value))).await?;
        outcome(live, serde_json::to_value(out).unwrap_or_default())
    }

    /// The owner chain, nearest first. A ReplicaSet owner is followed one
    /// step so a Deployment's pods link to the Deployment.
    async fn owner_chain(&self, ns: &str, obj: &DynamicObject) -> Vec<KubeRef> {
        let mut chain = summaries::owners(obj);
        if let Some(rs) = chain
            .iter()
            .find(|o| o.kind == "ReplicaSet")
            .map(|o| o.name.clone())
        {
            let rss: Api<ReplicaSet> = Api::namespaced(self.client.clone(), ns);
            if let Ok(Ok(Some(rs))) = tokio::time::timeout(REQUEST_TIMEOUT, rss.get_opt(&rs)).await
            {
                chain.extend(
                    rs.metadata
                        .owner_references
                        .into_iter()
                        .flatten()
                        .map(|o| KubeRef {
                            link: KubeKind::from_kind_name(&o.kind),
                            kind: o.kind,
                            name: o.name,
                        }),
                );
            }
        }
        chain
    }
}

/// The object as the editor and diffs show it.
pub(crate) fn cleaned(mut v: serde_json::Value) -> serde_json::Value {
    crate::mask::clean(&mut v);
    v
}

/// What a write did, judged on unmasked objects (a Secret whose values
/// changed has identical masked forms), and the masked YAML to show.
pub(crate) fn outcome(
    before: Option<serde_json::Value>,
    after: serde_json::Value,
) -> Result<(ApplyAction, Option<String>, String)> {
    let strip = |mut v: serde_json::Value| {
        crate::mask::strip(&mut v);
        v
    };
    let (before, after) = (before.map(strip), strip(after));
    let action = match &before {
        None => ApplyAction::Created,
        Some(b) if *b == after => ApplyAction::Unchanged,
        Some(_) => ApplyAction::Configured,
    };
    let before = before
        .map(|b| crate::mask::to_yaml(&cleaned(b)))
        .transpose()?;
    Ok((action, before, crate::mask::to_yaml(&cleaned(after))?))
}

/// Runs one apiserver call with the request timeout and error mapping.
pub(crate) async fn timed<T>(f: impl Future<Output = kube::Result<T>>) -> Result<T> {
    match tokio::time::timeout(REQUEST_TIMEOUT, f).await {
        Ok(r) => r.map_err(errors::map),
        Err(_) => Err(Error::Upstream {
            status: 504,
            message: format!(
                "the cluster did not answer within {}s",
                REQUEST_TIMEOUT.as_secs()
            ),
        }),
    }
}

async fn first_container(pods: &Api<Pod>, pod: &str) -> Result<String> {
    let p = timed(pods.get(pod)).await?;
    p.spec
        .and_then(|s| s.containers.into_iter().next())
        .map(|c| c.name)
        .ok_or_else(|| Error::NotFound(format!("pod '{pod}' has no containers")))
}

/// One pod's log; `o.container` `None` is its first container.
pub(crate) fn pod_logs(pods: Api<Pod>, pod: String, o: LogOptions) -> BoxStream<LogLine> {
    let lines = futures_util::stream::once(async move {
        let container = match o.container.clone() {
            Some(c) => c,
            None => first_container(&pods, &pod).await?,
        };
        let lp = LogParams {
            container: Some(container),
            follow: o.follow && !o.previous,
            previous: o.previous,
            tail_lines: o.tail.map(i64::from),
            since_seconds: o.since.map(|s| (now_unix() - s).max(1)),
            timestamps: o.timestamps,
            ..LogParams::default()
        };
        let read = pods.log_stream(&pod, &lp).await.map_err(errors::map)?;
        Ok::<_, Error>(read.lines().map(|l| {
            l.map(|text| LogLine {
                stream: LogStream::Stdout,
                text,
                pod: None,
            })
            .map_err(stream_err)
        }))
    })
    .try_flatten();
    Box::pin(lines)
}

/// A finished exec's code: 0 on Success, else the ExitCode cause.
fn exit_code(s: &Status) -> Option<i64> {
    if s.status.as_deref() == Some("Success") {
        return Some(0);
    }
    s.details
        .as_ref()?
        .causes
        .as_ref()?
        .iter()
        .find(|c| c.reason.as_deref() == Some("ExitCode"))
        .and_then(|c| c.message.as_deref()?.parse().ok())
}

fn stream_err(e: std::io::Error) -> Error {
    Error::Upstream {
        status: 502,
        message: e.to_string(),
    }
}

#[async_trait]
impl KubeOps for KubeClient {
    async fn version(&self) -> Result<KubeVersion> {
        let info = timed(self.client.apiserver_version()).await?;
        // Namespace-scoped credentials may not list nodes; the cluster is
        // up all the same.
        let (nodes, nodes_ready) = match self.node_counts().await {
            Ok((n, r)) => (Some(n), Some(r)),
            Err(Error::Forbidden(_)) => (None, None),
            Err(e) => return Err(e),
        };
        Ok(KubeVersion {
            git_version: info.git_version,
            platform: info.platform,
            nodes,
            nodes_ready,
        })
    }

    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList> {
        self.list_where(kind, ns, None).await
    }

    async fn list_filtered(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
        filter: KubeFilter,
    ) -> Result<KubeList> {
        filter.check(kind)?;
        self.list_where(kind, ns, Some(filter.field_selector()))
            .await
    }

    fn watch_filtered(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
        filter: KubeFilter,
    ) -> BoxStream<WatchEvent> {
        if let Err(e) = filter.check(kind) {
            return Box::pin(futures_util::stream::once(async move { Err(e) }));
        }
        let config = kube::runtime::watcher::Config::default().fields(filter.field_selector());
        crate::watch::watch_with(self.api(kind, ns), crate::watch::curated(kind), config)
    }

    async fn count(&self, kind: KubeKind, ns: Option<&str>) -> Result<u64> {
        self.count_impl(kind, ns).await
    }

    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        let obj = timed(self.api(kind, Some(ns)).get(name)).await?;
        let owners = self.owner_chain(ns, &obj).await;
        Ok(KubeObject {
            row: summaries::row(kind, &obj),
            yaml: mask::yaml(kind, &obj),
            conditions: summaries::conditions(&obj),
            owners,
            containers: if kind == KubeKind::Pods {
                summaries::containers(&obj)
            } else if kind.has_pod_logs() {
                summaries::template_containers(&obj)
            } else {
                vec![]
            },
        })
    }

    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        self.object_events(kind.kind_name(), !kind.cluster_scoped(), ns, name)
            .await
    }

    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        timed(
            self.api(kind, Some(ns))
                .delete(name, &DeleteParams::background()),
        )
        .await?;
        Ok(())
    }

    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()> {
        if !kind.scalable() {
            return Err(Error::Invalid(format!(
                "{} cannot be scaled",
                kind.as_str()
            )));
        }
        let patch = Patch::Merge(json!({ "spec": { "replicas": replicas } }));
        timed(self.api(kind, Some(ns)).patch_subresource(
            "scale",
            name,
            &PatchParams::default(),
            &patch,
        ))
        .await?;
        Ok(())
    }

    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        if !kind.restartable() {
            return Err(Error::Invalid(format!(
                "{} cannot be restarted",
                kind.as_str()
            )));
        }
        let now = k8s_openapi::jiff::Timestamp::now().to_string();
        let patch = Patch::Merge(
            json!({ "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": now } } } } }),
        );
        timed(
            self.api(kind, Some(ns))
                .patch(name, &PatchParams::default(), &patch),
        )
        .await?;
        Ok(())
    }

    async fn suspend(&self, kind: KubeKind, ns: &str, name: &str, on: bool) -> Result<()> {
        if !kind.suspendable() {
            return Err(Error::Invalid(format!(
                "{} cannot be suspended",
                kind.as_str()
            )));
        }
        let api = self.api(kind, Some(ns));
        let pp = PatchParams {
            field_manager: Some(FIELD_MANAGER.into()),
            ..PatchParams::default()
        };
        if kind == KubeKind::CronJobs {
            let patch = Patch::Merge(json!({ "spec": { "suspend": on } }));
            timed(api.patch(name, &pp, &patch)).await?;
            return Ok(());
        }
        // The version read is a precondition, so a scale in between is a
        // conflict, never a silent overwrite of someone else's count. The
        // controller also writes status (pods counted, rollout progress),
        // which moves the version too: when the replicas and the note are
        // still as first read, only status moved, and the write is retried.
        let mut first: Option<ScaleState> = None;
        for attempt in 1..=SUSPEND_ATTEMPTS {
            let obj = timed(api.get(name)).await?;
            let state = scale_state(&obj);
            match &first {
                None => first = Some(state.clone()),
                Some(f) if *f != state => return Err(suspend_conflict(name, on)),
                Some(_) => {}
            }
            let Some(patch) = suspend_patch(name, on, &obj, state)? else {
                return Ok(());
            };
            match timed(api.patch(name, &pp, &Patch::Merge(patch))).await {
                Err(Error::Conflict(_)) if attempt < SUSPEND_ATTEMPTS => continue,
                Err(Error::Conflict(_)) => return Err(suspend_conflict(name, on)),
                r => return r.map(|_| ()),
            }
        }
        Err(suspend_conflict(name, on))
    }

    async fn cordon(&self, node: &str, on: bool) -> Result<()> {
        // `null` drops the field, as `kubectl uncordon` does.
        let patch = Patch::Merge(
            json!({ "spec": { "unschedulable": if on { json!(true) } else { json!(null) } } }),
        );
        let pp = PatchParams {
            field_manager: Some(FIELD_MANAGER.into()),
            ..PatchParams::default()
        };
        timed(Api::<Node>::all(self.client.clone()).patch(node, &pp, &patch)).await?;
        Ok(())
    }

    async fn trigger(&self, ns: &str, cronjob: &str) -> Result<String> {
        let cj = timed(Api::<CronJob>::namespaced(self.client.clone(), ns).get(cronjob)).await?;
        let job = manual_job(&cj, now_unix())?;
        let pp = PostParams {
            field_manager: Some(FIELD_MANAGER.into()),
            ..PostParams::default()
        };
        let made = timed(Api::<Job>::namespaced(self.client.clone(), ns).create(&pp, &job)).await?;
        Ok(made.metadata.name.unwrap_or_default())
    }

    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        pod_logs(self.pods(ns), pod.to_string(), opts.clone())
    }

    fn workload_logs(
        &self,
        kind: KubeKind,
        ns: &str,
        name: &str,
        opts: &LogOptions,
    ) -> BoxStream<LogLine> {
        crate::workload_logs::run(
            self.client.clone(),
            kind,
            ns.into(),
            name.into(),
            opts.clone(),
        )
    }

    async fn workload_pods(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeList> {
        if !kind.has_pod_logs() {
            return Err(Error::Invalid(format!(
                "{} have no pods of their own",
                kind.as_str()
            )));
        }
        let obj = timed(self.api(kind, Some(ns)).get(name)).await?;
        let sel = crate::workload_logs::selector(&obj.data["spec"]["selector"])?;
        let lp = ListParams::default().labels(&sel).limit(PAGE_SIZE);
        let list = timed(self.api(KubeKind::Pods, Some(ns)).list(&lp)).await?;
        Ok(KubeList {
            truncated: list
                .metadata
                .continue_
                .as_deref()
                .is_some_and(|c| !c.is_empty()),
            rows: list
                .items
                .iter()
                .map(|o| summaries::row(KubeKind::Pods, o))
                .collect(),
        })
    }

    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec> {
        let pods = self.pods(ns);
        let container = match &opts.container {
            Some(c) => c.clone(),
            None => first_container(&pods, pod).await?,
        };
        let ap = AttachParams::interactive_tty().container(container);
        let mut proc = timed(pods.exec(pod, opts.command(), &ap)).await?;
        let mut size = proc.terminal_size().expect("tty exec has a size channel");
        let _ = size
            .send(TerminalSize {
                width: opts.cols,
                height: opts.rows,
            })
            .await;
        let stdout = proc.stdout().expect("tty exec has stdout");
        let mut stdin = proc.stdin().expect("tty exec has stdin");
        let status = proc.take_status().expect("exec has a status");

        let (input, mut input_rx) = mpsc::channel::<Vec<u8>>(64);
        tokio::spawn(async move {
            while let Some(bytes) = input_rx.recv().await {
                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let (resize, mut resize_rx) = mpsc::channel::<(u16, u16)>(8);
        tokio::spawn(async move {
            while let Some((cols, rows)) = resize_rx.recv().await {
                if size
                    .send(TerminalSize {
                        width: cols,
                        height: rows,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let (exit_tx, exit_rx) = oneshot::channel();
        tokio::spawn(async move {
            let code = status.await.as_ref().and_then(exit_code);
            let _ = exit_tx.send(code);
            let _ = proc.join().await;
        });
        let output = tokio_util::io::ReaderStream::new(stdout)
            .map(|r| r.map(|b| b.to_vec()).map_err(stream_err));
        Ok(KubeExec {
            output: Box::pin(output),
            input,
            resize,
            exit_code: exit_rx,
        })
    }

    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent> {
        crate::watch::watch(self.api(kind, ns), crate::watch::curated(kind))
    }

    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject> {
        let obj = timed(self.api(kind, Some(ns)).get(name)).await?;
        let resource_version = obj.metadata.resource_version.clone().unwrap_or_default();
        let yaml = crate::mask::to_yaml(&cleaned(crate::kinds::typed_json(kind, &obj)))?;
        Ok(EditableObject {
            yaml,
            resource_version,
        })
    }

    async fn apply(
        &self,
        yaml: &str,
        default_ns: &str,
        mode: ApplyMode,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<Vec<ApplyResult>> {
        let docs = crate::apply::split(yaml)?;
        let mut out = Vec::with_capacity(docs.len());
        for mut doc in docs {
            let r = self.apply_doc(&mut doc, default_ns, mode, allowed).await;
            let mut res = doc.result();
            match r {
                Ok((action, before, after)) => {
                    res.action = Some(action);
                    res.before = before;
                    res.after = Some(after);
                }
                Err(e) => res.error = Some(ApplyError::from(&e)),
            }
            out.push(res);
        }
        Ok(out)
    }

    async fn replace(
        &self,
        kind: KubeKind,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult> {
        self.replace_in(
            self.api(kind, Some(ns)),
            kind.kind_name(),
            !kind.cluster_scoped(),
            ns,
            name,
            yaml,
            resource_version,
            mode,
        )
        .await
    }

    async fn usage(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
    ) -> Result<fleetops_core::kube::KubeUsage> {
        self.usage_impl(kind, ns).await
    }

    async fn discover(&self, refresh: bool) -> Result<fleetops_core::kube::Discovery> {
        self.discover_impl(refresh).await
    }

    async fn generic_list(&self, k: &ApiKind, ns: Option<&str>) -> Result<KubeList> {
        self.generic_list_impl(k, ns).await
    }

    async fn generic_get(&self, k: &ApiKind, ns: &str, name: &str) -> Result<KubeObject> {
        self.generic_get_impl(k, ns, name).await
    }

    fn generic_watch(&self, k: &ApiKind, ns: Option<&str>) -> BoxStream<WatchEvent> {
        self.generic_watch_impl(k, ns)
    }

    async fn generic_events(&self, k: &ApiKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        self.generic_events_impl(k, ns, name).await
    }

    async fn generic_delete(&self, k: &ApiKind, ns: &str, name: &str) -> Result<()> {
        self.generic_delete_impl(k, ns, name).await
    }

    async fn generic_editable(&self, k: &ApiKind, ns: &str, name: &str) -> Result<EditableObject> {
        self.generic_editable_impl(k, ns, name).await
    }

    async fn generic_replace(
        &self,
        k: &ApiKind,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult> {
        self.generic_replace_impl(k, ns, name, yaml, resource_version, mode)
            .await
    }

    async fn helm_list(&self, ns: Option<&str>) -> Result<Vec<fleetops_core::helm::HelmRelease>> {
        self.helm_list_impl(ns).await
    }

    async fn helm_get(
        &self,
        ns: &str,
        name: &str,
        revision: Option<u32>,
    ) -> Result<fleetops_core::helm::ReleaseDetail> {
        self.helm_get_impl(ns, name, revision).await
    }

    async fn helm_history(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<Vec<fleetops_core::helm::HelmRevision>> {
        self.helm_history_impl(ns, name).await
    }

    async fn helm_rollback(
        &self,
        ns: &str,
        name: &str,
        revision: u32,
        expected_current: u32,
        dry_run: bool,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<fleetops_core::helm::HelmPlan> {
        self.helm_rollback_impl(ns, name, revision, expected_current, dry_run, allowed)
            .await
    }

    async fn helm_uninstall(
        &self,
        ns: &str,
        name: &str,
        expected_current: u32,
        dry_run: bool,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<fleetops_core::helm::HelmPlan> {
        self.helm_uninstall_impl(ns, name, expected_current, dry_run, allowed)
            .await
    }
}

impl KubeClient {
    /// Every row of `kind`, up to the cap, optionally narrowed by a field selector.
    async fn list_where(
        &self,
        kind: KubeKind,
        ns: Option<&str>,
        fields: Option<&str>,
    ) -> Result<KubeList> {
        self.list_rows(self.api(kind, ns), fields, &|o| summaries::row(kind, o))
            .await
    }

    /// Every row through `api`, up to the cap, optionally narrowed by a field selector.
    pub(crate) async fn list_rows(
        &self,
        api: Api<DynamicObject>,
        fields: Option<&str>,
        row: &(dyn Fn(&DynamicObject) -> fleetops_core::kube::KubeRow + Sync),
    ) -> Result<KubeList> {
        let page = (self.list_cap as u32).clamp(1, PAGE_SIZE);
        let mut rows = Vec::new();
        let mut cont: Option<String> = None;
        let mut truncated = false;
        loop {
            let mut lp = ListParams::default().limit(page);
            if let Some(f) = fields {
                lp = lp.fields(f);
            }
            if let Some(c) = &cont {
                lp = lp.continue_token(c);
            }
            let list = timed(api.list(&lp)).await?;
            rows.extend(list.items.iter().map(row));
            cont = list.metadata.continue_.filter(|c| !c.is_empty());
            if rows.len() >= self.list_cap {
                truncated = rows.len() > self.list_cap || cont.is_some();
                rows.truncate(self.list_cap);
                break;
            }
            if cont.is_none() {
                break;
            }
        }
        rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
        Ok(KubeList { rows, truncated })
    }

    /// Events about one object of `kind_name`, newest first.
    pub(crate) async fn object_events(
        &self,
        kind_name: &str,
        namespaced: bool,
        ns: &str,
        name: &str,
    ) -> Result<Vec<KubeEvent>> {
        let events: Api<Event> = if namespaced {
            Api::namespaced(self.client.clone(), ns)
        } else {
            Api::all(self.client.clone())
        };
        let fields = format!("involvedObject.name={name},involvedObject.kind={kind_name}");
        let list = timed(events.list(&ListParams::default().fields(&fields))).await?;
        let mut out: Vec<KubeEvent> = list
            .items
            .into_iter()
            .map(|e| KubeEvent {
                type_: e.type_.unwrap_or_else(|| "Normal".into()),
                reason: e.reason.unwrap_or_default(),
                message: e.message.unwrap_or_default(),
                count: e.count.unwrap_or(1),
                last_seen: e
                    .last_timestamp
                    .map(|t| t.0.as_second())
                    .or_else(|| e.event_time.map(|t| t.0.as_second())),
                source: e.source.and_then(|s| s.component),
            })
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.last_seen));
        Ok(out)
    }

    /// Replaces one object through `api`, refusing with `Conflict` when it
    /// changed since `resource_version`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn replace_in(
        &self,
        api: Api<DynamicObject>,
        kind_name: &str,
        namespaced: bool,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult> {
        if resource_version.trim().is_empty() {
            return Err(Error::Invalid(
                "an edit needs the resource version it was opened at".into(),
            ));
        }
        let mut docs = crate::apply::split(yaml)?;
        if docs.len() != 1 {
            return Err(Error::Invalid("edit one object at a time".into()));
        }
        let mut doc = docs.remove(0);
        if doc.kind != kind_name || doc.name != name {
            return Err(Error::Invalid(format!(
                "the YAML names {} {}, but this is {} {name}",
                doc.kind, doc.name, kind_name
            )));
        }
        let ns_opt = namespaced.then_some(ns);
        if doc.namespace.is_some() && doc.namespace.as_deref() != ns_opt {
            return Err(Error::Invalid(
                "an edit cannot move an object to another namespace".into(),
            ));
        }
        crate::apply::set_namespace(&mut doc, namespaced, ns);
        let live = serde_json::to_value(timed(api.get(name)).await?).unwrap_or_default();
        crate::mask::unmask(&mut doc.value, Some(&live)).map_err(Error::Invalid)?;
        crate::mask::restore_hidden(&mut doc.value, &live);
        doc.value["metadata"]["resourceVersion"] = resource_version.into();
        let obj: DynamicObject = serde_json::from_value(doc.value.clone())
            .map_err(|e| Error::Invalid(format!("not a Kubernetes object: {e}")))?;
        let pp = PostParams {
            dry_run: mode.dry_run(),
            field_manager: Some(FIELD_MANAGER.into()),
        };
        let out = match timed(api.replace(name, &pp, &obj)).await {
            Err(Error::Conflict(_)) => {
                return Err(Error::Conflict("changed since you opened it".into()))
            }
            r => r?,
        };
        let (action, before, after) =
            outcome(Some(live), serde_json::to_value(out).unwrap_or_default())?;
        let mut res = doc.result();
        res.action = Some(action);
        res.before = before;
        res.after = Some(after);
        Ok(res)
    }

    /// Counts from metadata only: one page of one item usually carries
    /// `remainingItemCount`; when it doesn't, page through names.
    async fn count_impl(&self, kind: KubeKind, ns: Option<&str>) -> Result<u64> {
        let api = self.api(kind, ns);
        let first = timed(api.list_metadata(&ListParams::default().limit(1))).await?;
        let mut cont = first.metadata.continue_.clone().filter(|c| !c.is_empty());
        if cont.is_none() {
            return Ok(first.items.len() as u64);
        }
        if let Some(rest) = first.metadata.remaining_item_count {
            return Ok(first.items.len() as u64 + rest.max(0) as u64);
        }
        let mut n = first.items.len() as u64;
        while let Some(c) = cont {
            let page = timed(
                api.list_metadata(&ListParams::default().limit(PAGE_SIZE).continue_token(&c)),
            )
            .await?;
            n += page.items.len() as u64;
            cont = page.metadata.continue_.filter(|c| !c.is_empty());
        }
        Ok(n)
    }
}

/// What a suspend or resume decides from: the replicas asked for and the
/// count a FleetOps suspend kept. Status and the version are not part of it.
pub type ScaleState = (i64, Option<String>);

/// How often a suspend retries when only status moved under it.
const SUSPEND_ATTEMPTS: u32 = 5;

pub fn scale_state(obj: &DynamicObject) -> ScaleState {
    let want = obj.data["spec"]["replicas"].as_i64().unwrap_or(1);
    let kept = obj
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(SUSPENDED_ANNOTATION))
        .cloned();
    (want, kept)
}

fn suspend_conflict(name: &str, on: bool) -> Error {
    Error::Conflict(format!(
        "{name} changed while it was being {}; try again",
        if on { "suspended" } else { "resumed" }
    ))
}

/// The merge patch that suspends (`on`) or resumes `obj`, preconditioned on
/// its version; `None` when there is nothing to do.
fn suspend_patch(
    name: &str,
    on: bool,
    obj: &DynamicObject,
    (want, kept): ScaleState,
) -> Result<Option<serde_json::Value>> {
    let rv = obj.metadata.resource_version.clone();
    let patch = match (on, kept) {
        (true, Some(_)) if want == 0 => return Ok(None),
        (true, _) if want == 0 => {
            return Err(Error::Invalid(format!(
                "{name} is already at 0 replicas; there is nothing to suspend"
            )))
        }
        (true, _) => json!({
            "metadata": { "resourceVersion": rv, "annotations": { SUSPENDED_ANNOTATION: want.to_string() } },
            "spec": { "replicas": 0 },
        }),
        (false, None) => {
            return Err(Error::Invalid(format!(
                "{name} was not suspended by FleetOps; scale it instead"
            )))
        }
        (false, Some(n)) => {
            let n: i64 = n.trim().parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                Error::Invalid(format!(
                    "{name} has an unreadable {SUSPENDED_ANNOTATION} ('{n}'); scale it instead"
                ))
            })?;
            // Scaled up by hand since: only the note is left to clear.
            let spec = if want == 0 {
                json!({ "replicas": n })
            } else {
                json!({})
            };
            json!({
                "metadata": { "resourceVersion": rv, "annotations": { SUSPENDED_ANNOTATION: null } },
                "spec": spec,
            })
        }
    };
    Ok(Some(patch))
}

/// The Job `kubectl create job --from=cronjob/<name>` makes: the CronJob's
/// template, marked as started by hand and owned by the CronJob, so its
/// history limits and the CronJob's deletion apply to it too.
pub fn manual_job(cj: &CronJob, now: i64) -> Result<Job> {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
    let name = cj.metadata.name.clone().unwrap_or_default();
    let uid = cj.metadata.uid.clone().ok_or_else(|| {
        Error::Invalid(format!("{name} has no uid; it may still be being created"))
    })?;
    let spec = cj
        .spec
        .as_ref()
        .ok_or_else(|| Error::Invalid(format!("{name} has no spec")))?;
    let template = spec.job_template.clone();
    let meta = template.metadata.unwrap_or_default();
    // Job names are DNS labels (63); CronJob names are at most 52.
    let suffix = format!("-manual-{}", now % 100_000);
    let base: String = name.chars().take(63 - suffix.len()).collect();
    let mut annotations = meta.annotations.unwrap_or_default();
    annotations.insert("cronjob.kubernetes.io/instantiate".into(), "manual".into());
    Ok(Job {
        metadata: ObjectMeta {
            name: Some(format!("{}{suffix}", base.trim_end_matches('-'))),
            namespace: cj.metadata.namespace.clone(),
            labels: meta.labels,
            annotations: Some(annotations),
            owner_references: Some(vec![OwnerReference {
                api_version: "batch/v1".into(),
                kind: "CronJob".into(),
                name,
                uid,
                controller: Some(true),
                block_owner_deletion: Some(true),
            }]),
            ..ObjectMeta::default()
        },
        spec: template.spec,
        status: None,
    })
}
