//! "Recreate with changes": replace a container with one built from a new
//! spec, rolling back to the original if the replacement cannot start.

use crate::{
    docker::{ContainerAction, ContainerOps, ContainerSpec},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecreateOutcome {
    pub new_id: String,
    /// False when the replacement is running but the old container could
    /// not be deleted; it is left renamed as `backup_name`.
    pub old_removed: bool,
    pub backup_name: String,
}

/// Steps:
/// 1. stop the old container if it is running;
/// 2. rename it to `<name>-dockerer-old-<now>`;
/// 3. create and start the replacement under the target name;
/// 4. remove the old container.
///
/// If step 3 fails, the replacement is removed, the old container gets its
/// name back and is restarted if it was running, and the step-3 error is
/// returned.
pub async fn recreate(
    ops: &dyn ContainerOps,
    id: &str,
    spec: &ContainerSpec,
    now: i64,
) -> Result<RecreateOutcome> {
    spec.validate()?;
    let inspect = ops.inspect_container(id).await?;
    let old_name = inspect["Name"]
        .as_str()
        .map(|n| n.trim_start_matches('/').to_string())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Internal("container inspect has no Name".into()))?;
    let was_running = inspect["State"]["Running"].as_bool().unwrap_or(false);
    let backup_name = format!("{old_name}-dockerer-old-{now}");

    if was_running {
        ops.container_action(id, ContainerAction::Stop).await?;
    }
    if let Err(e) = ops.rename_container(id, &backup_name).await {
        if was_running {
            let _ = ops.container_action(id, ContainerAction::Start).await;
        }
        return Err(e);
    }

    let mut new_spec = spec.clone();
    new_spec.name = Some(spec.name.clone().unwrap_or_else(|| old_name.clone()));

    match create_and_start(ops, &new_spec).await {
        Ok(new_id) => {
            let old_removed = ops.remove_container(id, true, false).await.is_ok();
            Ok(RecreateOutcome {
                new_id,
                old_removed,
                backup_name,
            })
        }
        Err(e) => {
            let _ = ops.rename_container(id, &old_name).await;
            if was_running {
                let _ = ops.container_action(id, ContainerAction::Start).await;
            }
            Err(e)
        }
    }
}

async fn create_and_start(ops: &dyn ContainerOps, spec: &ContainerSpec) -> Result<String> {
    let new_id = ops.create_container(spec).await?;
    if let Err(e) = ops.container_action(&new_id, ContainerAction::Start).await {
        let _ = ops.remove_container(&new_id, true, false).await;
        return Err(e);
    }
    Ok(new_id)
}
