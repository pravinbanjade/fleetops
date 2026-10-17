//! Custom roles granted straight to users: the core `CustomRoles` hook.

use super::db;
use async_trait::async_trait;
use fleetops_core::access::{CustomRoles, Grant};
use fleetops_core::edition::{Entitlements, Feature};
use fleetops_core::{Error, Result};
use sqlx::SqlitePool;
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct EeRoles {
    pool: SqlitePool,
    license: Arc<dyn Entitlements>,
}

impl EeRoles {
    pub fn new(pool: SqlitePool, license: Arc<dyn Entitlements>) -> Self {
        Self { pool, license }
    }
}

/// Whether `new` (same role) reaches no further than `old` (spec, "Licensing").
fn narrower(new: Option<&BTreeSet<String>>, old: Option<&BTreeSet<String>>) -> bool {
    match (new, old) {
        (_, None) => true,
        (Some(n), Some(o)) => n.is_subset(o),
        (None, Some(_)) => false,
    }
}

#[async_trait]
impl CustomRoles for EeRoles {
    async fn assign(
        &self,
        user_id: i64,
        env_id: i64,
        role_id: i64,
        namespaces: Option<&BTreeSet<String>>,
        now: i64,
    ) -> Result<Grant> {
        db::get(&self.pool, role_id).await?;
        if !self.license.has(Feature::Rbac) {
            let old = db::direct(&self.pool, user_id, env_id).await?;
            let same = old.as_ref().is_some_and(|g| {
                g.custom.as_ref().is_some_and(|c| c.id == role_id)
                    && narrower(namespaces, g.namespaces.as_ref())
            });
            if !same {
                return Err(Error::LicenseRequired(Feature::Rbac.as_str().into()));
            }
        }
        db::set_direct(&self.pool, user_id, env_id, role_id, namespaces, now).await
    }

    async fn clear(&self, user_id: i64, env_id: i64) -> Result<bool> {
        db::clear_direct(&self.pool, user_id, env_id).await
    }

    async fn user_rows(&self, user_id: i64) -> Result<Vec<(i64, Grant)>> {
        db::user_direct(&self.pool, user_id).await
    }

    async fn env_rows(&self, env_id: i64) -> Result<Vec<(i64, Grant)>> {
        db::env_direct(&self.pool, env_id).await
    }
}
