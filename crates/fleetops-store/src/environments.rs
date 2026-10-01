use crate::{db_err, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct EnvironmentRecord {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLS: &str = "id, name, kind, url, created_at, updated_at";

impl Store {
    pub async fn create_environment(
        &self,
        name: &str,
        kind: &str,
        url: &str,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut conn = self.pool.acquire().await.map_err(db_err)?;
        insert_environment(&mut conn, name, kind, url, now).await
    }

    pub async fn list_environments(&self) -> Result<Vec<EnvironmentRecord>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM environments ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn get_environment(&self, id: i64) -> Result<EnvironmentRecord> {
        sqlx::query_as::<_, EnvironmentRecord>(&format!(
            "SELECT {COLS} FROM environments WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound(format!("environment {id}")))
    }

    pub async fn update_environment(
        &self,
        id: i64,
        name: &str,
        url: &str,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        sqlx::query_as::<_, EnvironmentRecord>(&format!(
            "UPDATE environments SET name = ?, url = ?, updated_at = ? WHERE id = ? RETURNING {COLS}"
        ))
        .bind(name)
        .bind(url)
        .bind(now)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
            other => other,
        })?
        .ok_or_else(|| Error::NotFound(format!("environment {id}")))
    }

    pub async fn delete_environment(&self, id: i64) -> Result<()> {
        let res = sqlx::query("DELETE FROM environments WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("environment {id}")));
        }
        Ok(())
    }
}

/// One environment row; shared by plain creates and creates that store a
/// credential in the same transaction.
pub(crate) async fn insert_environment(
    conn: &mut sqlx::SqliteConnection,
    name: &str,
    kind: &str,
    url: &str,
    now: i64,
) -> Result<EnvironmentRecord> {
    sqlx::query_as(&format!(
        "INSERT INTO environments (name, kind, url, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?) RETURNING {COLS}"
    ))
    .bind(name)
    .bind(kind)
    .bind(url)
    .bind(now)
    .bind(now)
    .fetch_one(conn)
    .await
    .map_err(|e| match db_err(e) {
        Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
        other => other,
    })
}
