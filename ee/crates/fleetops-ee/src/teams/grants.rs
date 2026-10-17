//! Team grants and custom roles granted straight to users, as extra grants
//! core `authz` checks beside direct built-in ones.

use async_trait::async_trait;
use fleetops_core::access::{ExtraGrants, Grant};
use fleetops_core::Result;
use sqlx::SqlitePool;
use std::collections::HashMap;

pub struct TeamGrants {
    pool: SqlitePool,
}

impl TeamGrants {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ExtraGrants for TeamGrants {
    async fn env_grants(&self, user_id: i64, env_id: i64) -> Result<Vec<Grant>> {
        let mut out: Vec<Grant> = super::db::member_grants(&self.pool, user_id, Some(env_id))
            .await?
            .into_iter()
            .map(|(_, g)| g)
            .collect();
        out.extend(crate::roles::db::direct(&self.pool, user_id, env_id).await?);
        Ok(out)
    }

    async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        let mut out: HashMap<i64, Vec<Grant>> = HashMap::new();
        let team = super::db::member_grants(&self.pool, user_id, None).await?;
        let direct = crate::roles::db::user_direct(&self.pool, user_id).await?;
        for (env_id, g) in team.into_iter().chain(direct) {
            out.entry(env_id).or_default().push(g);
        }
        Ok(out)
    }
}
