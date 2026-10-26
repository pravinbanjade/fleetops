//! The subscription topics. Each resolves and authorises what it
//! needs, starts a producer task that fills the subscription's queue, and
//! returns the subscription.

use super::conn::{ExecCtl, Resize, Sub};
use super::protocol::{self, encode_binary, Out};
use super::queue::{Policy, SubQueue};
use super::watches::Watched;
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    tasks::{TaskEvent, TaskStatus},
    AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::kube::{KubeFilter, KubeKind};
use fleetops_core::perm::{row_of, Permission, Row, Verb};
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions};
use fleetops_core::swarm::SwarmOps;
use fleetops_core::{now_unix, Error, Result, Role};
use fleetops_store::NewAudit;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast::error::RecvError, Notify};

/// Logs are flushed to the client at most this often per subscription.
pub const LOG_FLUSH: Duration = Duration::from_millis(50);
/// Most lines in one logs `data` message.
pub const LOG_BATCH_MAX: usize = 500;

/// What every topic needs from the connection.
pub(super) struct Ctx<'a> {
    pub s: &'a AppState,
    pub cu: &'a CurrentUser,
    pub sub: u32,
    pub ready: Arc<Notify>,
}

impl Ctx<'_> {
    fn queue(&self, policy: Policy) -> Arc<SubQueue> {
        let cap = self.s.config.ws_queue_capacity.max(2);
        Arc::new(SubQueue::new(self.sub, policy, cap, self.ready.clone()))
    }
}

fn required<'a>(v: &'a Option<String>, what: &str) -> Result<&'a str> {
    v.as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Invalid(format!("{what} is required")))
}

fn opts<T: serde::de::DeserializeOwned + Default>(v: Value) -> Result<T> {
    if v.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(v).map_err(|e| Error::Invalid(format!("bad opts: {e}")))
}

/// A Docker environment as a topic sees it; on a swarm node's view, with
/// the stacks a grant limited to stacks covers (swarm spec §6).
struct DockerTopic {
    env: Arc<dyn Environment>,
    ops: Arc<dyn DockerOps>,
    stacks: Option<std::collections::BTreeSet<String>>,
}

/// Resolves the environment, authorises, and returns its Docker ops;
/// with `id`, checks that container's stack on a node's view.
async fn docker_scoped(
    s: &AppState,
    cu: &CurrentUser,
    env: &Option<String>,
    need: Need,
    id: Option<&str>,
) -> Result<DockerTopic> {
    let name = required(env, "env")?;
    let (env, grants) = crate::authz::env_access(s, cu, name, need).await?;
    if env.kind().is_kube() || env.kind().is_swarm() {
        return Err(crate::routes::kube::wrong_kind(name, env.kind()));
    }
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    let stacks = if env.kind() == fleetops_core::env::EnvKind::SwarmNode {
        crate::routes::docker::node_scope(&grants, need)
    } else {
        None
    };
    if let (Some(id), Need::Perm(p)) = (id, need) {
        crate::routes::docker::check_container(s, cu, &env, &ops, &grants, &stacks, p, id).await?;
    }
    Ok(DockerTopic { env, ops, stacks })
}

/// `<namespace>/<pod>`, the id of a pod on the logs and exec topics.
fn pod_id(id: &Option<String>) -> Result<(String, String)> {
    let id = required(id, "id")?;
    match id.split_once('/') {
        Some((ns, pod)) if !pod.contains('/') => {
            crate::routes::kube::check_namespace(ns)?;
            crate::routes::kube::check_name(pod)?;
            Ok((ns.into(), pod.into()))
        }
        _ => Err(Error::Invalid(
            "a Kubernetes pod id is <namespace>/<pod>".into(),
        )),
    }
}

/// What a Kubernetes `logs` subscription reads (spec 3i §5).
enum LogTarget {
    Pod(String, String),
    Workload(KubeKind, String, String),
}

