use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

pub struct NewAudit<'a> {
    pub ts: i64,
    pub user_id: Option<i64>,
    pub environment_id: Option<i64>,
    pub action: &'a str,
    pub target_type: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub result: &'a str,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub environment_id: Option<i64>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub result: String,
    pub detail: Option<Value>,
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    ts: i64,
    user_id: Option<i64>,
    username: Option<String>,
    environment_id: Option<i64>,
    action: String,
    target_type: Option<String>,
    target_id: Option<String>,
    result: String,
    detail: Option<String>,
}

impl TryFrom<AuditRow> for AuditEntry {
    type Error = Error;

    fn try_from(r: AuditRow) -> Result<Self> {
        let detail = r
            .detail
            .map(|d| serde_json::from_str(&d))
            .transpose()
            .map_err(|e| Error::Internal(format!("corrupt audit detail: {e}")))?;
        Ok(AuditEntry {
            id: r.id,
            ts: r.ts,
            user_id: r.user_id,
            username: r.username,
            environment_id: r.environment_id,
            action: r.action,
            target_type: r.target_type,
            target_id: r.target_id,
            result: r.result,
            detail,
        })
    }
}

impl Store {
    pub async fn insert_audit(&self, a: &NewAudit<'_>) -> Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO audit_log (ts, user_id, environment_id, action, target_type, target_id, result, detail)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(a.ts)
        .bind(a.user_id)
        .bind(a.environment_id)
        .bind(a.action)
        .bind(a.target_type)
        .bind(a.target_id)
        .bind(a.result)
        .bind(a.detail.as_ref().map(Value::to_string))
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn list_audit(&self, limit: i64, before_id: Option<i64>) -> Result<Vec<AuditEntry>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT a.id, a.ts, a.user_id, u.username, a.environment_id, a.action,
                    a.target_type, a.target_id, a.result, a.detail
             FROM audit_log a LEFT JOIN users u ON u.id = a.user_id
             WHERE (? IS NULL OR a.id < ?)
             ORDER BY a.id DESC LIMIT ?",
        )
        .bind(before_id)
        .bind(before_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(AuditEntry::try_from).collect()
    }

    pub async fn prune_audit(&self, older_than_ts: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM audit_log WHERE ts < ?")
            .bind(older_than_ts)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }
}
