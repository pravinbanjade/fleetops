//! Docker resource endpoints under `/envs/{env}/...`.

pub mod compose;
pub mod containers;
pub mod image_tasks;
pub mod images;
pub mod networks;
pub mod stack_files;
pub mod stacks;
pub mod stacks_git;
pub mod system;
pub mod volumes;

use crate::{auth::extract::CurrentUser, authz::Need, ApiResult, AppState};
use fleetops_core::access::Grants;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use utoipa::IntoParams;

/// Everything a Docker handler needs: the authorised user, the environment
/// and its Docker operations.
pub struct EnvCtx {
    pub user: CurrentUser,
    pub env: Arc<dyn Environment>,
    pub ops: Arc<dyn DockerOps>,
    /// The caller's grants, for checks beyond the one `ctx` made.
    pub grants: Grants,
}

pub async fn ctx(s: &AppState, user: CurrentUser, env_name: &str, need: Need) -> ApiResult<EnvCtx> {
    let (env, grants) = crate::authz::env_access(s, &user, env_name, need).await?;
    if env.kind().is_kube() || env.kind().is_swarm() {
        return Err(crate::routes::kube::wrong_kind(env_name, env.kind()).into());
    }
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{env_name}' is unreachable")))?;
    Ok(EnvCtx {
        user,
        env,
        ops,
        grants,
    })
}

/// `ctx` for FleetOps stacks, which are stored per environment: a Swarm
/// node's view shares its swarm's id, so stacks are refused there.
pub async fn stack_ctx(
    s: &AppState,
    user: CurrentUser,
    env_name: &str,
    need: Need,
) -> ApiResult<EnvCtx> {
    let c = ctx(s, user, env_name, need).await?;
    if c.env.kind() == fleetops_core::env::EnvKind::SwarmNode {
        return Err(
            Error::Conflict("stacks are not available on a single swarm node".into()).into(),
        );
    }
    Ok(c)
}

impl EnvCtx {
    /// Writes an audit row for a Docker mutation, recording failures with
    /// the error message. Audit failures are logged, never surfaced.
    pub async fn audit<T>(
        &self,
        s: &AppState,
        action: &str,
        target_type: &str,
        target_id: &str,
        outcome: &Result<T>,
    ) {
        let (result, mut detail) = match outcome {
            Ok(_) => ("success", None),
            Err(e) => ("failure", Some(json!({ "error": e.to_string() }))),
        };
        // A Swarm node's view: say which node.
        if let Some((_, node)) = fleetops_core::env::split_node_view(self.env.name()) {
            detail
                .get_or_insert_with(|| json!({}))
                .as_object_mut()
                .map(|o| o.insert("node".into(), json!(node)));
        }
        let entry = NewAudit {
            ts: now_unix(),
            user_id: Some(self.user.user.id),
            environment_id: Some(self.env.id()),
            action,
            target_type: Some(target_type),
            target_id: Some(target_id),
            result,
            detail,
        };
        if let Err(e) = s.store.insert_audit(&entry).await {
            tracing::error!(error = %e, action, "failed to write audit log");
        }
    }
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ForceQuery {
    /// Remove even if running or in use.
    #[serde(default)]
    pub force: bool,
    /// Containers only: also remove anonymous volumes.
    #[serde(default)]
    pub volumes: bool,
}
