use crate::{db_err, Store};
use dockerer_core::{Error, Result};
use serde_json::Value;

pub const AUDIT_RETENTION_KEY: &str = "audit.retention_days";
const DEFAULT_AUDIT_RETENTION_DAYS: i64 = 90;

impl Store {
    pub async fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        raw.map(|v| serde_json::from_str(&v))
            .transpose()
            .map_err(|e| Error::Internal(format!("corrupt setting {key}: {e}")))
    }

    pub async fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value.to_string())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn audit_retention_days(&self) -> Result<i64> {
        Ok(self
            .get_setting(AUDIT_RETENTION_KEY)
            .await?
            .and_then(|v| v.as_i64())
            .filter(|d| *d > 0)
            .unwrap_or(DEFAULT_AUDIT_RETENTION_DAYS))
    }
}
