//! The five subscription topics. Each resolves and authorises what it
//! needs, starts a producer task that fills the subscription's queue, and
//! returns the subscription.

use super::conn::Sub;
use super::protocol;
use super::queue::{Policy, SubQueue};
use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::streams::LogOptions;
use fleetops_core::{Error, Result};
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
    })
}