/// `<namespace>/<pod>` or `<kind>/<namespace>/<name>`.
fn log_target(id: &Option<String>) -> Result<LogTarget> {
    let raw = required(id, "id")?;
    match raw.split('/').collect::<Vec<_>>().as_slice() {
        [_, _] => {
            let (ns, pod) = pod_id(id)?;
            Ok(LogTarget::Pod(ns, pod))
        }
        [kind, ns, name] => {
            let refused = || Error::Invalid("logs are read from pods or workloads".into());
            let k = kind
                .parse::<KubeKind>()
                .ok()
                .filter(|k| k.has_pod_logs())
                .ok_or_else(refused)?;
            if *ns == "-" {
                return Err(refused());
            }
            crate::routes::kube::check_namespace(ns)?;
            crate::routes::kube::check_name(name)?;
            Ok(LogTarget::Workload(k, ns.to_string(), name.to_string()))
        }
        _ => Err(Error::Invalid(
            "a Kubernetes logs id is <namespace>/<pod> or <kind>/<namespace>/<name>".into(),
        )),
    }
}

fn is_swarm_env(s: &AppState, env: &Option<String>) -> bool {
    env.as_deref()
        .and_then(|n| s.envs.get(n).ok())
        .is_some_and(|e| e.kind().is_swarm())
}

/// The swarm's own events, as cluster pages need them (swarm spec §7): not
/// the serving manager's local ones (its containers, local networks).
fn is_swarm_event(e: &fleetops_core::streams::DockerEvent) -> bool {
    e.scope.as_deref() == Some("swarm")
}

/// The view cell a swarm event's kind falls under; unknown kinds go nowhere.
fn view_of(kind: &str) -> Option<Permission> {
    let row = match kind {
        "service" => Row::SwarmServices,
        "node" => Row::SwarmNodes,
        "secret" => Row::SwarmSecrets,
        "config" => Row::SwarmConfigs,
        "network" => Row::Networks,
        _ => return None,
    };
    Some(Permission::new(row, Verb::View))
}

/// Which stack each swarm object is in, for one subscriber limited to
/// stacks (swarm spec §6). Swarm events carry no labels, so the lists are
/// read when the subscription starts, and again (at most once per `window`
/// per kind, waiting it out) for an id not seen yet or an update, which
/// may have moved the object to another stack. Nothing is forgotten, so an
/// object in scope is still announced when it is removed; one made and
/// removed before a read could see it can't be placed, and is not.
struct StackCache {
    known: HashMap<(String, String), Option<String>>,
    refreshed: HashMap<String, std::time::Instant>,
    window: std::time::Duration,
}

impl Default for StackCache {
    fn default() -> Self {
        Self {
            known: HashMap::new(),
            refreshed: HashMap::new(),
            window: std::time::Duration::from_secs(1),
        }
    }
}

type StackList = Vec<(String, Option<String>)>;

/// Every object of `kind` with its stack; `None` without a manager or on a
/// failed read.
async fn stack_list(ops: Option<Arc<dyn SwarmOps>>, kind: &str) -> Option<StackList> {
    let ops = ops?;
    match kind {
        "service" => ops
            .services()
            .await
            .ok()
            .map(|l| l.into_iter().map(|x| (x.id, x.stack)).collect()),
        "secret" => ops
            .secrets()
            .await
            .ok()
            .map(|l| l.into_iter().map(|x| (x.id, x.stack)).collect()),
        "config" => ops
            .configs()
            .await
            .ok()
            .map(|l| l.into_iter().map(|x| (x.id, x.stack)).collect()),
        "network" => ops
            .networks()
            .await
            .ok()
            .map(|l| l.into_iter().map(|x| (x.id, x.stack)).collect()),
        _ => None,
    }
}

impl StackCache {
    fn learn(&mut self, kind: &str, items: impl IntoIterator<Item = (String, Option<String>)>) {
        for (id, stack) in items {
            self.known.insert((kind.to_string(), id), stack);
        }
    }

    fn get(&self, kind: &str, id: &str) -> Option<Option<String>> {
        self.known.get(&(kind.to_string(), id.to_string())).cloned()
    }

