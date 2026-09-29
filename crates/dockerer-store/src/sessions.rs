use crate::{db_err, Store};
use dockerer_core::Result;

pub struct NewSession<'a> {
    pub user_id: i64,
    pub token_hash: &'a str,
    pub csrf_token: &'a str,
    pub now: i64,
    pub expires_at: i64,
    pub user_agent: Option<&'a str>,
    pub ip: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: i64,
    pub user_id: i64,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

impl SessionRecord {
    pub fn is_active(&self, now: i64, idle_timeout: i64) -> bool {
        now < self.expires_at && now - self.last_seen_at < idle_timeout
    }
}

const COLS: &str = "id, user_id, csrf_token, created_at, expires_at, last_seen_at, user_agent, ip";

impl Store {
    pub async fn create_session(&self, s: &NewSession<'_>) -> Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO sessions (user_id, token_hash, csrf_token, created_at, expires_at, last_seen_at, user_agent, ip)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(s.user_id)
        .bind(s.token_hash)
        .bind(s.csrf_token)
        .bind(s.now)
        .bind(s.expires_at)
        .bind(s.now)
        .bind(s.user_agent)
        .bind(s.ip)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn find_session(&self, token_hash: &str) -> Result<Option<SessionRecord>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM sessions WHERE token_hash = ?"))
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn touch_session(&self, id: i64, now: i64) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn list_sessions(&self, user_id: i64) -> Result<Vec<SessionRecord>> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM sessions WHERE user_id = ? ORDER BY last_seen_at DESC, id DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn delete_session(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Returns false when the session does not exist or belongs to someone else.
    pub async fn delete_user_session(&self, user_id: i64, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn delete_user_sessions(&self, user_id: i64, except: Option<i64>) -> Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND (? IS NULL OR id <> ?)")
            .bind(user_id)
            .bind(except)
            .bind(except)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    pub async fn delete_expired_sessions(&self, now: i64, idle_timeout: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?")
            .bind(now)
            .bind(now - idle_timeout)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }
}
