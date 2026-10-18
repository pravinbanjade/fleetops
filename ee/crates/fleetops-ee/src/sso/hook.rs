//! Core's [`ExternalAuth`] hook, answered from the SSO tables.

use super::{
    db, ldap, password_login,
    provision::{self, Claims, Refusal},
    PasswordLogin,
};
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
        s: &AppState,
        username: &str,
        password: &str,
    ) -> Result<Option<(User, Option<i64>)>> {
        let Some(conn) = db::ldap_connection(self.store.pool()).await? else {
            return Ok(None);
        };
        let bind_password = db::secrets(&self.store, conn.id).await?.bind_password;
        let entry =
            match ldap::authenticate(&conn.settings, bind_password.as_deref(), username, password)
                .await
            {
                Ok(Some(entry)) => entry,
                Ok(None) => return Ok(None),
                // A directory outage looks like a wrong password to the client;
                // local accounts were already checked.
                Err(e) => {
                    tracing::warn!(connection = conn.id, error = %e, "LDAP sign-in failed");
                    provision::refused(s, &conn, Refusal::Invalid, &e.to_string()).await;
                    return Ok(None);
                }
            };
        let claims = Claims {
            subject: entry.subject,
            preferred_username: None,
            email: entry.email,
            login: Some(entry.username),
            groups: entry.groups,
        };
        Ok(match provision::login(s, &conn, claims).await? {
            Ok(user) => Some((user, Some(conn.session_hours))),
            Err(_) => None,
        })
    }

    async fn managed_users(&self) -> Result<HashMap<i64, String>> {
        Ok(db::managed_users(self.store.pool())
            .await?
            .into_iter()
            .collect())
    }
}
