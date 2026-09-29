//! The single authorization decision point. Sub-project 5 replaces the body
//! with per-environment role bindings; handlers do not change.

use fleetops_core::{Error, Result, Role, User};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// List environments and read their status.
    EnvRead,
    /// Create, change or delete environments.
    EnvManage,
    /// Read Docker resources in an environment.
    DockerRead,
    /// Change Docker resources in an environment.
    DockerWrite,
}

pub fn authorize(user: &User, _env_id: Option<i64>, action: Action) -> Result<()> {
    match (user.role, action) {
        (Role::Admin, _) => Ok(()),
        (Role::User, Action::EnvManage) => Err(Error::Forbidden("admin role required".into())),
        (Role::User, _) => Ok(()),
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
        }
    }

    #[test]
    fn admins_can_do_everything_users_cannot_manage_envs() {
        for a in [
            Action::EnvRead,
            Action::EnvManage,
            Action::DockerRead,
            Action::DockerWrite,
        ] {
            assert!(authorize(&user(Role::Admin), Some(1), a).is_ok());
        }
        assert!(authorize(&user(Role::User), Some(1), Action::DockerWrite).is_ok());
        assert!(authorize(&user(Role::User), None, Action::EnvRead).is_ok());
        assert!(matches!(
            authorize(&user(Role::User), None, Action::EnvManage),
            Err(Error::Forbidden(_))
        ));
    }
}
