//! Stacks, their drafts and revisions (stacks spec §1). Variables are
//! `KEY=value` lines, sealed with the server key; callers see plaintext.

use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use sqlx::AssertSqlSafe;
use utoipa::ToSchema;

/// What a revision built for one service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct BuiltImage {
    /// The git tree id of the build context ("" on an inline stack).
    pub tree: String,
    /// The image tag the build produced.
    pub image: String,
}

/// Service → what was built for it.
pub type Builds = std::collections::BTreeMap<String, BuiltImage>;

/// Revisions kept per stack (the deployed one is always kept too).
pub const STACK_REVISIONS_KEPT: i64 = 25;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct StackRecord {
    pub id: i64,
    pub env_id: i64,
    pub name: String,
    /// inline | git
    pub source: String,
    pub deployed_revision: Option<i64>,
    pub created_at: i64,
    pub created_by: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct RevisionRecord {
    pub id: i64,
    pub stack_id: i64,
    pub number: i64,
    pub compose: String,
    pub git_commit: Option<String>,
    /// deploying | deployed | failed | superseded
    pub status: String,
    pub error: Option<String>,
    /// deploy | rollback
    pub kind: String,
    pub rollback_of: Option<i64>,
    pub created_at: i64,
    pub created_by: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Draft {
    pub compose: String,
    pub variables: Vec<String>,
    pub updated_at: i64,
}

pub(crate) const STACK_COLS: &str =
    "id, environment_id AS env_id, name, source, deployed_revision, created_at, created_by";
const REV_COLS: &str = "id, stack_id, number, compose, git_commit, status, error, kind, rollback_of, created_at, created_by";

fn draft_aad(stack_id: i64) -> Vec<u8> {
    format!("stack:{stack_id}").into_bytes()
}

fn revision_aad(id: i64) -> Vec<u8> {
    format!("stack-revision:{id}").into_bytes()
}

impl Store {
    fn seal_vars(&self, aad: &[u8], variables: &[String]) -> String {
        let json = serde_json::to_vec(variables).expect("strings serialise");
        self.secrets.seal(aad, &json)
    }

    fn open_vars(&self, aad: &[u8], sealed: &str) -> Result<Vec<String>> {
        let plain = self
            .secrets
            .open(aad, sealed)
            .map_err(|_| Error::Internal("stack variables cannot be decrypted".into()))?;
        serde_json::from_slice(&plain)
            .map_err(|e| Error::Internal(format!("stack variables are corrupt: {e}")))
    }

    /// Creates the stack with its first draft.
    pub async fn create_stack(
        &self,
        env_id: i64,
        name: &str,
        compose: &str,
        variables: &[String],
        user: Option<i64>,
        now: i64,
    ) -> Result<StackRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let rec: StackRecord = sqlx::query_as(AssertSqlSafe(format!(
            "INSERT INTO stacks (environment_id, name, source, created_at, created_by)
             VALUES (?, ?, 'inline', ?, ?) RETURNING {STACK_COLS}"
        )))
        .bind(env_id)
        .bind(name)
        .bind(now)
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("a stack named '{name}' already exists")),
            other => other,
        })?;
        sqlx::query(
            "INSERT INTO stack_drafts (stack_id, compose, variables, updated_at, updated_by) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(rec.id)
        .bind(compose)
        .bind(self.seal_vars(&draft_aad(rec.id), variables))
        .bind(now)
        .bind(user)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(rec)
    }

    pub async fn list_stacks(&self, env_id: i64) -> Result<Vec<StackRecord>> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {STACK_COLS} FROM stacks WHERE environment_id = ? ORDER BY name"
        )))
        .bind(env_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn stack_by_name(&self, env_id: i64, name: &str) -> Result<StackRecord> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {STACK_COLS} FROM stacks WHERE environment_id = ? AND name = ?"
        )))
        .bind(env_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("stack '{name}'")))
    }

    pub async fn stack_by_id(&self, id: i64) -> Result<StackRecord> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {STACK_COLS} FROM stacks WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("stack {id}")))
    }

    pub async fn delete_stack(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM stacks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn save_draft(
        &self,
        stack_id: i64,
        compose: &str,
        variables: &[String],
        user: Option<i64>,
        now: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO stack_drafts (stack_id, compose, variables, updated_at, updated_by) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(stack_id) DO UPDATE SET compose = excluded.compose, variables = excluded.variables,
               updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        )
        .bind(stack_id)
        .bind(compose)
        .bind(self.seal_vars(&draft_aad(stack_id), variables))
        .bind(now)
        .bind(user)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn draft(&self, stack_id: i64) -> Result<Option<Draft>> {
        let row: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT compose, variables, updated_at FROM stack_drafts WHERE stack_id = ?",
        )
        .bind(stack_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(|(compose, sealed, updated_at)| {
            Ok(Draft {
                compose,
                variables: self.open_vars(&draft_aad(stack_id), &sealed)?,
                updated_at,
            })
        })
        .transpose()
    }

    /// Replaces only the draft's variables, so a git stack's fetched file is
    /// never written back.
    pub async fn save_draft_variables(
        &self,
        stack_id: i64,
        variables: &[String],
        user: Option<i64>,
        now: i64,
    ) -> Result<()> {
        let done = sqlx::query(
            "UPDATE stack_drafts SET variables = ?, updated_at = ?, updated_by = ? WHERE stack_id = ?",
        )
        .bind(self.seal_vars(&draft_aad(stack_id), variables))
        .bind(now)
        .bind(user)
        .bind(stack_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound(format!("draft of stack {stack_id}")));
        }
        Ok(())
    }

    /// The draft and the git commit its file came from, read together.
    pub async fn git_draft(&self, stack_id: i64) -> Result<Option<(Draft, Option<String>)>> {
        let row: Option<(String, String, i64, Option<String>)> = sqlx::query_as(
            "SELECT d.compose, d.variables, d.updated_at, g.last_fetched_commit
             FROM stack_drafts d LEFT JOIN stack_git g ON g.stack_id = d.stack_id
             WHERE d.stack_id = ?",
        )
        .bind(stack_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(|(compose, sealed, updated_at, commit)| {
            Ok((
                Draft {
                    compose,
                    variables: self.open_vars(&draft_aad(stack_id), &sealed)?,
                    updated_at,
                },
                commit,
            ))
        })
        .transpose()
    }

    /// Records a new revision, numbered after the newest, as `deploying`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_revision(
        &self,
        stack_id: i64,
        compose: &str,
        variables: &[String],
        kind: &str,
        rollback_of: Option<i64>,
        git_commit: Option<&str>,
        user: Option<i64>,
        now: i64,
        files: &[crate::RevisionFile],
        builds: &Builds,
    ) -> Result<RevisionRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let rec: RevisionRecord = sqlx::query_as(AssertSqlSafe(format!(
            "INSERT INTO stack_revisions (stack_id, number, compose, variables, git_commit, status, kind, rollback_of, created_at, created_by)
             VALUES (?, (SELECT COALESCE(MAX(number), 0) + 1 FROM stack_revisions WHERE stack_id = ?), ?, '', ?, 'deploying', ?, ?, ?, ?)
             RETURNING {REV_COLS}"
        )))
        .bind(stack_id)
        .bind(stack_id)
        .bind(compose)
        .bind(git_commit)
        .bind(kind)
        .bind(rollback_of)
        .bind(now)
        .bind(user)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("UPDATE stack_revisions SET variables = ?, builds = ? WHERE id = ?")
            .bind(self.seal_vars(&revision_aad(rec.id), variables))
            .bind(serde_json::to_string(builds).expect("strings serialise"))
            .bind(rec.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        self.insert_revision_files(&mut tx, rec.id, files).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(rec)
    }

    pub async fn revision(&self, stack_id: i64, number: i64) -> Result<RevisionRecord> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {REV_COLS} FROM stack_revisions WHERE stack_id = ? AND number = ?"
        )))
        .bind(stack_id)
        .bind(number)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("revision {number}")))
    }

    pub async fn revision_variables(&self, stack_id: i64, number: i64) -> Result<Vec<String>> {
        let (id, sealed): (i64, String) = sqlx::query_as(
            "SELECT id, variables FROM stack_revisions WHERE stack_id = ? AND number = ?",
        )
        .bind(stack_id)
        .bind(number)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("revision {number}")))?;
        self.open_vars(&revision_aad(id), &sealed)
    }

    /// What revision `number` built; empty for one without builds.
    pub async fn revision_builds(&self, stack_id: i64, number: i64) -> Result<Builds> {
        let text: String = sqlx::query_scalar(
            "SELECT builds FROM stack_revisions WHERE stack_id = ? AND number = ?",
        )
        .bind(stack_id)
        .bind(number)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("revision {number}")))?;
        serde_json::from_str(&text)
            .map_err(|e| Error::Internal(format!("revision builds are corrupt: {e}")))
    }

    /// The build context tree ids the last git sync recorded with the draft.
    pub async fn draft_build_trees(
        &self,
        stack_id: i64,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let text: Option<String> =
            sqlx::query_scalar("SELECT build_trees FROM stack_drafts WHERE stack_id = ?")
                .bind(stack_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        match text {
            Some(t) => serde_json::from_str(&t)
                .map_err(|e| Error::Internal(format!("draft build trees are corrupt: {e}"))),
            None => Ok(Default::default()),
        }
    }

    /// Newest first.
    pub async fn list_revisions(&self, stack_id: i64) -> Result<Vec<RevisionRecord>> {
        sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {REV_COLS} FROM stack_revisions WHERE stack_id = ? ORDER BY number DESC"
        )))
        .bind(stack_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    /// Ends a deploy. On success the revision becomes `deployed`, the one
    /// before it `superseded`, and the stack points at it; on failure it
    /// becomes `failed` with the error.
    pub async fn finish_revision(
        &self,
        stack_id: i64,
        number: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        match error {
            Some(e) => {
                sqlx::query(
                    "UPDATE stack_revisions SET status = 'failed', error = ? WHERE stack_id = ? AND number = ?",
                )
                .bind(e)
                .bind(stack_id)
                .bind(number)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            None => {
                sqlx::query(
                    "UPDATE stack_revisions SET status = 'superseded' WHERE stack_id = ? AND status = 'deployed'",
                )
                .bind(stack_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
                sqlx::query(
                    "UPDATE stack_revisions SET status = 'deployed', error = NULL WHERE stack_id = ? AND number = ?",
                )
                .bind(stack_id)
                .bind(number)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
                sqlx::query("UPDATE stacks SET deployed_revision = ? WHERE id = ?")
                    .bind(number)
                    .bind(stack_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Notes an error on a revision that stays `deploying` (its restore
    /// could not run yet).
    pub async fn note_revision_error(&self, stack_id: i64, number: i64, error: &str) -> Result<()> {
        sqlx::query("UPDATE stack_revisions SET error = ? WHERE stack_id = ? AND number = ?")
            .bind(error)
            .bind(stack_id)
            .bind(number)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Every revision still `deploying`, with its stack.
    pub async fn deploying_revisions(&self) -> Result<Vec<(StackRecord, RevisionRecord)>> {
        let revs: Vec<RevisionRecord> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {REV_COLS} FROM stack_revisions WHERE status = 'deploying' ORDER BY id"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(revs.len());
        for r in revs {
            let stack: StackRecord = sqlx::query_as(AssertSqlSafe(format!(
                "SELECT {STACK_COLS} FROM stacks WHERE id = ?"
            )))
            .bind(r.stack_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
            out.push((stack, r));
        }
        Ok(out)
    }

    /// Deletes all but the newest `keep` revisions of each stack, never the
    /// deployed one or one still deploying. Returns how many were deleted.
    pub async fn prune_stack_revisions(&self, keep: i64) -> Result<u64> {
        let res = sqlx::query(
            "DELETE FROM stack_revisions WHERE status NOT IN ('deployed', 'deploying') AND id IN (
               SELECT r.id FROM stack_revisions r
               WHERE (SELECT COUNT(*) FROM stack_revisions n
                      WHERE n.stack_id = r.stack_id AND n.number > r.number) >= ?)",
        )
        .bind(keep)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected())
    }
}
