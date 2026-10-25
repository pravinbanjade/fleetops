//! The single authorization decision point (spec §2.2). Global actions
//! need the admin role; environment actions need a grant from the store,
//! read fresh on every request.

use crate::{auth::extract::CurrentUser, AppState};
use fleetops_core::access::{perm_denied, Grants};
use fleetops_core::env::Environment;
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result, Role, User};
use fleetops_store::NewAudit;
use std::collections::HashMap;
use std::sync::Arc;

/// What an environment request needs (custom roles spec, "The grid").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Any grant: see the environment, its status and engine info.
    AnyGrant,
    /// One permission, held by some grant. Where it applies (namespaces)
    /// is checked by the handler against [`Grants`].
    Perm(Permission),
}

impl Need {
    pub const fn of(row: Row, verb: Verb) -> Need {
        Need::Perm(Permission::new(row, verb))
    }

    fn label(self) -> String {
        match self {
            Need::AnyGrant => "env.read".into(),
            Need::Perm(p) => p.to_string(),
        }
    }
}

/// Global actions (manage environments) need the admin role.
pub fn require_admin(user: &User) -> Result<()> {
    if user.role == Role::Admin {
        Ok(())
    } else {
        Err(Error::Forbidden("admin role required".into()))
    }
}

/// No grant reads exactly like a missing environment.
pub fn authorize_env(grants: &Grants, env_name: &str, need: Need) -> Result<()> {
    if grants.is_empty() {
        return Err(Error::NotFound(format!("environment '{env_name}'")));
    }
    match need {
        Need::AnyGrant => Ok(()),
        Need::Perm(p) if grants.holds(p) => Ok(()),
        Need::Perm(p) => Err(perm_denied(p)),
    }
}

/// The user's grants on one environment: their direct grant plus any extra
/// grants (Enterprise teams and custom roles), kept apart. Every access
/// lookup goes through here or [`effective_grants`] (scripts/check-editions.sh).
pub async fn effective_grant(s: &AppState, user_id: i64, env_id: i64) -> Result<Grants> {
    let direct = s.store.env_grant(user_id, env_id).await?;
    let extra = s.extra_grants.env_grants(user_id, env_id).await?;
    Ok(Grants(direct.into_iter().chain(extra).collect()))
}

/// Every environment the user can see, with their grants on it.
pub async fn effective_grants(s: &AppState, user_id: i64) -> Result<HashMap<i64, Grants>> {
    let mut all: HashMap<i64, Grants> = s
        .store
        .user_grants(user_id)
        .await?
        .into_iter()
        .map(|(id, g)| (id, Grants(vec![g])))
        .collect();
    for (env_id, extra) in s.extra_grants.user_grants(user_id).await? {
        all.entry(env_id).or_default().0.extend(extra);
    }
    all.retain(|_, g| !g.is_empty());
    Ok(all)
}

/// Resolves `name`, checks `need` against the caller's grants, and
/// returns both. A refusal on a visible environment is audited.
pub async fn env_access(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    need: Need,
) -> Result<(Arc<dyn Environment>, Grants)> {
    let env = s.envs.get(name)?;
    let grants = effective_grant(s, cu.user.id, env.id()).await?;
    let r = authorize_env(&grants, name, need);
    deny_audited(s, cu, env.id(), &need.label(), r).await?;
    Ok((env, grants))
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
            detail: Some(serde_json::json!({ "permission": what, "error": e.to_string() })),
        };
        if let Err(e) = s.store.insert_audit(&entry).await {
            tracing::error!(error = %e, "failed to write audit log");
        }
    }
    r
}

/// [`deny_audited`] for an object the caller may not even see: the refusal
/// is audited, then reads as `missing`, so it cannot be told apart from an
/// object that does not exist (swarm spec §6).
pub async fn hidden_audited(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    what: &str,
    r: Result<()>,
    missing: Error,
) -> Result<()> {
    deny_audited(s, cu, env_id, what, r)
        .await
        .map_err(|e| match e {
            Error::Forbidden(_) => missing,
            e => e,
        })
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
            sso: None,
        }
    }

    #[test]
    fn only_admins_manage_environments() {
        assert!(require_admin(&user(Role::Admin)).is_ok());
        assert!(matches!(
            require_admin(&user(Role::User)),
            Err(Error::Forbidden(_))
        ));
    }

    #[test]
    fn environment_needs_follow_the_grants() {
        use fleetops_core::access::{EnvRole, Grant};
        let v = Grants(vec![Grant::full(EnvRole::Viewer)]);
        let o = Grants(vec![Grant::full(EnvRole::Operator)]);
        let read = Need::of(Row::Containers, Verb::View);
        let del = Need::of(Row::Volumes, Verb::Delete);
        assert!(authorize_env(&v, "e", Need::AnyGrant).is_ok());
        assert!(authorize_env(&v, "e", read).is_ok());
        assert!(
            matches!(authorize_env(&v, "e", del), Err(Error::Forbidden(m)) if m == "volumes.delete permission required")
        );
        assert!(authorize_env(&o, "e", del).is_ok());
        assert!(
            matches!(authorize_env(&Grants::default(), "e", read), Err(Error::NotFound(m)) if m == "environment 'e'")
        );
    }
}
