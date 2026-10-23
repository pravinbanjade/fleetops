//! Swarm environments: an `environments` row of kind `swarm-agent`, one
//! `agent_credentials` row holding the shared key (and the previous one
//! during a rotation), the pinned swarm, and per-node agent state (swarm
//! spec §2, §3).

use crate::{db_err, EnvironmentRecord, Store};
use fleetops_core::{env::EnvKind, Error, Result};
use serde::Serialize;
use utoipa::ToSchema;

/// Which of the environment's keys a bearer matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMatch {
    Current,
    /// The key before the last rotation, still inside its grace period.
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SwarmCluster {
    pub cluster_id: String,
    /// PEM of the root CA node certificates must chain to.
    pub trust_root: String,
    pub pinned_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct SwarmAgentRecord {
    pub node_id: String,
    pub hostname: Option<String>,
    pub manager: bool,
    pub agent_version: Option<String>,
    pub last_seen_at: Option<i64>,
    pub last_error: Option<String>,
}

impl Store {
    /// A swarm environment is enrolled from the start: its agents share
    /// `key_hash`, so there is no join token.
    pub async fn create_swarm_environment(
        &self,
        name: &str,
        key_hash: &str,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO environments (name, kind, url, created_at, updated_at)
             VALUES (?, ?, '', ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(EnvKind::SwarmAgent.as_str())
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
        sqlx::query("INSERT INTO agent_credentials (environment_id, key_hash) VALUES (?, ?)")
            .bind(id)
            .bind(key_hash)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.get_environment(id).await
    }

    /// The swarm environment `key_hash` opens, if any: its current key, or
    /// its previous one before `prev_key_expires_at`.
    pub async fn swarm_by_key(&self, key_hash: &str, now: i64) -> Result<Option<(i64, KeyMatch)>> {
        let row: Option<(i64, bool)> = sqlx::query_as(
            "SELECT c.environment_id, c.key_hash = ?1
             FROM agent_credentials c JOIN environments e ON e.id = c.environment_id
             WHERE e.kind = 'swarm-agent'
               AND (c.key_hash = ?1 OR (c.prev_key_hash = ?1 AND c.prev_key_expires_at > ?2))",
        )
        .bind(key_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|(id, current)| {
            (
                id,
                if current {
                    KeyMatch::Current
                } else {
                    KeyMatch::Previous
                },
            )
        }))
    }

    /// Makes `new_hash` the key and keeps the old one until
    /// `prev_expires_at`; returns the new key generation.
    pub async fn rotate_swarm_key(
        &self,
        env_id: i64,
        new_hash: &str,
        prev_expires_at: i64,
    ) -> Result<i64> {
        sqlx::query_scalar(
            "UPDATE agent_credentials
             SET prev_key_hash = key_hash, prev_key_expires_at = ?,
                 key_hash = ?, key_generation = key_generation + 1
             WHERE environment_id = ? RETURNING key_generation",
        )
        .bind(prev_expires_at)
        .bind(new_hash)
        .bind(env_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("agent environment {env_id}")))
    }

    /// Ends the grace period now; returns the previous key's hash, if one
    /// was still pending, so its sessions can be revoked.
    pub async fn finish_swarm_rotation(&self, env_id: i64) -> Result<Option<String>> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let prev: Option<Option<String>> = sqlx::query_scalar(
            "SELECT prev_key_hash FROM agent_credentials WHERE environment_id = ?",
        )
        .bind(env_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "UPDATE agent_credentials SET prev_key_hash = NULL, prev_key_expires_at = NULL
             WHERE environment_id = ?",
        )
        .bind(env_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(prev.flatten())
    }

    /// Pins the environment to a swarm unless one is pinned already;
    /// returns whichever is pinned.
    pub async fn pin_swarm_cluster(
        &self,
        env_id: i64,
        cluster_id: &str,
        trust_root: &str,
        now: i64,
    ) -> Result<SwarmCluster> {
        sqlx::query(
            "INSERT INTO swarm_clusters (environment_id, cluster_id, trust_root, pinned_at)
             VALUES (?, ?, ?, ?) ON CONFLICT (environment_id) DO NOTHING",
        )
        .bind(env_id)
        .bind(cluster_id)
        .bind(trust_root)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        self.get_swarm_cluster(env_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("swarm environment {env_id}")))
    }

    pub async fn get_swarm_cluster(&self, env_id: i64) -> Result<Option<SwarmCluster>> {
        sqlx::query_as(
            "SELECT cluster_id, trust_root, pinned_at FROM swarm_clusters WHERE environment_id = ?",
        )
        .bind(env_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn clear_swarm_cluster(&self, env_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM swarm_clusters WHERE environment_id = ?")
            .bind(env_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Upserts a node's agent state. `None` for hostname or version keeps
    /// the stored value; `last_error` is always written (None clears it).
    pub async fn record_swarm_agent(&self, env_id: i64, rec: &SwarmAgentRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO swarm_agents
               (environment_id, node_id, hostname, manager, agent_version, last_seen_at, last_error)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (environment_id, node_id) DO UPDATE SET
               hostname = COALESCE(excluded.hostname, hostname),
               manager = excluded.manager,
               agent_version = COALESCE(excluded.agent_version, agent_version),
               last_seen_at = COALESCE(excluded.last_seen_at, last_seen_at),
               last_error = excluded.last_error",
        )
        .bind(env_id)
        .bind(&rec.node_id)
        .bind(&rec.hostname)
        .bind(rec.manager)
        .bind(&rec.agent_version)
        .bind(rec.last_seen_at)
        .bind(&rec.last_error)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn list_swarm_agents(&self, env_id: i64) -> Result<Vec<SwarmAgentRecord>> {
        sqlx::query_as(
            "SELECT node_id, hostname, manager, agent_version, last_seen_at, last_error
             FROM swarm_agents WHERE environment_id = ? ORDER BY node_id",
        )
        .bind(env_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }
}
