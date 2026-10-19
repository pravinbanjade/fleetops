//! The role tables (`1000002_roles.sql`).

use fleetops_core::access::{CustomRef, Grant};
use fleetops_core::perm::PermSet;
use fleetops_core::{Error, Result};
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::BTreeSet;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRecord {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub perms: PermSet,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(sqlx::FromRow)]
struct RoleRow {
    id: i64,
    name: String,
    description: Option<String>,
    permissions: String,
    created_at: i64,
    updated_at: i64,
}

impl RoleRow {
    fn into_record(self) -> Result<RoleRecord> {
        Ok(RoleRecord {
            perms: decode_perms(&self.permissions)?,
            id: self.id,
            name: self.name,
            description: self.description,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// `ee_roles.permissions`: a JSON array of `row.verb`. A name this build
/// does not know (a newer server wrote it) is skipped, not an error.
pub fn decode_perms(json: &str) -> Result<PermSet> {
    let v: Vec<String> = serde_json::from_str(json)
        .map_err(|e| Error::Internal(format!("bad permissions in ee_roles: {e}")))?;
    Ok(v.iter().filter_map(|p| p.parse().ok()).collect())
}

fn encode_perms(p: PermSet) -> String {
    serde_json::to_string(&p.to_strs()).expect("strings serialise")
}

fn encode_ns(ns: Option<&BTreeSet<String>>) -> Option<String> {
    ns.map(|n| serde_json::to_string(n).expect("names serialise"))
}

const COLS: &str = "id, name, description, permissions, created_at, updated_at";

fn name_taken(name: &str) -> impl FnOnce(sqlx::Error) -> Error + '_ {
    move |e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict(format!("a role named '{name}' already exists"))
        }
        _ => db_err(e),
    }
}

/// `name` and `description` already validated.
pub async fn create(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    perms: PermSet,
    now: i64,
) -> Result<RoleRecord> {
    sqlx::query_as::<_, RoleRow>(AssertSqlSafe(format!(
        "INSERT INTO ee_roles (name, description, permissions, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?) RETURNING {COLS}"
    )))
    .bind(name)
    .bind(description)
    .bind(encode_perms(perms))
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(name_taken(name))?
    .into_record()
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<RoleRecord> {
    sqlx::query_as::<_, RoleRow>(AssertSqlSafe(format!(
        "SELECT {COLS} FROM ee_roles WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("role {id}")))?
    .into_record()
}

/// Every role, by name, with how many users and teams hold it.
pub async fn list(pool: &SqlitePool) -> Result<Vec<(RoleRecord, i64, i64)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        role: RoleRow,
        users: i64,
        teams: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT r.id, r.name, r.description, r.permissions, r.created_at, r.updated_at,
            (SELECT COUNT(DISTINCT user_id) FROM ee_user_role_bindings b WHERE b.role_id = r.id) AS users,
            (SELECT COUNT(DISTINCT team_id) FROM ee_team_role_bindings b WHERE b.role_id = r.id) AS teams
         FROM ee_roles r ORDER BY r.name",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|r| Ok((r.role.into_record()?, r.users, r.teams)))
        .collect()
}

/// Changes what is given; `name` and `description` already validated.
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: Option<&str>,
    description: Option<Option<&str>>,
    perms: Option<PermSet>,
    now: i64,
) -> Result<RoleRecord> {
    let old = get(pool, id).await?;
    let name = name.unwrap_or(&old.name);
    let description = description.unwrap_or(old.description.as_deref());
    let perms = perms.unwrap_or(old.perms);
    sqlx::query_as::<_, RoleRow>(AssertSqlSafe(format!(
        "UPDATE ee_roles SET name = ?, description = ?, permissions = ?, updated_at = ?
         WHERE id = ? RETURNING {COLS}"
    )))
    .bind(name)
    .bind(description)
    .bind(encode_perms(perms))
    .bind(now)
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(name_taken(name))?
    .into_record()
}

/// Deletes the role and every grant of it; how many grants went.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<u64> {
    get(pool, id).await?;
    let mut n = 0;
    for table in ["ee_user_role_bindings", "ee_team_role_bindings"] {
        n += sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE role_id = ?"
        )))
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err)?
        .rows_affected();
    }
    sqlx::query("DELETE FROM ee_roles WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(n)
}

/// Every user the role reaches: direct holders and members of holding teams.
pub async fn holders(pool: &SqlitePool, id: i64) -> Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT user_id FROM ee_user_role_bindings WHERE role_id = ?1
         UNION
         SELECT m.user_id FROM ee_team_role_bindings b
         JOIN ee_team_members m ON m.team_id = b.team_id WHERE b.role_id = ?1",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

/// A direct grant row joined to its role.
type DirectRow = (i64, i64, i64, String, String, Option<String>, i64);

