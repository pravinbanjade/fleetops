//! `KubeOps` over a kube-rs client, the same code whether the client
//! talks to the apiserver directly or through an agent tunnel.

use crate::{errors, kinds::api_resource, mask, summaries};
use async_trait::async_trait;
use fleetops_core::kube::{
    ApplyMode, ApplyResult, EditableObject, KubeEvent, KubeExec, KubeKind, KubeList, KubeObject,
    KubeOps, KubeRef, KubeVersion, WatchEvent, LIST_CAP, PAGE_SIZE,
};
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{now_unix, Error, Result};
use futures_util::{AsyncBufReadExt, SinkExt, StreamExt, TryStreamExt};
use k8s_openapi::api::{
    apps::v1::ReplicaSet,
    core::v1::{Event, Node, Pod},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::api::{
    Api, AttachParams, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams,
    TerminalSize,
};
use serde_json::json;
use std::{
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

pub struct KubeClient {
    client: kube::Client,
    list_cap: usize,
    nodes: Mutex<Option<(Instant, (u32, u32))>>,
}

impl KubeClient {
    pub fn new(client: kube::Client) -> Self {
        Self {
            client,
            list_cap: LIST_CAP,
            nodes: Mutex::new(None),
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

/// Runs one apiserver call with the request timeout and error mapping.
async fn timed<T>(f: impl Future<Output = kube::Result<T>>) -> Result<T> {
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
        let api = self.api(kind, ns);
        let page = (self.list_cap as u32).clamp(1, PAGE_SIZE);
        let mut rows = Vec::new();
        let mut cont: Option<String> = None;
        let mut truncated = false;
        loop {
            let mut lp = ListParams::default().limit(page);
            if let Some(c) = &cont {
                lp = lp.continue_token(c);
            }
            let list = timed(api.list(&lp)).await?;
            rows.extend(list.items.iter().map(|o| summaries::row(kind, o)));
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
            } else {
                vec![]
            },
        })
    }

    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        let events: Api<Event> = if kind.cluster_scoped() {
            Api::all(self.client.clone())
        } else {
            Api::namespaced(self.client.clone(), ns)
        };
        let fields = format!(
            "involvedObject.name={name},involvedObject.kind={}",
            kind.kind_name()
        );
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

    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        let pods = self.pods(ns);
        let (pod, o) = (pod.to_string(), opts.clone());
        let lines = futures_util::stream::once(async move {
            let container = match o.container.clone() {
                Some(c) => c,
                None => first_container(&pods, &pod).await?,
            };
            let lp = LogParams {
                container: Some(container),
                follow: o.follow,
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
                })
                .map_err(stream_err)
            }))
        })
        .try_flatten();
        Box::pin(lines)
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
        crate::watch::watch(self.api(kind, ns), kind)
    }

    async fn get_editable(
        &self,
        _kind: KubeKind,
        _ns: &str,
        _name: &str,
    ) -> Result<EditableObject> {
        Err(Error::Invalid("editing is not available yet".into()))
    }

    async fn apply(
        &self,
        _yaml: &str,
        _default_ns: &str,
        _mode: ApplyMode,
    ) -> Result<Vec<ApplyResult>> {
        Err(Error::Invalid("apply is not available yet".into()))
    }

    async fn replace(
        &self,
        _kind: KubeKind,
        _ns: &str,
        _name: &str,
        _yaml: &str,
        _rv: &str,
        _mode: ApplyMode,
    ) -> Result<ApplyResult> {
        Err(Error::Invalid("editing is not available yet".into()))
    }
}
