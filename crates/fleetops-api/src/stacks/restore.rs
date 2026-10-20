//! Puts a stack back as it was before a deploy (stacks spec §4, "on
//! failure"), from labels and names alone, so it works after a restart:
//! containers labelled with the deploy's revision are removed, the set-aside
//! `<name>-fleetops-old-<revision>-<run|stop>` containers get their names
//! back (and are started if they were running), and networks the deploy
//! created are removed. Volumes and pulled images stay.

use super::{is_old, stack_containers};
use fleetops_core::compose::docker::{LABEL_PROJECT, LABEL_REVISION};
use fleetops_core::compose::plan::OLD_MARKER;
use fleetops_core::docker::{ContainerAction, DockerOps};
use fleetops_core::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Containers this deploy created, now removed.
    pub removed: Vec<String>,
    /// Set-aside containers given their names back.
    pub restored: Vec<String>,
}

/// The suffix a deploy of `revision` gives a container it sets aside.
pub fn old_suffix(revision: i64, running: bool) -> String {
    format!(
        "{OLD_MARKER}{revision}-{}",
        if running { "run" } else { "stop" }
    )
}

/// Undoes the deploy of `revision`. Keeps going past errors; if any, the
/// error lists them and every container left set aside.
pub async fn restore(ops: &dyn DockerOps, stack: &str, revision: i64) -> Result<RestoreReport> {
    let mut report = RestoreReport::default();
    let mut errors = Vec::new();
    let rev = revision.to_string();
    let all = ops.list_containers().await?;
    for c in stack_containers(&all, stack).filter(|c| !is_old(c)) {
        if c.labels.get(LABEL_REVISION) == Some(&rev) {
            match ops.remove_container(&c.id, true, false).await {
                Ok(()) => report.removed.push(c.name.clone()),
                Err(e) => errors.push(format!("remove {}: {e}", c.name)),
            }
        }
    }
    let mut leftovers = Vec::new();
    for c in stack_containers(&all, stack).filter(|c| is_old(c)) {
        let (base, running) = if let Some(b) = c.name.strip_suffix(&old_suffix(revision, true)) {
            (b, true)
        } else if let Some(b) = c.name.strip_suffix(&old_suffix(revision, false)) {
            (b, false)
        } else {
            continue;
        };
        if let Err(e) = ops.rename_container(&c.id, base).await {
            errors.push(format!("rename {} to {base}: {e}", c.name));
            leftovers.push(c.name.clone());
            continue;
        }
        if running {
            if let Err(e) = ops.container_action(&c.id, ContainerAction::Start).await {
                errors.push(format!("start {base}: {e}"));
            }
        }
        report.restored.push(base.to_string());
    }
    for n in ops.list_networks().await.unwrap_or_default() {
        if n.labels.get(LABEL_PROJECT).map(String::as_str) == Some(stack)
            && n.labels.get(LABEL_REVISION) == Some(&rev)
        {
            if let Err(e) = ops.remove_network(&n.id).await {
                errors.push(format!("remove network {}: {e}", n.name));
            }
        }
    }
    if errors.is_empty() {
        Ok(report)
    } else {
        let mut msg = errors.join("; ");
        if !leftovers.is_empty() {
            msg.push_str(&format!("; left set aside: {}", leftovers.join(", ")));
        }
        Err(Error::Internal(msg))
    }
}
