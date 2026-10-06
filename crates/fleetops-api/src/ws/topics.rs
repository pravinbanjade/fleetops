//! The subscription topics. Each resolves and authorises what it
//! needs, starts a producer task that fills the subscription's queue, and
//! returns the subscription.

use super::conn::{ExecCtl, Resize, Sub};
use super::protocol::{self, encode_binary, Out};
use super::queue::{Policy, SubQueue};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    tasks::{TaskEvent, TaskStatus},
    AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::kube::KubeKind;
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions};
use fleetops_core::{now_unix, Error, Result, Role};
use fleetops_store::NewAudit;
use futures_util::StreamExt;
use serde_json::{json, Value};
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

/// Resolves the environment, authorises, and returns its Docker ops.
async fn docker_for(
    s: &AppState,
    cu: &CurrentUser,
    env: &Option<String>,
    action: Action,
) -> Result<(Arc<dyn Environment>, Arc<dyn DockerOps>)> {
    let name = required(env, "env")?;
    let (env, _) = crate::authz::env_access(s, cu, name, action).await?;
    if env.kind().is_kube() {
        return Err(crate::routes::kube::wrong_kind(name, env.kind()));
    }
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok((env, ops))
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

fn is_kube_env(s: &AppState, env: &Option<String>) -> bool {
    env.as_deref()
        .and_then(|n| s.envs.get(n).ok())
        .is_some_and(|e| e.kind().is_kube())
}

/// Docker events for the environment, optionally only those whose actor id
/// or name is `id`. One shared Docker stream per environment.
pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, _) = docker_for(s, cu, &env, Action::DockerRead).await?;
    let q = c.queue(Policy::DropOldest);
    let mut rx = s
        .hubs
        .events(&env)
        .subscribe(env, s.hubs.status(), s.config.ws_backoff);
    let filter = id.filter(|i| !i.is_empty());
    let qq = q.clone();
    let task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(e) => {
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
        exec: None,
    })
}

/// Stats samples for container `id`, shared with every other subscriber.
pub(super) async fn stats(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, ops) = docker_for(s, cu, &env, Action::DockerRead).await?;
    let id = required(&id, "id")?.to_string();
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
        let crate::routes::kube::KubeCtx { env, ops, grant } =
            crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeRead).await?;
        let (ns, pod) = pod_id(&id)?;
        crate::authz::deny_audited(s, cu, env.id(), "k8s.read", grant.check_ns(&ns)).await?;
        ops.logs(&ns, &pod, &o)
    } else {
        let (_, ops) = docker_for(s, cu, &env, Action::DockerRead).await?;
        ops.logs(required(&id, "id")?, &o)
    };
    let q = c.queue(Policy::Gap);
    let task = pump_logs(q.clone(), sub, lines);
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
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
    let (env, ops) = docker_for(s, cu, &env, Action::DockerWrite).await?;
    let id = required(&id, "id")?.to_string();
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
    let crate::routes::kube::KubeCtx { env, ops, grant } =
        crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeWrite).await?;
    let (ns, pod) = pod_id(&id)?;
    crate::authz::deny_audited(s, cu, env.id(), "k8s.write", grant.check_ns(&ns)).await?;
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

/// `<kind>/<namespace>`, with `-` for all namespaces or a cluster-scoped kind.
fn watch_id(id: &Option<String>) -> Result<(KubeKind, Option<String>)> {
    let id = required(id, "id")?;
    let (kind, ns) = id
        .split_once('/')
        .ok_or_else(|| Error::Invalid("a watch id is <kind>/<namespace or ->".into()))?;
    let kind: KubeKind = kind.parse()?;
    crate::routes::kube::check_namespace(ns)?;
    Ok((
        kind,
        (ns != "-" && !kind.cluster_scoped()).then(|| ns.to_string()),
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
    let crate::routes::kube::KubeCtx { env, ops, grant } =
        crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeRead).await?;
    let (kind, ns) = watch_id(&id)?;
    crate::authz::deny_audited(
        s,
        cu,
        env.id(),
        "k8s.read",
        grant.check_list(kind, ns.as_deref()),
    )
    .await?;
    let mut w = s
        .hubs
        .kube_watches
        .subscribe(&env, ops, kind, ns, s.config.kube_watch_grace);
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let task = tokio::spawn(async move {
        while let Some(item) = w.next().await {
            match item {
                Ok(ev) => {
                    // The shared watch carries every namespace; this
                    // subscriber sees only its grant's.
                    let Some(ev) = grant.filter_event(kind, ev) else {
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
