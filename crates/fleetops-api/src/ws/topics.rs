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
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{broadcast::error::RecvError, Notify};

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