const DIRECT: &str =
    "SELECT b.user_id, b.environment_id, r.id, r.name, r.permissions, b.namespaces, b.updated_at
    FROM ee_user_role_bindings b JOIN ee_roles r ON r.id = b.role_id";

fn direct_grant(r: &DirectRow) -> Result<Grant> {
    let ns = r
        .5
        .as_deref()
        .map(serde_json::from_str::<BTreeSet<String>>)
        .transpose()
        .map_err(|e| Error::Internal(format!("bad namespaces in ee_user_role_bindings: {e}")))?;
    Ok(Grant::custom(
        CustomRef {
            id: r.2,
            name: r.3.clone(),
        },
        decode_perms(&r.4)?,
        ns,
    ))
}

/// The custom grant straight to `user_id` on `env_id`, if any.
pub async fn direct(pool: &SqlitePool, user_id: i64, env_id: i64) -> Result<Option<Grant>> {
    let row: Option<DirectRow> = sqlx::query_as(AssertSqlSafe(format!(
        "{DIRECT} WHERE b.user_id = ? AND b.environment_id = ?"
    )))
    .bind(user_id)
    .bind(env_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    row.as_ref().map(direct_grant).transpose()
}

/// `(environment id, grant)` for each custom grant straight to `user_id`.
pub async fn user_direct(pool: &SqlitePool, user_id: i64) -> Result<Vec<(i64, Grant)>> {
    let rows: Vec<DirectRow> =
        sqlx::query_as(AssertSqlSafe(format!("{DIRECT} WHERE b.user_id = ?")))
            .bind(user_id)
            .fetch_all(pool)
            .await
            .map_err(db_err)?;
    rows.iter().map(|r| Ok((r.1, direct_grant(r)?))).collect()
}

/// `(user id, grant)` for each custom grant straight to a user on `env_id`.
pub async fn env_direct(pool: &SqlitePool, env_id: i64) -> Result<Vec<(i64, Grant)>> {
    let rows: Vec<DirectRow> = sqlx::query_as(AssertSqlSafe(format!(
        "{DIRECT} WHERE b.environment_id = ?"
    )))
    .bind(env_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows.iter().map(|r| Ok((r.0, direct_grant(r)?))).collect()
}

/// Stores the direct grant and returns it. Callers check the role exists.
pub async fn set_direct(
    pool: &SqlitePool,
    user_id: i64,
    env_id: i64,
    role_id: i64,
    namespaces: Option<&BTreeSet<String>>,
    now: i64,
) -> Result<Grant> {
    sqlx::query(
        "INSERT INTO ee_user_role_bindings (user_id, environment_id, role_id, namespaces, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, environment_id) DO UPDATE SET role_id = excluded.role_id,
             namespaces = excluded.namespaces, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(env_id)
    .bind(role_id)
    .bind(encode_ns(namespaces))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(db_err)?;
    direct(pool, user_id, env_id)
        .await?
        .ok_or_else(|| Error::Internal("custom grant vanished".into()))
}

pub async fn clear_direct(pool: &SqlitePool, user_id: i64, env_id: i64) -> Result<bool> {
    let r =
        sqlx::query("DELETE FROM ee_user_role_bindings WHERE user_id = ? AND environment_id = ?")
            .bind(user_id)
            .bind(env_id)
            .execute(pool)
            .await
            .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

/// A (user, environment) with both a built-in and a custom direct grant
/// can only come from running the Community image in between: keep the
/// newer one (a tie keeps the built-in).
pub async fn reconcile_direct_grants(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "DELETE FROM ee_user_role_bindings WHERE EXISTS (
             SELECT 1 FROM role_bindings b
             WHERE b.user_id = ee_user_role_bindings.user_id
               AND b.environment_id = ee_user_role_bindings.environment_id
               AND b.updated_at >= ee_user_role_bindings.updated_at)",
    )
    .execute(pool)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM role_bindings WHERE EXISTS (
             SELECT 1 FROM ee_user_role_bindings c
             WHERE c.user_id = role_bindings.user_id
               AND c.environment_id = role_bindings.environment_id)",
    )
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Who holds the role, for its page: `(user id, username, environment)`
/// for direct grants and `(team id, team name, environment)` for teams.
pub async fn holder_rows(
    pool: &SqlitePool,
    id: i64,
) -> Result<(Vec<(i64, String, String)>, Vec<(i64, String, String)>)> {
    let users = sqlx::query_as(
        "SELECT u.id, u.username, e.name FROM ee_user_role_bindings b
         JOIN users u ON u.id = b.user_id JOIN environments e ON e.id = b.environment_id
         WHERE b.role_id = ? ORDER BY u.username, e.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    let teams = sqlx::query_as(
        "SELECT t.id, t.name, e.name FROM ee_team_role_bindings b
         JOIN ee_teams t ON t.id = b.team_id JOIN environments e ON e.id = b.environment_id
         WHERE b.role_id = ? ORDER BY t.name, e.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok((users, teams))
}
