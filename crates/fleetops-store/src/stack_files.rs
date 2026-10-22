//! Secret and config values of stacks (stack secrets spec §2), sealed with
//! the server key and bound to their row id; callers see plaintext.

use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use sqlx::{Sqlite, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub kind: String,
    /// `fleetops` (Secrets tab) or `repo` (fetched from git).
    pub source: String,
    pub name: String,
    pub value: String,
    pub updated_at: i64,
    /// The username.
    pub updated_by: Option<String>,
}

/// One file content a revision deployed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionFile {
    pub kind: String,
    pub name: String,
    pub value: String,
}

/// id, kind, source, name, sealed value, updated_at, username.
type FileRow = (i64, String, String, String, String, i64, Option<String>);

fn file_aad(id: i64) -> Vec<u8> {
    format!("stack-file:{id}").into_bytes()
}

fn revision_file_aad(id: i64) -> Vec<u8> {
    format!("stack-revision-file:{id}").into_bytes()
}

impl Store {
    fn open_file(&self, aad: &[u8], sealed: &str) -> Result<String> {
        let plain = self
            .secrets
            .open(aad, sealed)
            .map_err(|_| Error::Internal("a stack file cannot be decrypted".into()))?;
        String::from_utf8(plain).map_err(|_| Error::Internal("a stack file is corrupt".into()))
    }

    /// Every stored value of a stack, ordered by kind, name and source.
    pub async fn stack_files(&self, stack_id: i64) -> Result<Vec<StoredFile>> {
        let rows: Vec<FileRow> = sqlx::query_as(
            "SELECT f.id, f.kind, f.source, f.name, f.value, f.updated_at, u.username
             FROM stack_files f LEFT JOIN users u ON u.id = f.updated_by
             WHERE f.stack_id = ? ORDER BY f.kind, f.name, f.source",
        )
        .bind(stack_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|(id, kind, source, name, sealed, updated_at, updated_by)| {
                Ok(StoredFile {
                    value: self.open_file(&file_aad(id), &sealed)?,
                    kind,
                    source,
                    name,
                    updated_at,
                    updated_by,
                })
            })
            .collect()
    }

    /// Sets a Secrets-tab value, replacing one of the same kind and name.
    pub async fn set_stack_file(
        &self,
        stack_id: i64,
        kind: &str,
        name: &str,
        value: &str,
        user: Option<i64>,
        now: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        self.put_file(&mut tx, stack_id, kind, "fleetops", name, value, user, now)
            .await?;
        tx.commit().await.map_err(db_err)
    }

    /// Upserts a row, then seals the value with the row's id.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn put_file(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        stack_id: i64,
        kind: &str,
        source: &str,
        name: &str,
        value: &str,
        user: Option<i64>,
        now: i64,
    ) -> Result<()> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO stack_files (stack_id, kind, source, name, value, updated_at, updated_by)
             VALUES (?, ?, ?, ?, '', ?, ?)
             ON CONFLICT (stack_id, kind, source, name)
             DO UPDATE SET updated_at = excluded.updated_at, updated_by = excluded.updated_by
             RETURNING id",
        )
        .bind(stack_id)
        .bind(kind)
        .bind(source)
        .bind(name)
        .bind(now)
        .bind(user)
        .fetch_one(&mut **tx)
        .await
        .map_err(db_err)?;
        sqlx::query("UPDATE stack_files SET value = ? WHERE id = ?")
            .bind(self.secrets.seal(&file_aad(id), value.as_bytes()))
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Removes a Secrets-tab value; false when there was none.
    pub async fn remove_stack_file(&self, stack_id: i64, kind: &str, name: &str) -> Result<bool> {
        let r = sqlx::query(
            "DELETE FROM stack_files WHERE stack_id = ? AND kind = ? AND source = 'fleetops' AND name = ?",
        )
        .bind(stack_id)
        .bind(kind)
        .bind(name)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    pub(crate) async fn insert_revision_files(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        revision_id: i64,
        files: &[RevisionFile],
    ) -> Result<()> {
        for f in files {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO stack_revision_files (revision_id, kind, name, value)
                 VALUES (?, ?, ?, '') RETURNING id",
            )
            .bind(revision_id)
            .bind(&f.kind)
            .bind(&f.name)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_err)?;
            sqlx::query("UPDATE stack_revision_files SET value = ? WHERE id = ?")
                .bind(
                    self.secrets
                        .seal(&revision_file_aad(id), f.value.as_bytes()),
                )
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// The file contents revision `number` deployed.
    pub async fn revision_files(&self, stack_id: i64, number: i64) -> Result<Vec<RevisionFile>> {
        let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT f.id, f.kind, f.name, f.value FROM stack_revision_files f
             JOIN stack_revisions r ON r.id = f.revision_id
             WHERE r.stack_id = ? AND r.number = ? ORDER BY f.kind, f.name",
        )
        .bind(stack_id)
        .bind(number)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|(id, kind, name, sealed)| {
                Ok(RevisionFile {
                    value: self.open_file(&revision_file_aad(id), &sealed)?,
                    kind,
                    name,
                })
            })
            .collect()
    }

    /// The key of the digests in service config hashes, derived from the
    /// server key.
    pub fn file_digest_key(&self) -> [u8; 32] {
        self.key.derive(b"fleetops stack file digest v1")
    }
}
