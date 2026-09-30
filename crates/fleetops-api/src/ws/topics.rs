//! The five subscription topics. Each resolves and authorises what it
//! needs, starts a producer task that fills the subscription's queue, and
//! returns the subscription.

use super::conn::{ExecCtl, Sub};
use super::protocol::{self, encode_binary, Out};
use super::queue::{Policy, SubQueue};
use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    tasks::{TaskEvent, TaskStatus},
    AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::streams::{ExecOptions, LogOptions};
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
fn docker_for(
    s: &AppState,
    cu: &CurrentUser,
    env: &Option<String>,
    action: Action,
) -> Result<(Arc<dyn Environment>, Arc<dyn DockerOps>)> {
    let name = required(env, "env")?;
    let env = s.envs.get(name)?;
    authorize(&cu.user, Some(env.id()), action)?;
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok((env, ops))
}

/// Docker events for the environment, optionally only those whose actor id
/// or name is `id`. One shared Docker stream per environment.
pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, _) = docker_for(s, cu, &env, Action::DockerRead)?;
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
    let (env, ops) = docker_for(s, cu, &env, Action::DockerRead)?;
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

/// Container `id`'s logs as batches of lines, at most every `LOG_FLUSH`.
pub(super) async fn logs(
    c: &Ctx<'_>,
    env: Option<String>,
    id: Option<String>,
    opts_v: Value,
) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (_, ops) = docker_for(s, cu, &env, Action::DockerRead)?;
    let id = required(&id, "id")?;
    let o: LogOptions = opts(opts_v)?;
    let mut lines = ops.logs(id, &o);
    let q = c.queue(Policy::Gap);
    let qq = q.clone();
    let task = tokio::spawn(async move {
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
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        exec: None,
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
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, ops) = docker_for(s, cu, &env, Action::DockerWrite)?;
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
            exec_id: session.exec_id,
            ops,
        }),
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
