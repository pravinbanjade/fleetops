//! Single sign-on (SSO spec): identity provider connections, the users they
//! create, and group sync to teams and admin. Existing SSO users keep
//! signing in without a license; adding or widening anything needs one.

pub mod db;
pub mod flow;
pub mod github;
pub mod google;
pub mod hook;
pub mod microsoft;
pub mod model;
pub mod oidc;
pub mod provision;
pub mod routes;

use fleetops_core::Result;
use fleetops_store::Store;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The password hash of every SSO user: never a valid argon2 hash, so no
/// password verifies against it.
pub const NO_PASSWORD: &str = "!sso";

/// The settings key for [`PasswordLogin`].
pub const PASSWORD_LOGIN_KEY: &str = "sso.password_login";

/// Who may log in with a local password.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PasswordLogin {
    #[default]
    Everyone,
    /// Only local accounts with the admin role (the break-glass path).
    LocalAdmins,
}

pub async fn password_login(store: &Store) -> Result<PasswordLogin> {
    Ok(store
        .get_setting(PASSWORD_LOGIN_KEY)
        .await?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}
