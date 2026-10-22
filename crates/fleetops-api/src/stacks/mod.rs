//! Stack deploys (stacks spec §4): the runner that applies a plan through
//! `DockerOps`, the restore that undoes a failed or interrupted deploy, and
//! recovery of deploys a restart cut off.

pub mod build;
pub mod deploy;
pub mod git;
pub mod recover;
pub mod restore;
pub mod start;
pub mod webhook;

pub use deploy::{deploy_stream, execute, Timing};
pub use recover::{recover_all, recover_stack, restore_and_settle, run_recovery};
pub use restore::{restore, RestoreReport};
pub use start::{start_deploy, Actor, DeploySource};

use fleetops_core::compose::docker::{LABEL_PROJECT, LABEL_SERVICE};
use fleetops_core::compose::plan::OLD_MARKER;
use fleetops_core::docker::ContainerSummary;

/// The stack's own containers, set-aside ones included.
pub(crate) fn stack_containers<'a>(
    all: &'a [ContainerSummary],
    stack: &'a str,
) -> impl Iterator<Item = &'a ContainerSummary> + 'a {
    all.iter().filter(move |c| {
        c.labels.get(LABEL_PROJECT).map(String::as_str) == Some(stack)
            || c.compose_project.as_deref() == Some(stack)
    })
}

pub(crate) fn service_of(c: &ContainerSummary) -> Option<&str> {
    c.labels
        .get(LABEL_SERVICE)
        .map(String::as_str)
        .or(c.compose_service.as_deref())
}

pub(crate) fn is_old(c: &ContainerSummary) -> bool {
    c.name.contains(OLD_MARKER)
}

/// Holds a stack's deploy lock until dropped, so a deploy, a recovery, a
/// removal and an action never run on one stack at once.
pub struct StackLock {
    set: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<i64>>>,
    id: i64,
}

impl Drop for StackLock {
    fn drop(&mut self) {
        self.set.lock().unwrap().remove(&self.id);
    }
}

/// Takes `stack`'s lock, or `None` while something else holds it.
pub fn try_lock(s: &crate::AppState, stack: i64) -> Option<StackLock> {
    s.stack_locks
        .lock()
        .unwrap()
        .insert(stack)
        .then(|| StackLock {
            set: s.stack_locks.clone(),
            id: stack,
        })
}
