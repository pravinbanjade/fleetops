//! Sign-in from outside the local user table (Enterprise single sign-on).
//! Core asks this hook which providers the login page shows, whether a
//! user may log in with a local password, and which users an identity
//! provider manages.

use crate::AppState;
use async_trait::async_trait;
use fleetops_core::{external::LoginProvider, Result, User};
use std::collections::HashMap;

#[async_trait]
pub trait ExternalAuth: Send + Sync {
    /// Enabled redirect providers, for the login page.
    async fn providers(&self) -> Result<Vec<LoginProvider>>;
    /// Whether `user` may log in with a local password: false for users an
    /// identity provider manages, and for non-admins when password login is
    /// limited to local admins.
    async fn password_allowed(&self, user: &User) -> Result<bool>;
    /// Directory login for a username whose local check failed or that has
    /// no local account: the user and their session lifetime in hours.
    async fn directory_login(
        &self,
        s: &AppState,
        username: &str,
        password: &str,
    ) -> Result<Option<(User, Option<i64>)>>;
    /// The connection name for every user an identity provider manages.
    async fn managed_users(&self) -> Result<HashMap<i64, String>>;
}

/// The Community edition: local passwords only.
pub struct NoExternalAuth;

#[async_trait]
impl ExternalAuth for NoExternalAuth {
    async fn providers(&self) -> Result<Vec<LoginProvider>> {
        Ok(vec![])
    }
    async fn password_allowed(&self, _: &User) -> Result<bool> {
        Ok(true)
    }
    async fn directory_login(
        &self,
        _: &AppState,
        _: &str,
        _: &str,
    ) -> Result<Option<(User, Option<i64>)>> {
        Ok(None)
    }
    async fn managed_users(&self) -> Result<HashMap<i64, String>> {
        Ok(HashMap::new())
    }
}
