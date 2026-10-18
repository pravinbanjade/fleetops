//! The SSO tables (`1000003_sso.sql`).

use super::model::{ConnectionSettings, Secrets};
use fleetops_core::{Error, Result};
use fleetops_store::Store;
use sqlx::SqlitePool;

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

fn conflict_or(e: sqlx::Error, what: &str) -> Error {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict(format!("{what} already exists"))
        }
        _ => db_err(e),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub settings: ConnectionSettings,
    pub require_mapped_group: bool,
    pub session_hours: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(sqlx::FromRow)]
struct ConnectionRow {
    id: i64,
    name: String,
    enabled: bool,
    settings: String,
    require_mapped_group: bool,
    session_hours: i64,
    created_at: i64,
    updated_at: i64,
}

impl ConnectionRow {
    fn into_connection(self) -> Result<Connection> {
        Ok(Connection {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            settings: serde_json::from_str(&self.settings)
                .map_err(|e| Error::Internal(format!("bad SSO settings: {e}")))?,
            require_mapped_group: self.require_mapped_group,
            session_hours: self.session_hours,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

const COLS: &str =
    "id, name, enabled, settings, require_mapped_group, session_hours, created_at, updated_at";

fn aad(id: i64) -> Vec<u8> {
    format!("sso:{id}").into_bytes()
}

fn settings_json(s: &ConnectionSettings) -> Result<String> {
    serde_json::to_string(s).map_err(|e| Error::Internal(format!("SSO settings: {e}")))
}

pub struct NewConnection<'a> {
    pub name: &'a str,
    pub settings: &'a ConnectionSettings,
    pub secrets: &'a Secrets,
    pub enabled: bool,
    pub require_mapped_group: bool,
    pub session_hours: i64,
    pub now: i64,
}

pub async fn create(store: &Store, c: &NewConnection<'_>) -> Result<Connection> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO ee_sso_connections
             (name, kind, enabled, settings, require_mapped_group, session_hours, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(c.name)
    .bind(c.settings.kind())
    .bind(c.enabled)
    .bind(settings_json(c.settings)?)
    .bind(c.require_mapped_group)
    .bind(c.session_hours)
    .bind(c.now)
    .bind(c.now)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| conflict_or(e, "a connection with that name"))?;
    sqlx::query("UPDATE ee_sso_connections SET secrets = ? WHERE id = ?")
        .bind(seal(store, id, c.secrets)?)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    get(store.pool(), id).await
}

fn seal(store: &Store, id: i64, s: &Secrets) -> Result<String> {
    let json = serde_json::to_vec(s).map_err(|e| Error::Internal(format!("SSO secrets: {e}")))?;
    Ok(store.secrets().seal(&aad(id), &json))
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Connection>> {
    sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {COLS} FROM ee_sso_connections ORDER BY name"
    ))
    .fetch_all(pool)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(ConnectionRow::into_connection)
    .collect()
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Connection> {
    sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {COLS} FROM ee_sso_connections WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("connection {id}")))?
    .into_connection()
}

/// The stored secrets; empty when none were saved or the server key changed.
pub async fn secrets(store: &Store, id: i64) -> Result<Secrets> {
    let sealed: Option<String> =
        sqlx::query_scalar("SELECT secrets FROM ee_sso_connections WHERE id = ?")
            .bind(id)
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?
            .flatten();
    let Some(sealed) = sealed else {
        return Ok(Secrets::default());
    };
    match store.secrets().open(&aad(id), &sealed) {
        Ok(plain) => {
            serde_json::from_slice(&plain).map_err(|e| Error::Internal(format!("SSO secrets: {e}")))
        }
        Err(e) => {
            tracing::warn!(connection = id, error = %e, "SSO secrets do not open with this server key");
            Ok(Secrets::default())
        }
    }
}

pub async fn update(
    store: &Store,
    c: &Connection,
    secrets: &Secrets,
    now: i64,
) -> Result<Connection> {
    sqlx::query(
        "UPDATE ee_sso_connections SET name = ?, enabled = ?, settings = ?, secrets = ?,
             require_mapped_group = ?, session_hours = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&c.name)
    .bind(c.enabled)
    .bind(settings_json(&c.settings)?)
    .bind(seal(store, c.id, secrets)?)
    .bind(c.require_mapped_group)
    .bind(c.session_hours)
    .bind(now)
    .bind(c.id)
    .execute(store.pool())
    .await
    .map_err(|e| conflict_or(e, "a connection with that name"))?;
    get(store.pool(), c.id).await
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_sso_connections WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

/// Users per connection.
pub async fn user_counts(pool: &SqlitePool) -> Result<std::collections::HashMap<i64, i64>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT connection_id, COUNT(*) FROM ee_sso_identities GROUP BY connection_id",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

/// The user ids a connection manages.
pub async fn connection_users(pool: &SqlitePool, id: i64) -> Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT user_id FROM ee_sso_identities WHERE connection_id = ? ORDER BY user_id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

/// Every SSO user with their connection's name.
pub async fn managed_users(pool: &SqlitePool) -> Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT i.user_id, c.name FROM ee_sso_identities i
         JOIN ee_sso_connections c ON c.id = i.connection_id",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

pub async fn is_managed(pool: &SqlitePool, user_id: i64) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ee_sso_identities WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(db_err)?;
    Ok(n > 0)
}

/// Whether an admin exists that signs in with a local password.
pub async fn local_admin_exists(pool: &SqlitePool) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users u WHERE u.role = 'admin'
         AND NOT EXISTS (SELECT 1 FROM ee_sso_identities i WHERE i.user_id = u.id)",
    )
    .fetch_one(pool)
    .await
    .map_err(db_err)?;
    Ok(n > 0)
}

// ---- mappings ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Mapping {
    pub id: i64,
    pub connection_id: i64,
    pub group_value: String,
    pub team_id: Option<i64>,
    pub team_name: Option<String>,
    pub is_admin: bool,
}

pub async fn mappings(pool: &SqlitePool, connection_id: i64) -> Result<Vec<Mapping>> {
    sqlx::query_as(
        "SELECT m.id, m.connection_id, m.group_value, m.team_id, t.name AS team_name, m.is_admin
         FROM ee_sso_mappings m LEFT JOIN ee_teams t ON t.id = m.team_id
         WHERE m.connection_id = ? ORDER BY m.group_value, t.name",
    )
    .bind(connection_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

pub async fn add_mapping(
    pool: &SqlitePool,
    connection_id: i64,
    group: &str,
    team_id: Option<i64>,
    now: i64,
) -> Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO ee_sso_mappings (connection_id, group_value, team_id, is_admin, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(connection_id)
    .bind(group)
    .bind(team_id)
    .bind(team_id.is_none())
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|e| conflict_or(e, "that mapping"))
}

pub async fn delete_mapping(pool: &SqlitePool, connection_id: i64, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_sso_mappings WHERE id = ? AND connection_id = ?")
        .bind(id)
        .bind(connection_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}
