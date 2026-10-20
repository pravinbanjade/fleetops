//! Stack deploys (stacks spec §4): the runner that applies a plan through
//! `DockerOps`, the restore that undoes a failed or interrupted deploy, and
//! recovery of deploys a restart cut off.

pub mod deploy;
pub mod restore;

pub use deploy::{deploy_stream, execute, Timing};
pub use restore::{restore, RestoreReport};

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
