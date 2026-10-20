//! Deploys a restart or a lost agent cut off (stacks spec §4, "interrupted
//! deploys"): a revision still `deploying` with no deploy running is
//! restored and marked failed.

use super::restore;
use crate::AppState;
use fleetops_core::docker::DockerOps;
use fleetops_core::Result;
use fleetops_store::StackRecord;
use std::time::Duration;

pub const INTERRUPTED: &str = "interrupted";

/// Restores `stack` if its latest revision is still `deploying` and no
/// deploy of it is running here.
pub async fn recover_stack(s: &AppState, ops: &dyn DockerOps, stack: &StackRecord) -> Result<()> {
    if s.stack_locks.lock().unwrap().contains(&stack.id) {
        return Ok(());
    }
    let Some(latest) = s.store.list_revisions(stack.id).await?.into_iter().next() else {
        return Ok(());
    };
    if latest.status != "deploying" {
        return Ok(());
    }
    let msg = match restore(ops, &stack.name, latest.number).await {
        Ok(_) => INTERRUPTED.to_string(),
        Err(e) => format!("{INTERRUPTED}; restore: {e}"),
    };
    tracing::info!(stack = %stack.name, revision = latest.number, "recovered an interrupted deploy");
    s.store
        .finish_revision(stack.id, latest.number, Some(&msg))
        .await
}

/// Recovers every interrupted deploy whose environment is reachable now;
/// the rest wait for a later pass.
pub async fn recover_all(s: &AppState) {
    let pending = match s.store.deploying_revisions().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "listing interrupted deploys failed");
            return;
        }
    };
    for (stack, _) in pending {
        let Ok(env) = s.store.get_environment(stack.env_id).await else {
            continue;
        };
        let Some(ops) = s.envs.get(&env.name).ok().and_then(|e| e.docker()) else {
            continue;
        };
        if let Err(e) = recover_stack(s, ops.as_ref(), &stack).await {
            tracing::warn!(error = %e, stack = %stack.name, "recovering a deploy failed");
        }
    }
}

/// At startup, then every minute.
pub async fn run_recovery(s: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        recover_all(&s).await;
    }
}
