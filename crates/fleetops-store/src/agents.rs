//! Agent environments: an `environments` row of kind `docker-agent` plus
//! one `agent_credentials` row (sub-project 2 spec §3).

use crate::{db_err, EnvironmentRecord, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct AgentRecord {
    pub environment_id: i64,
    /// An agent key is set: the agent has enrolled and not been rotated since.
    pub enrolled: bool,
    pub join_expires_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub agent_version: Option<String>,
    /// JSON from the agent's hello.
    pub host_info: Option<String>,
    /// Why the last connection was refused, when it was.
    pub last_error: Option<String>,
}

const AGENT_COLS: &str = "environment_id, key_hash IS NOT NULL AS enrolled, join_expires_at, \
                          last_seen_at, agent_version, host_info, last_error";

impl Store {
    pub async fn create_agent_environment(
        &self,
        name: &str,
        kind: fleetops_core::env::EnvKind,
        join_hash: &str,
        join_expires_at: i64,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO environments (name, kind, url, created_at, updated_at)
             VALUES (?, ?, '', ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(kind.as_str())
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
            other => other,
        })?;
        sqlx::query("UPDATE environments SET url = ? WHERE id = ?")
            .bind(fleetops_core::env::agent_url(id))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO agent_credentials (environment_id, join_token_hash, join_expires_at)
             VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(join_hash)
        .bind(join_expires_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.get_environment(id).await
    }

    pub async fn agent_by_join_token(&self, join_hash: &str, now: i64) -> Result<Option<i64>> {
        sqlx::query_scalar(
            "SELECT environment_id FROM agent_credentials
             WHERE join_token_hash = ? AND join_expires_at > ?",
        )
        .bind(join_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn agent_by_key(&self, key_hash: &str) -> Result<Option<i64>> {
        sqlx::query_scalar("SELECT environment_id FROM agent_credentials WHERE key_hash = ?")
            .bind(key_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    /// Swaps a live join token for a key in one statement, so of two
    /// agents racing on the same token exactly one wins.
    pub async fn enroll_agent(
        &self,
        env_id: i64,
        join_hash: &str,
        key_hash: &str,
        now: i64,
    ) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE agent_credentials
             SET key_hash = ?, join_token_hash = NULL, join_expires_at = NULL, last_error = NULL
             WHERE environment_id = ? AND join_token_hash = ? AND join_expires_at > ?",
        )
        .bind(key_hash)
        .bind(env_id)
        .bind(join_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn rotate_agent(
        &self,
        env_id: i64,
        join_hash: &str,
        join_expires_at: i64,
    ) -> Result<()> {
        let done = sqlx::query(
            "UPDATE agent_credentials
             SET key_hash = NULL, join_token_hash = ?, join_expires_at = ?
             WHERE environment_id = ?",
        )
        .bind(join_hash)
        .bind(join_expires_at)
        .bind(env_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound(format!("agent environment {env_id}")));
        }
        Ok(())
    }

    /// `None` for version or host keeps the stored value; `last_error` is
    /// always written (None clears it).
    pub async fn record_agent_seen(
        &self,
        env_id: i64,
        now: i64,
        agent_version: Option<&str>,
        host_info: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE agent_credentials
             SET last_seen_at = ?, agent_version = COALESCE(?, agent_version),
                 host_info = COALESCE(?, host_info), last_error = ?
             WHERE environment_id = ?",
        )
        .bind(now)
        .bind(agent_version)
        .bind(host_info)
        .bind(last_error)
        .bind(env_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn get_agent(&self, env_id: i64) -> Result<Option<AgentRecord>> {
        sqlx::query_as(&format!(
            "SELECT {AGENT_COLS} FROM agent_credentials WHERE environment_id = ?"
        ))
        .bind(env_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        sqlx::query_as(&format!(
            "SELECT {AGENT_COLS} FROM agent_credentials ORDER BY environment_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }
}
