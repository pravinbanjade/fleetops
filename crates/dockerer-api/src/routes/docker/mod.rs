//! Docker resource endpoints under `/envs/{env}/...`.

pub mod compose;
pub mod containers;
pub mod images;
pub mod networks;
pub mod system;
pub mod volumes;

use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    ApiResult, AppState,
};
use dockerer_core::docker::DockerOps;
use dockerer_core::env::Environment;
use dockerer_core::{now_unix, Error, Result};
use dockerer_store::NewAudit;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

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
    let env = s.envs.get(env_name)?;
    authorize(&user.user, Some(env.id()), action)?;
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

#[derive(Deserialize, Default)]
pub struct ForceQuery {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub volumes: bool,
}
