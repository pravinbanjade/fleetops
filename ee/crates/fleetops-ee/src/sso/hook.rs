//! Core's [`ExternalAuth`] hook, answered from the SSO tables.

use super::{db, password_login, PasswordLogin};
use async_trait::async_trait;
use fleetops_core::{
    external::{ExternalAuth, LoginProvider},
    Result, Role, User,
};
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
            PasswordLogin::LocalAdmins => user.role == Role::Admin,
        })
    }

    async fn directory_login(&self, _: &str, _: &str) -> Result<Option<(User, Option<i64>)>> {
        Ok(None)
    }

    async fn managed_users(&self) -> Result<HashMap<i64, String>> {
        Ok(db::managed_users(self.store.pool())
            .await?
            .into_iter()
            .collect())
    }
}
