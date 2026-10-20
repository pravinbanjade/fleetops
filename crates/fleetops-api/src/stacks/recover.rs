//! Deploys a restart or a lost agent cut off (stacks spec §4, "interrupted
//! deploys"): a revision still `deploying` with no deploy running is
//! restored and marked failed.

use super::{restore, try_lock};
use crate::AppState;
use fleetops_core::docker::DockerOps;
use fleetops_core::Result;
use fleetops_store::StackRecord;
use std::time::Duration;

pub const INTERRUPTED: &str = "interrupted";

/// Undoes revision `number` and records why it failed (`cause`). When the
/// environment cannot be reached to undo it, the revision stays
/// `deploying`, with the error noted, so a later recovery pass retries.
/// Returns the message to report and whether the stack was restored.
pub async fn restore_and_settle(
    s: &AppState,
    ops: &dyn DockerOps,
    stack: &StackRecord,
    number: i64,
    cause: &str,
) -> (String, bool) {
    match restore(ops, &stack.name, number).await {
        Ok(_) => {
            if let Err(e) = s.store.finish_revision(stack.id, number, Some(cause)).await {
                tracing::warn!(error = %e, stack = %stack.name, "recording a deploy failed");
            }
            (cause.to_string(), true)
        }
        Err(e) => {
            let msg = format!("{cause}; restore: {e}; retrying when the environment is reachable");
            if let Err(e) = s.store.note_revision_error(stack.id, number, &msg).await {
                tracing::warn!(error = %e, stack = %stack.name, "recording a deploy failed");
            }
            (msg, false)
        }
    }
}

/// Restores `stack` if its latest revision is still `deploying` and nothing
/// else holds the stack.
pub async fn recover_stack(s: &AppState, ops: &dyn DockerOps, stack: &StackRecord) -> Result<()> {
    let Some(_lock) = try_lock(s, stack.id) else {
        return Ok(());
    };
    let Some(latest) = s.store.list_revisions(stack.id).await?.into_iter().next() else {
        return Ok(());
    };
    if latest.status != "deploying" {
        return Ok(());
    }
    let (_, restored) = restore_and_settle(s, ops, stack, latest.number, INTERRUPTED).await;
    if restored {
        tracing::info!(stack = %stack.name, revision = latest.number, "recovered an interrupted deploy");
    }
    Ok(())
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
