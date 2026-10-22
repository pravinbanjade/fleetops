//! Git sources of stacks (stacks spec §8). The token and webhook secret are
//! sealed with the server key; callers see plaintext.

use crate::stacks::{StackRecord, STACK_COLS};
use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use sqlx::AssertSqlSafe;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct GitRecord {
    pub stack_id: i64,
    pub url: String,
    pub branch: String,
    pub path: String,
    /// manual | poll | webhook
    pub sync: String,
    pub poll_seconds: i64,
    pub auto_deploy: bool,
    /// Auto-deploy is held after a rollback until resumed.
    pub paused: bool,
    /// Whether an access token is stored (it is never returned).
    pub has_token: bool,
    /// Whether an SSH key is stored (the private key is never returned).
    pub has_key: bool,
    /// The stored SSH key's public line, to add to the git host.
    pub ssh_public_key: Option<String>,
    /// The SSH host key pinned for the stack (`<algorithm> SHA256:…`).
    pub ssh_host_key: Option<String>,
    pub last_fetched_commit: Option<String>,
    /// A fetched commit whose Compose file is not deployed yet.
    pub pending_commit: Option<String>,
    pub last_sync_at: Option<i64>,
    pub last_sync_error: Option<String>,
    pub updated_at: i64,
}

/// The settings a user edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSettings {
    pub url: String,
    pub branch: String,
    pub path: String,
    pub sync: String,
    pub poll_seconds: i64,
    pub auto_deploy: bool,
}

/// What saving settings does to the stored SSH key.
#[derive(Debug, Clone, Copy)]
pub enum KeyChange<'a> {
    Keep,
    Clear,
    Set { private: &'a str, public: &'a str },
}

/// What saving settings does to the stored token.
#[derive(Debug, Clone, Copy)]
pub enum TokenChange<'a> {
    Keep,
    Clear,
    Set(&'a str),
}

const GIT_COLS: &str = "stack_id, url, branch, path, sync, poll_seconds, auto_deploy, paused,
    token IS NOT NULL AS has_token, ssh_key IS NOT NULL AS has_key, ssh_public_key, ssh_host_key,
    last_fetched_commit, pending_commit, last_sync_at,
    last_sync_error, updated_at";

fn token_aad(stack_id: i64) -> Vec<u8> {
    format!("stack-git-token:{stack_id}").into_bytes()
}

fn key_aad(stack_id: i64) -> Vec<u8> {
    format!("stack-git-key:{stack_id}").into_bytes()
}

fn secret_aad(stack_id: i64) -> Vec<u8> {
    format!("stack-git-hook:{stack_id}").into_bytes()
}

impl Store {
    fn open_text(&self, aad: &[u8], sealed: &str, what: &str) -> Result<String> {
        let plain = self
            .secrets
            .open(aad, sealed)
            .map_err(|_| Error::Internal(format!("the stack's {what} cannot be decrypted")))?;
        String::from_utf8(plain)
            .map_err(|_| Error::Internal(format!("the stack's {what} is corrupt")))
    }

    /// [`Self::set_stack_git_with`], keeping the stored SSH key.
    pub async fn set_stack_git(
        &self,
        stack_id: i64,
        settings: &GitSettings,
        token: TokenChange<'_>,
        new_secret: Option<&str>,
        user: Option<i64>,
        now: i64,
    ) -> Result<GitRecord> {
        self.set_stack_git_with(
            stack_id,
            settings,
            token,
            KeyChange::Keep,
            new_secret,
            user,
            now,
        )
        .await
    }

