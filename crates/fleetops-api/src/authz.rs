//! The single authorization decision point (spec §2.2). Global actions
//! need the admin role; environment actions need a grant from the store,
//! read fresh on every request.

use crate::{auth::extract::CurrentUser, AppState};
use fleetops_core::access::{EnvRole, Grant};
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result, Role, User};
use fleetops_store::NewAudit;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// See an environment and its status.
    EnvRead,
    /// Create, change or delete environments.
    EnvManage,
    DockerRead,
    DockerWrite,
    KubeRead,
    KubeWrite,
}

impl Action {
    fn needs(self) -> Option<EnvRole> {
        match self {
            Action::EnvRead | Action::DockerRead | Action::KubeRead => Some(EnvRole::Viewer),
            Action::DockerWrite | Action::KubeWrite => Some(EnvRole::Operator),
            Action::EnvManage => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Action::EnvRead => "env.read",
            Action::EnvManage => "env.manage",
            Action::DockerRead => "docker.read",
            Action::DockerWrite => "docker.write",
            Action::KubeRead => "k8s.read",
            Action::KubeWrite => "k8s.write",
        }
    }
}

pub fn authorize_global(user: &User, action: Action) -> Result<()> {
    match action {
        Action::EnvManage if user.role != Role::Admin => {
            Err(Error::Forbidden("admin role required".into()))
        }
        _ => Ok(()),
    }
}

/// No grant reads exactly like a missing environment.
pub fn authorize_env(grant: Option<&Grant>, env_name: &str, action: Action) -> Result<()> {
    let g = grant.ok_or_else(|| Error::NotFound(format!("environment '{env_name}'")))?;
    match action.needs() {
        Some(role) => g.require(role),
        None => Err(Error::Forbidden("admin role required".into())),
    }
}

/// Resolves `name`, checks `action` against the caller's grant, and
/// returns both. A refusal on a visible environment is audited.
pub async fn env_access(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    action: Action,
) -> Result<(Arc<dyn Environment>, Grant)> {
    let env = s.envs.get(name)?;
    let grant = s.store.env_grant(cu.user.id, env.id()).await?;
    let r = authorize_env(grant.as_ref(), name, action);
    deny_audited(s, cu, env.id(), action.as_str(), r).await?;
    Ok((env, grant.expect("authorized")))
}

/// Passes `r` through; a `Forbidden` is first written to the audit log.
pub async fn deny_audited<T>(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    what: &str,
    r: Result<T>,
) -> Result<T> {
    if let Err(e @ Error::Forbidden(_)) = &r {
        let id = env_id.to_string();
        let entry = NewAudit {
            ts: now_unix(),
            user_id: Some(cu.user.id),
            environment_id: Some(env_id),
            action: "access.denied",
            target_type: Some("env"),
            target_id: Some(&id),
            result: "denied",
            detail: Some(serde_json::json!({ "action": what, "error": e.to_string() })),
        };
        if let Err(e) = s.store.insert_audit(&entry).await {
            tracing::error!(error = %e, "failed to write audit log");
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: Role) -> User {
        User {
            id: 1,
            username: "u".into(),
            role,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn only_admins_manage_environments() {
        assert!(authorize_global(&user(Role::Admin), Action::EnvManage).is_ok());
        assert!(matches!(
            authorize_global(&user(Role::User), Action::EnvManage),
            Err(Error::Forbidden(_))
        ));
        assert!(authorize_global(&user(Role::User), Action::EnvRead).is_ok());
    }

    #[test]
    fn environment_actions_follow_the_grant() {
        let v = Grant::full(EnvRole::Viewer);
        let o = Grant::full(EnvRole::Operator);
        for a in [Action::EnvRead, Action::DockerRead, Action::KubeRead] {
            assert!(authorize_env(Some(&v), "e", a).is_ok());
        }
        for a in [Action::DockerWrite, Action::KubeWrite] {
            assert!(matches!(
                authorize_env(Some(&v), "e", a),
                Err(Error::Forbidden(_))
            ));
            assert!(authorize_env(Some(&o), "e", a).is_ok());
        }
        assert!(
            matches!(authorize_env(None, "e", Action::DockerRead), Err(Error::NotFound(m)) if m == "environment 'e'")
        );
    }
}

/// Fails with `license_required` unless the license grants `feature`.
/// Enterprise handlers call this before doing anything.
pub fn require_feature(state: &AppState, feature: fleetops_core::edition::Feature) -> Result<()> {
    if state.entitlements.has(feature) {
        Ok(())
    } else {
        Err(Error::LicenseRequired(feature.as_str().into()))
    }
}
