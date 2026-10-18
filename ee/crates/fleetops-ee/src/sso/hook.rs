//! Core's [`ExternalAuth`] hook, answered from the SSO tables.

use super::{db, password_login, PasswordLogin};
use async_trait::async_trait;
use fleetops_api::{external::ExternalAuth, AppState};
use fleetops_core::{external::LoginProvider, Result, Role, User};
use fleetops_store::Store;
use std::collections::HashMap;

pub struct EeSso {
    store: Store,
}

impl EeSso {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ExternalAuth for EeSso {
    async fn providers(&self) -> Result<Vec<LoginProvider>> {
        Ok(db::list(self.store.pool())
            .await?
            .into_iter()
            .filter(|c| c.enabled)
            .map(|c| LoginProvider {
                id: c.id,
                kind: c.settings.kind().into(),
                name: c.name,
            })
            .collect())
    }

    async fn password_allowed(&self, user: &User) -> Result<bool> {
        if db::is_managed(self.store.pool(), user.id).await? {
            return Ok(false);
        }
        Ok(match password_login(&self.store).await? {
            PasswordLogin::Everyone => true,
            PasswordLogin::LocalAdmins if user.role == Role::Admin => true,
            // Nobody may be locked out: with no local admin left to sign in,
            // the limit is not applied.
            PasswordLogin::LocalAdmins => {
                let fallback = !db::local_admin_exists(self.store.pool()).await?;
                if fallback {
                    tracing::warn!(
                        "password login is limited to local admins, but none can sign in; allowing everyone"
                    );
                }
                fallback
            }
        })
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
        Ok(db::managed_users(self.store.pool())
            .await?
            .into_iter()
            .collect())
    }
}
