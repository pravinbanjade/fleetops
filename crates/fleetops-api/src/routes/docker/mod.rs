//! Docker resource endpoints under `/envs/{env}/...`.

pub mod compose;
pub mod containers;
pub mod image_tasks;
pub mod images;
pub mod networks;
pub mod system;
pub mod volumes;

use crate::{auth::extract::CurrentUser, authz::Action, ApiResult, AppState};
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
}

pub async fn ctx(
    s: &AppState,
    user: CurrentUser,
    env_name: &str,
    action: Action,
) -> ApiResult<EnvCtx> {
    let (env, _) = crate::authz::env_access(s, &user, env_name, action).await?;
    if env.kind().is_kube() {
        return Err(crate::routes::kube::wrong_kind(env_name, env.kind()).into());
    }
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{env_name}' is unreachable")))?;
    Ok(EnvCtx { user, env, ops })
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
        let (result, detail) = match outcome {
            Ok(_) => ("success", None),
            Err(e) => ("failure", Some(json!({ "error": e.to_string() }))),
        };
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