    /// Attaches or updates the stack's git source and marks the stack `git`.
    /// A new source needs `new_secret`. Changing the URL, branch or path
    /// forgets what was fetched; changing the URL also forgets the pinned
    /// host key.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_stack_git_with(
        &self,
        stack_id: i64,
        settings: &GitSettings,
        token: TokenChange<'_>,
        key: KeyChange<'_>,
        new_secret: Option<&str>,
        user: Option<i64>,
        now: i64,
    ) -> Result<GitRecord> {
        // Immediate: this reads before it writes, and a sync writing in
        // between would otherwise fail the upgrade with "database is locked".
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db_err)?;
        let existing: Option<(String, String, String)> =
            sqlx::query_as("SELECT url, branch, path FROM stack_git WHERE stack_id = ?")
                .bind(stack_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        let sealed_token = |t: &str| self.secrets.seal(&token_aad(stack_id), t.as_bytes());
        match existing {
            None => {
                let secret = new_secret.ok_or_else(|| {
                    Error::Invalid("a new git source needs a webhook secret".into())
                })?;
                let token = match token {
                    TokenChange::Set(t) => Some(sealed_token(t)),
                    TokenChange::Keep | TokenChange::Clear => None,
                };
                sqlx::query(
                    "INSERT INTO stack_git (stack_id, url, branch, path, token, webhook_secret, sync,
                       poll_seconds, auto_deploy, updated_at, updated_by)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(stack_id)
                .bind(&settings.url)
                .bind(&settings.branch)
                .bind(&settings.path)
                .bind(token)
                .bind(self.secrets.seal(&secret_aad(stack_id), secret.as_bytes()))
                .bind(&settings.sync)
                .bind(settings.poll_seconds)
                .bind(settings.auto_deploy)
                .bind(now)
                .bind(user)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            Some((url, branch, path)) => {
                let moved =
                    url != settings.url || branch != settings.branch || path != settings.path;
                sqlx::query(
                    "UPDATE stack_git SET url = ?, branch = ?, path = ?, sync = ?, poll_seconds = ?,
                       auto_deploy = ?, updated_at = ?, updated_by = ? WHERE stack_id = ?",
                )
                .bind(&settings.url)
                .bind(&settings.branch)
                .bind(&settings.path)
                .bind(&settings.sync)
                .bind(settings.poll_seconds)
                .bind(settings.auto_deploy)
                .bind(now)
                .bind(user)
                .bind(stack_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
                if url != settings.url {
                    sqlx::query("UPDATE stack_git SET ssh_host_key = NULL WHERE stack_id = ?")
                        .bind(stack_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                }
                if moved {
                    sqlx::query(
                        "UPDATE stack_git SET last_fetched_commit = NULL, pending_commit = NULL,
                           last_sync_error = NULL WHERE stack_id = ?",
                    )
                    .bind(stack_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
                let token = match token {
                    TokenChange::Keep => None,
                    TokenChange::Clear => Some(None),
                    TokenChange::Set(t) => Some(Some(sealed_token(t))),
                };
                if let Some(t) = token {
                    sqlx::query("UPDATE stack_git SET token = ? WHERE stack_id = ?")
                        .bind(t)
                        .bind(stack_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                }
            }
        }
        let key = match key {
            KeyChange::Keep => None,
            KeyChange::Clear => Some((None, None)),
            KeyChange::Set { private, public } => Some((
                Some(self.secrets.seal(&key_aad(stack_id), private.as_bytes())),
                Some(public),
            )),
        };
        if let Some((sealed, public)) = key {
            sqlx::query("UPDATE stack_git SET ssh_key = ?, ssh_public_key = ? WHERE stack_id = ?")
                .bind(sealed)
                .bind(public)
                .bind(stack_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        sqlx::query("UPDATE stacks SET source = 'git' WHERE id = ?")
            .bind(stack_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.stack_git(stack_id)
            .await?
            .ok_or_else(|| Error::Internal("the git source was not saved".into()))
    }

    pub async fn stack_git(&self, stack_id: i64) -> Result<Option<GitRecord>> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {GIT_COLS} FROM stack_git WHERE stack_id = ?"
        )))
        .bind(stack_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn stack_git_token(&self, stack_id: i64) -> Result<Option<String>> {
        let sealed: Option<Option<String>> =
            sqlx::query_scalar("SELECT token FROM stack_git WHERE stack_id = ?")
                .bind(stack_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        sealed
            .flatten()
            .map(|s| self.open_text(&token_aad(stack_id), &s, "git token"))
            .transpose()
    }

    pub async fn stack_git_key(&self, stack_id: i64) -> Result<Option<String>> {
        let sealed: Option<Option<String>> =
            sqlx::query_scalar("SELECT ssh_key FROM stack_git WHERE stack_id = ?")
                .bind(stack_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        sealed
            .flatten()
            .map(|s| self.open_text(&key_aad(stack_id), &s, "SSH key"))
            .transpose()
    }

    /// Pins (or, with `None`, forgets) the stack's SSH host key.
    pub async fn set_git_host_key(&self, stack_id: i64, key: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE stack_git SET ssh_host_key = ? WHERE stack_id = ?")
            .bind(key)
            .bind(stack_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn stack_git_secret(&self, stack_id: i64) -> Result<Option<String>> {
        let sealed: Option<String> =
            sqlx::query_scalar("SELECT webhook_secret FROM stack_git WHERE stack_id = ?")
                .bind(stack_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        sealed
            .map(|s| self.open_text(&secret_aad(stack_id), &s, "webhook secret"))
            .transpose()
    }

    pub async fn rotate_stack_git_secret(&self, stack_id: i64, secret: &str) -> Result<()> {
        let res = sqlx::query("UPDATE stack_git SET webhook_secret = ? WHERE stack_id = ?")
            .bind(self.secrets.seal(&secret_aad(stack_id), secret.as_bytes()))
            .bind(stack_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound("the stack has no git source".into()));
        }
        Ok(())
    }

    /// Turns the stack back into an inline one; its draft stays.
    pub async fn detach_stack_git(&self, stack_id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM stack_git WHERE stack_id = ?")
            .bind(stack_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("UPDATE stacks SET source = 'inline' WHERE id = ?")
            .bind(stack_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        // Repo configs become Secrets-tab values, so the stack deploys the
        // same files as before.
        sqlx::query(
            "DELETE FROM stack_files WHERE stack_id = ? AND source = 'fleetops' AND kind = 'config'
             AND name IN (SELECT name FROM stack_files WHERE stack_id = ? AND source = 'repo')",
        )
        .bind(stack_id)
        .bind(stack_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "UPDATE stack_files SET source = 'fleetops' WHERE stack_id = ? AND source = 'repo'",
        )
        .bind(stack_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    /// Records a successful fetch, and puts the fetched file in the draft
    /// (its variables stay). False when the stack no longer has a git source.
    pub async fn record_git_fetch(
        &self,
        stack_id: i64,
        commit: &str,
        compose: &str,
        configs: &[(String, String)],
        trees: &std::collections::BTreeMap<String, String>,
        now: i64,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let git = sqlx::query(
            "UPDATE stack_git SET last_fetched_commit = ?, last_sync_at = ?, last_sync_error = NULL
             WHERE stack_id = ?",
        )
        .bind(commit)
        .bind(now)
        .bind(stack_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if git.rows_affected() == 0 {
            // Detached or removed while the fetch ran.
            return Ok(false);
        }
        sqlx::query(
            "UPDATE stack_drafts SET compose = ?, build_trees = ?, updated_at = ? WHERE stack_id = ?",
        )
            .bind(compose)
            .bind(serde_json::to_string(trees).expect("strings serialise"))
            .bind(now)
            .bind(stack_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM stack_files WHERE stack_id = ? AND source = 'repo'")
            .bind(stack_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for (name, value) in configs {
            self.put_file(&mut tx, stack_id, "config", "repo", name, value, None, now)
                .await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }

    pub async fn record_git_error(&self, stack_id: i64, error: &str, now: i64) -> Result<()> {
        sqlx::query(
            "UPDATE stack_git SET last_sync_error = ?, last_sync_at = ? WHERE stack_id = ?",
        )
        .bind(error)
        .bind(now)
        .bind(stack_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn set_git_pending(&self, stack_id: i64, commit: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE stack_git SET pending_commit = ? WHERE stack_id = ?")
            .bind(commit)
            .bind(stack_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn set_git_paused(&self, stack_id: i64, paused: bool) -> Result<()> {
        sqlx::query("UPDATE stack_git SET paused = ? WHERE stack_id = ?")
            .bind(paused)
            .bind(stack_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Polled stacks whose interval has passed, least recently synced first.
    pub async fn git_stacks_due(&self, now: i64) -> Result<Vec<StackRecord>> {
        let cols = STACK_COLS
            .split(", ")
            .map(|c| format!("s.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {cols} FROM stacks s JOIN stack_git g ON g.stack_id = s.id
             WHERE g.sync = 'poll' AND (g.last_sync_at IS NULL OR g.last_sync_at + g.poll_seconds <= ?)
             ORDER BY COALESCE(g.last_sync_at, 0), s.id"
        )))
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }
}
