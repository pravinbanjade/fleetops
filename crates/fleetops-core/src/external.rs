//! Sign-in from outside the local user table (Enterprise single sign-on).
//! The hook itself is `fleetops_api::external::ExternalAuth`.

use serde::Serialize;
use utoipa::ToSchema;

/// A "Sign in with …" button on the login page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct LoginProvider {
    pub id: i64,
    pub name: String,
    pub kind: String,
}