    /// Reads `kind` again, once its window since the last read has passed.
    async fn refresh<F, Fut>(&mut self, kind: &str, fetch: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<StackList>>,
    {
        if let Some(t) = self.refreshed.get(kind) {
            let wait = self.window.saturating_sub(t.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        self.refreshed
            .insert(kind.to_string(), std::time::Instant::now());
        if let Some(items) = fetch().await {
            self.learn(kind, items);
        }
    }

    /// The stack of `kind` `id` as of an event with `action`; `None` when
    /// it is in no stack or unknown.
    async fn stack<F, Fut>(
        &mut self,
        kind: &str,
        id: &str,
        action: &str,
        fetch: F,
    ) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<StackList>>,
    {
        if action != "update" {
            if let Some(stack) = self.get(kind, id) {
                return stack;
            }
        }
        self.refresh(kind, fetch).await;
        self.get(kind, id).flatten()
    }
}

fn is_kube_env(s: &AppState, env: &Option<String>) -> bool {
    env.as_deref()
        .and_then(|n| s.envs.get(n).ok())
        .is_some_and(|e| e.kind().is_kube())
}

/// Docker events for the environment, optionally only those whose actor id
/// or name is `id`. One shared Docker stream per environment.
pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    // A swarm's events come from its serving manager: only the cluster's
    // own kinds go on, not that one node's containers.
    let swarm_env = is_swarm_env(s, &env);
    // On a node's view, a grant limited to stacks hears of their
    // containers only.
    let mut node_stacks: Option<std::collections::BTreeSet<String>> = None;
    let (env, grants) = if swarm_env {
        // The hub waits out a manager outage and says so (status down/up).
        let (env, grants) =
            crate::routes::swarm::swarm_env_scoped(s, cu, required(&env, "env")?, Need::AnyGrant)
                .await?;
        (env, Some(grants))
    } else {
        let d = docker_scoped(s, cu, &env, Need::AnyGrant, None).await?;
        node_stacks = d.stacks;
        (d.env, None)
    };
    let q = c.queue(Policy::DropOldest);
    let mut rx = s
        .hubs
        .events(&env)
        .subscribe(env.clone(), s.hubs.status(), s.config.ws_backoff);
    let filter = id.filter(|i| !i.is_empty());
    let qq = q.clone();
    let swarm = env;
    let task = tokio::spawn(async move {
        let mut cache = StackCache::default();
        if let Some(g) = &grants {
            // Only a grant limited to stacks needs to know them.
            if g.0.iter().any(|g| g.namespaces.is_some()) {
                for kind in ["service", "secret", "config", "network"] {
                    cache
                        .refresh(kind, || stack_list(swarm.swarm(), kind))
                        .await;
                }
            }
        }
        loop {
            match rx.recv().await {
                Ok(e) => {
                    if swarm_env && !is_swarm_event(&e) {
                        continue;
                    }
                    if let Some(set) = &node_stacks {
                        let stack = e.attributes.get(fleetops_core::swarm::STACK_LABEL);
                        if e.kind != "container" || !stack.is_some_and(|st| set.contains(st)) {
                            continue;
                        }
                    }
                    // Each kind needs its view cell: nodes anywhere, the rest
                    // on the object's stack.
                    if let Some(g) = &grants {
                        let Some(p) = view_of(&e.kind) else { continue };
                        if !g.allows(p, None) {
                            let pass = if e.kind == "node" {
                                g.holds(p)
                            } else {
                                let stack = cache
                                    .stack(&e.kind, &e.id, &e.action, || {
                                        stack_list(swarm.swarm(), &e.kind)
                                    })
                                    .await;
                                g.allows_stack(p, stack.as_deref())
                            };
                            if !pass {
                                continue;
                            }
                        }
                    }
                    if filter
                        .as_ref()
                        .is_some_and(|f| *f != e.id && e.name.as_ref() != Some(f))
                    {
                        continue;
                    }
                    let v = serde_json::to_value(&e).unwrap_or(Value::Null);
                    if !qq.push(protocol::data(sub, v), 1) {
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    qq.finish(protocol::end(sub, "closed", Value::Null));
                    return;
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: None,
    })
}

/// Stats samples for container `id`, shared with every other subscriber.
pub(super) async fn stats(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let id = required(&id, "id")?.to_string();
    let d = docker_scoped(
        s,
        cu,
        &env,
        Need::of(Row::Containers, Verb::View),
        Some(&id),
    )
    .await?;
    let (env, ops) = (d.env, d.ops);
    let (handle, mut rx) = s.hubs.stats(&env).subscribe(ops, &id);
    let q = c.queue(Policy::DropOldest);
    let qq = q.clone();
    let task = tokio::spawn(async move {
        let _keep_stream_alive = handle;
        loop {
            match rx.recv().await {
                Ok(Some(sample)) => {
                    let v = serde_json::to_value(&sample).unwrap_or(Value::Null);
                    if !qq.push(protocol::data(sub, v), 1) {
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Ok(None) | Err(RecvError::Closed) => {
                    qq.finish(protocol::end(sub, "ended", Value::Null));
                    return;
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: None,
    })
}

/// A container's or pod's logs as batches of lines, at most every
/// `LOG_FLUSH`. On a Kubernetes environment `id` is `<namespace>/<pod>`.
pub(super) async fn logs(
    c: &Ctx<'_>,
    env: Option<String>,
    id: Option<String>,
    opts_v: Value,
) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let o: LogOptions = opts(opts_v)?;
    let lines = if is_kube_env(s, &env) {
        let target = log_target(&id)?;
        let (ns, view) = match &target {
            LogTarget::Pod(ns, _) => (ns.clone(), Permission::new(Row::Pods, Verb::View)),
            LogTarget::Workload(_, ns, _) => {
                (ns.clone(), Permission::new(Row::Workloads, Verb::View))
            }
        };
        let crate::routes::kube::KubeCtx { env, ops, grants } =
            crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Need::Perm(view)).await?;
        crate::authz::deny_audited(
            s,
            cu,
            env.id(),
            &view.to_string(),
            grants.check_ns(view, &ns),
        )
        .await?;
        match target {
            LogTarget::Pod(ns, pod) => ops.logs(&ns, &pod, &o),
            LogTarget::Workload(k, ns, name) => ops.workload_logs(k, &ns, &name, &o),
        }
    } else if is_swarm_env(s, &env) {
        let id = required(&id, "id")?;
        let service = id
            .strip_prefix("service/")
            .filter(|i| !i.is_empty())
            .ok_or_else(|| Error::Invalid("a swarm logs id is service/<id>".into()))?;
        // Service logs fall under the service row's View, on its stack
        // (spec §6).
        let view = Permission::new(Row::SwarmServices, Verb::View);
        let (env, grants) =
            crate::routes::swarm::swarm_env_scoped(s, cu, required(&env, "env")?, Need::Perm(view))
                .await?;
        let ops = env
            .swarm()
            .ok_or_else(|| Error::EnvUnavailable("no manager agent is connected".into()))?;
        if !grants.allows(view, None) {
            let missing = || Error::NotFound(format!("no such service '{service}'"));
            let stack = ops
                .service(service)
                .await?
                .ok_or_else(missing)?
                .summary
                .stack;
            crate::authz::hidden_audited(
                s,
                cu,
                env.id(),
                &view.to_string(),
                grants.check_stack(view, stack.as_deref()),
                missing(),
            )
            .await?;
        }
        ops.service_logs(service, &o)
    } else {
        let id = required(&id, "id")?;
        let d = docker_scoped(s, cu, &env, Need::of(Row::Containers, Verb::View), Some(id)).await?;
        d.ops.logs(id, &o)
    };
    let q = c.queue(Policy::Gap);
    let task = pump_logs(q.clone(), sub, lines);
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: None,
    })
}

fn pump_logs(
    qq: Arc<SubQueue>,
    sub: u32,
    mut lines: BoxStream<LogLine>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let first = match lines.next().await {
                Some(Ok(l)) => l,
                Some(Err(e)) => {
                    return qq.finish(protocol::error(Some(sub), e.code(), &e.to_string()))
                }
                None => return qq.finish(protocol::end(sub, "eof", Value::Null)),
            };
            let mut batch = vec![first];
            let deadline = tokio::time::Instant::now() + LOG_FLUSH;
            let mut ended = None;
            while batch.len() < LOG_BATCH_MAX {
                match tokio::time::timeout_at(deadline, lines.next()).await {
                    Ok(Some(Ok(l))) => batch.push(l),
                    Ok(Some(Err(e))) => {
                        ended = Some(protocol::error(Some(sub), e.code(), &e.to_string()));
                        break;
                    }
                    Ok(None) => {
                        ended = Some(protocol::end(sub, "eof", Value::Null));
                        break;
                    }
                    Err(_) => break,
                }
            }
            let n = batch.len() as u64;
            if !qq.push(protocol::data(sub, json!(batch)), n) {
                return;
            }
            if let Some(last) = ended {
                return qq.finish(last);
            }
        }
    })
}

/// An interactive TTY in container `id`: output as binary frames, input
/// through `input` frames, resized with `resize`.
pub(super) async fn exec(
    c: &Ctx<'_>,
    env: Option<String>,
    id: Option<String>,
    opts_v: Value,
) -> Result<Sub> {
    if is_kube_env(c.s, &env) {
        return kube_exec(c, env, id, opts_v).await;
    }
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let id = required(&id, "id")?.to_string();
    let d = docker_scoped(
        s,
        cu,
        &env,
        Need::of(Row::Containers, Verb::Shell),
        Some(&id),
    )
    .await?;
    let (env, ops) = (d.env, d.ops);
    let o: ExecOptions = opts(opts_v)?;
    let session = ops.exec_start(&id, &o).await;
    // Only the error text crosses the await: the session is not Sync.
    let failure = session.as_ref().err().map(|e| e.to_string());
    audit_exec(s, cu, env.id(), &id, &o, failure).await;
    let session = session?;
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let mut output = session.output;
    let exec_id = session.exec_id.clone();
    let task_ops = ops.clone();
    let task = tokio::spawn(async move {
        while let Some(chunk) = output.next().await {
            let Ok(bytes) = chunk else { break };
            // Waits while the client is behind: Docker's output is
            // read no faster than the socket drains it.
            if !qq.push_wait(Out::Binary(encode_binary(sub, &bytes))).await {
                return;
            }
        }
        let mut code = None;
        for _ in 0..20 {
            code = task_ops.exec_exit_code(&exec_id).await.ok().flatten();
            if code.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        qq.finish(protocol::end(sub, "exited", json!({ "exit_code": code })));
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: Some(ExecCtl {
            input: session.input,
            resize: Resize::Docker {
                ops,
                exec_id: session.exec_id,
            },
        }),
    })
}

/// A TTY in a pod's container. Ends with `exited` when the output ends;
/// the exit code is whatever the process reported within a second.
async fn kube_exec(
    c: &Ctx<'_>,
    env: Option<String>,
    id: Option<String>,
    opts_v: Value,
) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let shell = Permission::new(Row::Pods, Verb::Shell);
    let crate::routes::kube::KubeCtx { env, ops, grants } =
        crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Need::Perm(shell)).await?;
    let (ns, pod) = pod_id(&id)?;
    crate::authz::deny_audited(s, cu, env.id(), "pods.shell", grants.check_ns(shell, &ns)).await?;
    let o: ExecOptions = opts(opts_v)?;
    let session = ops.exec_start(&ns, &pod, &o).await;
    let failure = session.as_ref().err().map(|e| e.to_string());
    audit_kube_exec(s, cu, env.id(), &format!("pods/{ns}/{pod}"), &o, failure).await;
    let session = session?;
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let mut output = session.output;
    let exit = session.exit_code;
    let task = tokio::spawn(async move {
        while let Some(chunk) = output.next().await {
            let Ok(bytes) = chunk else { break };
            if !qq.push_wait(Out::Binary(encode_binary(sub, &bytes))).await {
                return;
            }
        }
        let code = tokio::time::timeout(Duration::from_secs(1), exit)
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten();
        qq.finish(protocol::end(sub, "exited", json!({ "exit_code": code })));
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: Some(ExecCtl {
            input: session.input,
            resize: Resize::Kube(session.resize),
        }),
    })
}

async fn audit_kube_exec(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    target: &str,
    o: &ExecOptions,
    failure: Option<String>,
) {
    let (result, mut detail) = match failure {
        None => ("success", json!({})),
        Some(e) => ("failure", json!({ "error": e })),
    };
    detail["cmd"] = json!(o.command());
    detail["container"] = json!(o.container);
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action: "k8s.exec",
        target_type: Some("k8s"),
        target_id: Some(target),
        result,
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

enum WatchId {
    Curated(KubeKind, Option<String>, Option<KubeFilter>),
    /// Namespace `None` for all namespaces; resolved against discovery later.
    Generic(fleetops_core::kube::KindRef, Option<String>),
}

/// `<kind>/<namespace>[/<filter>]` or `r/<group>/<version>/<resource>/<namespace>`,
/// with `-` for all namespaces or a cluster-scoped kind, e.g. `events/-/warnings`.
fn watch_id(id: &Option<String>) -> Result<WatchId> {
    let id = required(id, "id")?;
    if let Some(rest) = id.strip_prefix("r/") {
        let bad = || {
            Error::Invalid(
                "a generic watch id is r/<group>/<version>/<resource>/<namespace or ->".into(),
            )
        };
        let parts: Vec<&str> = rest.split('/').collect();
        let [group, version, resource, ns] = parts[..] else {
            return Err(bad());
        };
        crate::routes::kube::check_namespace(ns)?;
        let r = crate::routes::kube_generic::kind_ref(group, version, resource)?;
        return Ok(WatchId::Generic(r, (ns != "-").then(|| ns.to_string())));
    }
    let bad = || Error::Invalid("a watch id is <kind>/<namespace or ->[/<filter>]".into());
    let mut parts = id.splitn(3, '/');
    let kind: KubeKind = parts.next().ok_or_else(bad)?.parse()?;
    let ns = parts.next().ok_or_else(bad)?;
    crate::routes::kube::check_namespace(ns)?;
    let filter = parts.next().map(KubeFilter::parse).transpose()?;
    if let Some(f) = filter {
        f.check(kind)?;
    }
    Ok(WatchId::Curated(
        kind,
        (ns != "-" && !kind.cluster_scoped()).then(|| ns.to_string()),
        filter,
    ))
}

/// A live list of Kubernetes objects: a snapshot, then changes (spec §5).
/// The producer waits for the client; a client that falls behind the shared
/// watch gets a fresh snapshot rather than a gap.
pub(super) async fn kube_watch(
    c: &Ctx<'_>,
    env: Option<String>,
    id: Option<String>,
) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let crate::routes::kube::KubeCtx { env, ops, grants } =
        crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Need::AnyGrant).await?;
    let (what, ns, filter) = match watch_id(&id)? {
        WatchId::Curated(kind, ns, filter) => {
            let view = Permission::new(row_of(kind), Verb::View);
            crate::authz::deny_audited(
                s,
                cu,
                env.id(),
                &view.to_string(),
                grants.check_list(view, kind, ns.as_deref()),
            )
            .await?;
            (Watched::Curated(kind), ns, filter)
        }
        WatchId::Generic(r, ns) => {
            let k = crate::routes::kube_generic::resolve(&ops, &r).await?;
            let ns = ns.filter(|_| k.namespaced);
            let view = Permission::new(Row::K8sOther, Verb::View);
            crate::authz::deny_audited(
                s,
                cu,
                env.id(),
                &view.to_string(),
                grants.check_list_in(view, k.namespaced, ns.as_deref()),
            )
            .await?;
            (Watched::Generic(k), ns, None)
        }
    };
    let mut w = s.hubs.kube_watches.subscribe_filtered(
        &env,
        ops,
        what.clone(),
        ns,
        filter,
        s.config.kube_watch_grace,
    );
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let task = tokio::spawn(async move {
        while let Some(item) = w.next().await {
            match item {
                Ok(ev) => {
                    // The shared watch carries every namespace; this
                    // subscriber sees only its grant's.
                    let ev = match &what {
                        Watched::Curated(kind) => grants.filter_event(*kind, ev),
                        Watched::Generic(k) => grants.filter_event_in(k.namespaced, ev),
                    };
                    let Some(ev) = ev else {
                        continue;
                    };
                    let v = serde_json::to_value(&ev).unwrap_or(Value::Null);
                    if !qq.push_wait(protocol::data(sub, v)).await {
                        return;
                    }
                }
                Err(message) => {
                    return qq.finish(protocol::end(sub, "error", json!({ "message": message })))
                }
            }
        }
        qq.finish(protocol::end(sub, "closed", Value::Null));
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: None,
    })
}

/// A background task's progress: everything so far, then live, then `end`.
pub(super) async fn task(c: &Ctx<'_>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let id = required(&id, "id")?;
    let task = s
        .tasks
        .get(id)
        .filter(|t| t.owner == cu.user.id || cu.user.role == Role::Admin)
        .ok_or_else(|| Error::NotFound(format!("task '{id}'")))?;
    let feed = task.feed();
    let q = c.queue(Policy::DropOldest);
    let qq = q.clone();
    let done = move |info: crate::tasks::TaskInfo| {
        let reason = match info.status {
            TaskStatus::Succeeded => "succeeded",
            _ => "failed",
        };
        protocol::end(sub, reason, json!({ "task": info }))
    };
    let handle = tokio::spawn(async move {
        let mut live = feed.live;
        for v in feed.backlog {
            qq.push(protocol::data(sub, v), 1);
        }
        if let Some(info) = feed.done {
            return qq.finish(done(info));
        }
        loop {
            match live.recv().await {
                Ok(TaskEvent::Progress(v)) => {
                    if !qq.push(protocol::data(sub, v), 1) {
                        return;
                    }
                }
                Ok(TaskEvent::Done(info)) => return qq.finish(done(info)),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    return qq.finish(protocol::end(sub, "closed", Value::Null))
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: handle.abort_handle(),
        // Owner-checked, not grant-checked: an access change leaves it.
        keep_on_access: true,
        exec: None,
    })
}

/// The id of each new audit entry, so an open audit log fetches it at once.
/// Admins only, like the audit log itself.
pub(super) async fn audit(c: &Ctx<'_>) -> Result<Sub> {
    if c.cu.user.role != Role::Admin {
        return Err(Error::Forbidden("the audit log is for admins".into()));
    }
    let mut feed = c.s.store.audit_feed();
    let sub = c.sub;
    let q = c.queue(Policy::DropOldest);
    let qq = q.clone();
    let handle = tokio::spawn(async move {
        loop {
            match feed.recv().await {
                Ok(id) => {
                    if !qq.push(protocol::data(sub, json!({ "id": id })), 1) {
                        return;
                    }
                }
                // Missed ids only mean the page refetches; say so with the next one.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    return qq.finish(protocol::end(sub, "closed", Value::Null))
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: handle.abort_handle(),
        // A demoted admin loses it with the rest.
        keep_on_access: false,
        exec: None,
    })
}

/// Exec is root-equivalent access to the container, so every attempt is
/// audited with the command.
async fn audit_exec(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    id: &str,
    o: &ExecOptions,
    failure: Option<String>,
) {
    let (result, mut detail) = match failure {
        None => ("success", json!({})),
        Some(e) => ("failure", json!({ "error": e })),
    };
    detail["cmd"] = json!(o.command());
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action: "container.exec",
        target_type: Some("container"),
        target_id: Some(id),
        result,
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

/// A swarm's shared task feed: the snapshot, then changes.
pub(super) async fn swarm_tasks(c: &Ctx<'_>, env: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    // The feed itself reports a manager outage, and recovers.
    let view = Permission::new(Row::SwarmServices, Verb::View);
    let (env, grants) =
        crate::routes::swarm::swarm_env_scoped(s, cu, required(&env, "env")?, Need::Perm(view))
            .await?;
    let (snapshot, mut rx) = s.hubs.swarm_tasks.subscribe(env, s.config.swarm_task_poll);
    // A grant limited to stacks sees their tasks only, and hears of no
    // other task's removal.
    let mut sent: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scoped = move |m: crate::ws::swarm_tasks::FeedMsg| {
        use crate::ws::swarm_tasks::FeedMsg;
        if grants.allows(view, None) {
            return m;
        }
        let ok =
            |t: &fleetops_core::swarm::TaskSummary| grants.allows_stack(view, t.stack.as_deref());
        match m {
            FeedMsg::Snapshot { mut tasks } => {
                tasks.retain(|t| ok(t));
                sent = tasks.iter().map(|t| t.id.clone()).collect();
                FeedMsg::Snapshot { tasks }
            }
            FeedMsg::Changes {
                mut upsert,
                mut remove,
            } => {
                upsert.retain(|t| ok(t));
                sent.extend(upsert.iter().map(|t| t.id.clone()));
                remove.retain(|id| sent.remove(id));
                FeedMsg::Changes { upsert, remove }
            }
            paused => paused,
        }
    };
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let task = tokio::spawn(async move {
        // Waits for room rather than dropping: changes only make sense on
        // top of every message before them. A client too slow for that
        // lags the broadcast, and is told to start over.
        let data = |m: &crate::ws::swarm_tasks::FeedMsg| {
            protocol::data(sub, serde_json::to_value(m).unwrap_or(Value::Null))
        };
        if let Some(m) = snapshot {
            if !qq.push_wait(data(&scoped(m))).await {
                return;
            }
        }
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if !qq.push_wait(data(&scoped(m))).await {
                        return;
                    }
                }
                // Too far behind to patch: start over from the next read.
                Err(RecvError::Lagged(_)) => {
                    qq.finish(protocol::end(sub, "lagged", Value::Null));
                    return;
                }
                Err(RecvError::Closed) => {
                    qq.finish(protocol::end(sub, "closed", Value::Null));
                    return;
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        keep_on_access: false,
        exec: None,
    })
}

#[cfg(test)]
mod stack_cache_tests {
    use super::StackCache;

    #[test]
    fn an_object_once_seen_keeps_its_stack() {
        let mut c = StackCache::default();
        c.learn(
            "service",
            [("s1".into(), Some("blog".into())), ("s2".into(), None)],
        );
        // A later read no longer lists s1: it was removed.
        c.learn("service", [("s2".into(), None)]);
        assert_eq!(c.get("service", "s1"), Some(Some("blog".into())));
        assert_eq!(c.get("service", "s2"), Some(None));
        assert_eq!(c.get("service", "s9"), None);
    }

    type List = Vec<(String, Option<String>)>;

    fn list(items: &[(&str, &str)]) -> Option<List> {
        Some(
            items
                .iter()
                .map(|(id, st)| (id.to_string(), Some(st.to_string())))
                .collect(),
        )
    }

    fn quick() -> StackCache {
        StackCache {
            window: std::time::Duration::from_millis(50),
            ..Default::default()
        }
    }

    /// An admin may move a service to another stack: its update event is
    /// placed by what it is in now.
    #[tokio::test]
    async fn an_update_reads_the_stack_again() {
        let mut c = quick();
        c.learn("service", list(&[("s1", "blog")]).unwrap());
        let got = c
            .stack("service", "s1", "update", || async {
                list(&[("s1", "shop")])
            })
            .await;
        assert_eq!(got.as_deref(), Some("shop"));
    }

    /// An object made just after a refresh is still found: the next read
    /// waits out the rate limit instead of giving up.
    #[tokio::test]
    async fn a_new_object_right_after_a_refresh_is_found() {
        let mut c = quick();
        let first = c
            .stack("secret", "a", "create", || async { list(&[("a", "blog")]) })
            .await;
        assert_eq!(first.as_deref(), Some("blog"));
        let second = c
            .stack("secret", "b", "create", || async {
                list(&[("a", "blog"), ("b", "blog")])
            })
            .await;
        assert_eq!(second.as_deref(), Some("blog"));
    }
}
