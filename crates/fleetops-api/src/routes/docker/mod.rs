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
use fleetops_core::access::{swarm_wide_denied, Grants};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::perm::{Permission, Row};
use fleetops_core::swarm::STACK_LABEL;
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
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
    /// On a swarm node's view, the stacks a grant limited to stacks covers
    /// (swarm spec §6); `None` when the permission covers the whole swarm.
    pub stacks: Option<BTreeSet<String>>,
}

/// The stacks `need` reaches on a swarm node's view, when no grant covers
/// the whole swarm.
pub(crate) fn node_scope(grants: &Grants, need: Need) -> Option<BTreeSet<String>> {
    match need {
        Need::Perm(p) => grants.namespaces_for(p),
        Need::AnyGrant => {
            let mut all = BTreeSet::new();
            for g in &grants.0 {
                all.extend(g.namespaces.as_ref()?.iter().cloned());
            }
            Some(all)
        }
    }
}

pub async fn ctx(s: &AppState, user: CurrentUser, env_name: &str, need: Need) -> ApiResult<EnvCtx> {
    let (env, grants) = crate::authz::env_access(s, &user, env_name, need).await?;
    if env.kind().is_kube() || env.kind().is_swarm() {
        return Err(crate::routes::kube::wrong_kind(env_name, env.kind()).into());
    }
    let stacks = if env.kind() == fleetops_core::env::EnvKind::SwarmNode {
        node_scope(&grants, need)
    } else {
        None
    };
    // A grant limited to stacks reaches a node's containers only.
    if let (Some(_), Need::Perm(p)) = (&stacks, need) {
        if p.row != Row::Containers {
            crate::authz::deny_audited(
                s,
                &user,
                env.id(),
                &p.to_string(),
                Err::<(), _>(swarm_wide_denied()),
            )
            .await?;
        }
    }
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{env_name}' is unreachable")))?;
    Ok(EnvCtx {
        user,
        env,
        ops,
        grants,
        stacks,
    })
}

/// A container's stack, from its labels.
pub(crate) fn stack_of(labels: &std::collections::BTreeMap<String, String>) -> Option<&str> {
    labels.get(STACK_LABEL).map(String::as_str)
}

/// `p` on container `id` of a node's view, by its stack: a grant limited to
/// stacks reaches its stacks' containers only. Audited when refused.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn check_container(
    s: &AppState,
    user: &CurrentUser,
    env: &Arc<dyn Environment>,
    ops: &Arc<dyn DockerOps>,
    grants: &Grants,
    stacks: &Option<BTreeSet<String>>,
    p: Permission,
    id: &str,
) -> Result<()> {
    if stacks.is_none() {
        return Ok(());
    }
    // Another stack's container reads exactly as a missing one.
    let missing = || Error::NotFound(format!("no such container '{id}'"));
    let inspect = ops.inspect_container(id).await.map_err(|_| missing())?;
    let stack = inspect["Config"]["Labels"][STACK_LABEL].as_str();
    let view = Permission::new(Row::Containers, fleetops_core::perm::Verb::View);
    crate::authz::hidden_audited(
        s,
        user,
        env.id(),
        &view.to_string(),
        grants.check_stack(view, stack),
        missing(),
    )
    .await?;
    crate::authz::deny_audited(
        s,
        user,
        env.id(),
        &p.to_string(),
        grants.check_stack(p, stack),
    )
    .await
}

impl EnvCtx {
    /// See [`check_container`].
    pub async fn check_container(&self, s: &AppState, p: Permission, id: &str) -> Result<()> {
        check_container(
            s,
            &self.user,
            &self.env,
            &self.ops,
            &self.grants,
            &self.stacks,
            p,
            id,
        )
        .await
    }

    /// Refuses a grant limited to stacks: creating and pruning reach the
    /// whole node.
    pub async fn whole(&self, s: &AppState, p: Permission) -> Result<()> {
        if self.stacks.is_none() {
            return Ok(());
        }
        crate::authz::deny_audited(
            s,
            &self.user,
            self.env.id(),
            &p.to_string(),
            Err(swarm_wide_denied()),
        )
        .await
    }

    /// Whether a container with `labels` is in the caller's stacks.
    pub fn sees(&self, labels: &std::collections::BTreeMap<String, String>) -> bool {
        match &self.stacks {
            None => true,
            Some(set) => stack_of(labels).is_some_and(|st| set.contains(st)),
        }
    }
}

/// Everything a stack handler needs: the caller, the environment, what its
/// stacks deploy through, and the caller's grants.
pub struct StackCtx {
    pub user: CurrentUser,
    pub env: Arc<dyn Environment>,
    pub ops: crate::stacks::StackOps,
    pub grants: Grants,
    /// On a swarm, the stacks the caller may deploy (`None`: every stack).
    pub scope: Option<std::collections::BTreeSet<String>>,
}

/// The context for FleetOps stacks, which live on a Docker host or a swarm.
/// A Swarm node's view shares its swarm's id, so stacks are refused there.
/// On a swarm, a grant may be limited to stacks (swarm spec §6): `stack`
/// names the one the request is about, and `None` (lists, validation) only
/// needs the permission held somewhere.
pub async fn stack_ctx(
    s: &AppState,
    user: CurrentUser,
    env_name: &str,
    need: Need,
    stack: Option<&str>,
) -> ApiResult<StackCtx> {
    let (env, grants) = crate::authz::env_access(s, &user, env_name, need).await?;
    let kind = env.kind();
    if kind == fleetops_core::env::EnvKind::SwarmNode {
        return Err(
            Error::Conflict("stacks are not available on a single swarm node".into()).into(),
        );
    }
    if kind.is_kube() {
        return Err(crate::routes::kube::wrong_kind(env_name, kind).into());
    }
    let mut scope = None;
    if kind.is_swarm() {
        if let (Need::Perm(p), Some(stack)) = (need, stack) {
            crate::authz::deny_audited(
                s,
                &user,
                env.id(),
                &p.to_string(),
                grants.check_stack(p, Some(stack)),
            )
            .await?;
        }
        scope = grants.namespaces_for(fleetops_core::perm::Permission::new(
            fleetops_core::perm::Row::Stacks,
            fleetops_core::perm::Verb::Operate,
        ));
    }
    let ops = crate::stacks::StackOps::of(&env).ok_or_else(|| {
        Error::EnvUnavailable(if kind.is_swarm() {
            "no manager agent is connected".to_string()
        } else {
            format!("environment '{env_name}' is unreachable")
        })
    })?;
    Ok(StackCtx {
        user,
        env,
        ops,
        grants,
        scope,
    })
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
