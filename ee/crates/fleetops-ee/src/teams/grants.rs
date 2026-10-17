//! Team grants as extra grants, merged with direct ones in core `authz`.

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
        Ok(super::db::member_grants(&self.pool, user_id, Some(env_id))
            .await?
            .into_iter()
            .map(|(_, g)| g)
            .collect())
    }

    async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        let mut out: HashMap<i64, Vec<Grant>> = HashMap::new();
        for (env_id, g) in super::db::member_grants(&self.pool, user_id, None).await? {
            out.entry(env_id).or_default().push(g);
        }
        Ok(out)
    }
}
