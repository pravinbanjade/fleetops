//! Encrypted environment credentials (spec §3). The store seals and opens
//! them, so callers see plaintext or `Credential::Unreadable` only.

use crate::{db_err, EnvironmentRecord, Store};
use fleetops_core::env::Credential;
use fleetops_core::Result;
use serde_json::Value;
use std::collections::HashMap;

fn aad(env_id: i64) -> Vec<u8> {
    env_id.to_string().into_bytes()
}

impl Store {
    /// Creates the environment and its credential in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_environment_with_credential(
        &self,
        name: &str,
        kind: &str,
        url: &str,
        cred_kind: &str,
        plaintext: &str,
        summary: &Value,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let rec = crate::environments::insert_environment(&mut tx, name, kind, url, now).await?;
        let sealed = self.secrets.seal(&aad(rec.id), plaintext.as_bytes());
        sqlx::query("INSERT INTO env_credentials (env_id, kind, ciphertext, summary, updated_at) VALUES (?, ?, ?, ?, ?)")
            .bind(rec.id)
            .bind(cred_kind)
            .bind(sealed)
            .bind(summary.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(rec)
    }

    pub async fn put_env_credential(
        &self,
        env_id: i64,
        cred_kind: &str,
        plaintext: &str,
        summary: &Value,
        now: i64,
    ) -> Result<()> {
        let sealed = self.secrets.seal(&aad(env_id), plaintext.as_bytes());
        sqlx::query(
            "INSERT INTO env_credentials (env_id, kind, ciphertext, summary, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(env_id) DO UPDATE SET kind = excluded.kind, ciphertext = excluded.ciphertext,
               summary = excluded.summary, updated_at = excluded.updated_at",
        )
        .bind(env_id)
        .bind(cred_kind)
        .bind(sealed)
        .bind(summary.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn env_credential(&self, env_id: i64) -> Result<Option<Credential>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT ciphertext FROM env_credentials WHERE env_id = ?")
                .bind(env_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(
            row.map(|(sealed,)| match self.secrets.open(&aad(env_id), &sealed) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Credential::Kubeconfig(text),
                    Err(_) => Credential::Unreadable("credential is not UTF-8".into()),
                },
                Err(e) => Credential::Unreadable(e.to_string()),
            }),
        )
    }

    pub async fn env_credential_summaries(&self) -> Result<HashMap<i64, Value>> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT env_id, summary FROM env_credentials")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, s)| serde_json::from_str(&s).ok().map(|v| (id, v)))
            .collect())
    }
}
