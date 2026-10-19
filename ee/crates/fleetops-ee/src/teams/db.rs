//! The team tables (`1000001_teams.sql`).

use fleetops_core::access::{CustomRef, EnvRole, Grant};
use fleetops_core::{Error, Result, Role, User};
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::BTreeSet;

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TeamRecord {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One team's grant on one environment, with the names a page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamBinding {
    pub team_id: i64,
    pub environment_id: i64,
    pub env_name: String,
    pub env_kind: String,
    pub grant: Grant,
    pub updated_at: i64,
}

#[derive(sqlx::FromRow)]
struct BindingRow {
    team_id: i64,
    environment_id: i64,
    env_name: String,
    env_kind: String,
    role: Option<String>,
    role_id: Option<i64>,
    role_name: Option<String>,
    permissions: Option<String>,
    namespaces: Option<String>,
    updated_at: i64,
}

/// A grant from a row of `ee_team_grants` (built-in `role`, or a custom
/// role's id, name and permissions). Namespaces as in `role_bindings`.
pub(crate) fn grant(
    role: Option<&str>,
    custom: Option<(i64, &str, &str)>,
    namespaces: Option<&str>,
) -> Result<Grant> {
    let namespaces = namespaces
        .map(serde_json::from_str::<BTreeSet<String>>)
        .transpose()
        .map_err(|e| Error::Internal(format!("bad namespaces in a grant: {e}")))?;
    match (role, custom) {
        (Some(role), _) => Ok(Grant::builtin(role.parse()?, namespaces)),
        (None, Some((id, name, perms))) => Ok(Grant::custom(
            CustomRef {
                id,
                name: name.to_string(),
            },
            crate::roles::db::decode_perms(perms)?,
            namespaces,
        )),
        (None, None) => Err(Error::Internal("a grant with no role".into())),
    }
}

impl BindingRow {
    fn into_binding(self) -> Result<TeamBinding> {
        Ok(TeamBinding {
            grant: grant(
                self.role.as_deref(),
                self.role_id
                    .zip(self.role_name.as_deref())
                    .zip(self.permissions.as_deref())
                    .map(|((id, n), p)| (id, n, p)),
                self.namespaces.as_deref(),
            )?,
            team_id: self.team_id,
            environment_id: self.environment_id,
            env_name: self.env_name,
            env_kind: self.env_kind,
            updated_at: self.updated_at,
        })
    }
}

const TEAM_COLS: &str = "id, name, description, created_at, updated_at";
const BINDING_SELECT: &str = "SELECT b.team_id, b.environment_id, e.name AS env_name,
        e.kind AS env_kind, b.role, b.role_id, b.role_name, b.permissions, b.namespaces, b.updated_at
    FROM ee_team_grants b
    JOIN environments e ON e.id = b.environment_id";

fn name_taken(name: &str) -> impl FnOnce(sqlx::Error) -> Error + '_ {
    move |e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict(format!("a team named '{name}' already exists"))
        }
        _ => db_err(e),
    }
}

pub async fn create(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    now: i64,
) -> Result<TeamRecord> {
    sqlx::query_as(AssertSqlSafe(format!(
        "INSERT INTO ee_teams (name, description, created_at, updated_at)
         VALUES (?, ?, ?, ?) RETURNING {TEAM_COLS}"
    )))
    .bind(name)
    .bind(description)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(name_taken(name))
}

/// Every team, by name, with its member and environment counts.
pub async fn list(pool: &SqlitePool) -> Result<Vec<(TeamRecord, i64, i64)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        team: TeamRecord,
        members: i64,
        envs: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, t.name, t.description, t.created_at, t.updated_at,
            (SELECT COUNT(*) FROM ee_team_members m WHERE m.team_id = t.id) AS members,
            (SELECT COUNT(*) FROM ee_team_grants b WHERE b.team_id = t.id) AS envs
         FROM ee_teams t ORDER BY t.name",
    )
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|r| (r.team, r.members, r.envs))
        .collect())
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<TeamRecord> {
    sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {TEAM_COLS} FROM ee_teams WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("team {id}")))
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    description: Option<&str>,
    now: i64,
) -> Result<TeamRecord> {
    sqlx::query_as(AssertSqlSafe(format!(
        "UPDATE ee_teams SET name = ?, description = ?, updated_at = ?
         WHERE id = ? RETURNING {TEAM_COLS}"
    )))
    .bind(name)
    .bind(description)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(name_taken(name))?
    .ok_or_else(|| Error::NotFound(format!("team {id}")))
}

/// Whether the team existed.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_teams WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

/// The team's members, by username.
pub async fn members(pool: &SqlitePool, team_id: i64) -> Result<Vec<User>> {
    let rows: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT u.id, u.username, u.role, u.created_at, u.updated_at
         FROM ee_team_members m JOIN users u ON u.id = m.user_id
         WHERE m.team_id = ? ORDER BY u.username",
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|(id, username, role, created_at, updated_at)| {
            Ok(User {
                id,
                username,
                role: role.parse::<Role>()?,
                created_at,
                updated_at,
                sso: None,
            })
        })
        .collect()
}

/// Whether the user was added (false: already a member). A membership that
/// single sign-on added becomes a manual one, which sync never removes.
pub async fn add_member(pool: &SqlitePool, team_id: i64, user_id: i64, now: i64) -> Result<bool> {
    let r = sqlx::query(
        "INSERT INTO ee_team_members (team_id, user_id, created_at) VALUES (?, ?, ?)
         ON CONFLICT DO UPDATE SET source_connection_id = NULL
             WHERE source_connection_id IS NOT NULL",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

/// Members added by single sign-on group sync, with the connection name.
pub async fn synced_members(pool: &SqlitePool, team_id: i64) -> Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT m.user_id, c.name FROM ee_team_members m
         JOIN ee_sso_connections c ON c.id = m.source_connection_id
         WHERE m.team_id = ? ORDER BY m.user_id",
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

/// Whether the user was a member.
pub async fn remove_member(pool: &SqlitePool, team_id: i64, user_id: i64) -> Result<bool> {
    let r = sqlx::query("DELETE FROM ee_team_members WHERE team_id = ? AND user_id = ?")
        .bind(team_id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(r.rows_affected() > 0)
}

/// The team's grants, by environment name.
pub async fn bindings(pool: &SqlitePool, team_id: i64) -> Result<Vec<TeamBinding>> {
    sqlx::query_as::<_, BindingRow>(AssertSqlSafe(format!(
        "{BINDING_SELECT} WHERE b.team_id = ? ORDER BY e.name"
    )))
    .bind(team_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(BindingRow::into_binding)
    .collect()
}

pub async fn binding(pool: &SqlitePool, team_id: i64, env_id: i64) -> Result<Option<TeamBinding>> {
    sqlx::query_as::<_, BindingRow>(AssertSqlSafe(format!(
        "{BINDING_SELECT} WHERE b.team_id = ? AND b.environment_id = ?"
    )))
    .bind(team_id)
    .bind(env_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .map(BindingRow::into_binding)
    .transpose()
}

pub async fn set_binding(
    pool: &SqlitePool,
    team_id: i64,
    env_id: i64,
    role: EnvRole,
    namespaces: Option<&BTreeSet<String>>,
    now: i64,
) -> Result<TeamBinding> {
    let ns = namespaces
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| Error::Internal(format!("encode namespaces: {e}")))?;
    sqlx::query(
        "INSERT INTO ee_team_bindings (team_id, environment_id, role, namespaces, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (team_id, environment_id) DO UPDATE SET role = excluded.role,
             namespaces = excluded.namespaces, updated_at = excluded.updated_at",
    )
    .bind(team_id)
    .bind(env_id)
    .bind(role.as_str())
    .bind(ns)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(db_err)?;
    sqlx::query("DELETE FROM ee_team_role_bindings WHERE team_id = ? AND environment_id = ?")
        .bind(team_id)
        .bind(env_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    binding(pool, team_id, env_id)
        .await?
        .ok_or_else(|| Error::Internal("team binding vanished".into()))
}

/// Grants custom role `role_id` to a team, replacing its grant there.
pub async fn set_custom_binding(
    pool: &SqlitePool,
    team_id: i64,
    env_id: i64,
    role_id: i64,
    namespaces: Option<&BTreeSet<String>>,
    now: i64,
) -> Result<TeamBinding> {
    crate::roles::db::get(pool, role_id).await?;
    let ns = namespaces
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| Error::Internal(format!("encode namespaces: {e}")))?;
    sqlx::query(
        "INSERT INTO ee_team_role_bindings (team_id, environment_id, role_id, namespaces, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (team_id, environment_id) DO UPDATE SET role_id = excluded.role_id,
             namespaces = excluded.namespaces, updated_at = excluded.updated_at",
    )
    .bind(team_id)
    .bind(env_id)
    .bind(role_id)
    .bind(ns)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(db_err)?;
    sqlx::query("DELETE FROM ee_team_bindings WHERE team_id = ? AND environment_id = ?")
        .bind(team_id)
        .bind(env_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    binding(pool, team_id, env_id)
        .await?
        .ok_or_else(|| Error::Internal("team binding vanished".into()))
}

/// Whether there was a grant to revoke.
pub async fn delete_binding(pool: &SqlitePool, team_id: i64, env_id: i64) -> Result<bool> {
    let mut n = 0;
    for table in ["ee_team_bindings", "ee_team_role_bindings"] {
        n += sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE team_id = ? AND environment_id = ?"
        )))
        .bind(team_id)
        .bind(env_id)
        .execute(pool)
        .await
        .map_err(db_err)?
        .rows_affected();
    }
    Ok(n > 0)
}

/// Every team with a grant on `env_id`, by name, with the grant.
pub async fn env_teams(pool: &SqlitePool, env_id: i64) -> Result<Vec<(TeamRecord, TeamBinding)>> {
    let bindings: Vec<TeamBinding> = sqlx::query_as::<_, BindingRow>(AssertSqlSafe(format!(
        "{BINDING_SELECT} WHERE b.environment_id = ?"
    )))
    .bind(env_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(BindingRow::into_binding)
    .collect::<Result<_>>()?;
    let mut out = Vec::new();
    for b in bindings {
        out.push((get(pool, b.team_id).await?, b));
    }
    out.sort_by_key(|(t, _)| t.name.to_lowercase());
    Ok(out)
}

/// The teams a user belongs to, by name.
pub async fn user_teams(pool: &SqlitePool, user_id: i64) -> Result<Vec<TeamRecord>> {
    sqlx::query_as(
        "SELECT t.id, t.name, t.description, t.created_at, t.updated_at
         FROM ee_team_members m JOIN ee_teams t ON t.id = m.team_id
         WHERE m.user_id = ? ORDER BY t.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)
}

/// Every grant a user has through their teams, with the team it comes from.
pub async fn user_team_bindings(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<(TeamRecord, TeamBinding)>> {
    let mut out = Vec::new();
    for team in user_teams(pool, user_id).await? {
        for b in bindings(pool, team.id).await? {
            out.push((team.clone(), b));
        }
    }
    Ok(out)
}

/// `(environment id, grant)` for each team grant reaching `user_id`, on one
/// environment or all of them, in one query.
pub(crate) async fn member_grants(
    pool: &SqlitePool,
    user_id: i64,
    env_id: Option<i64>,
) -> Result<Vec<(i64, Grant)>> {
    type Row = (
        i64,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT b.environment_id, b.role, b.role_id, b.role_name, b.permissions, b.namespaces
         FROM ee_team_members m JOIN ee_team_grants b ON b.team_id = m.team_id
         WHERE m.user_id = ?1 AND (?2 IS NULL OR b.environment_id = ?2)",
    )
    .bind(user_id)
    .bind(env_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|(env, role, id, name, perms, ns)| {
            let custom = id
                .zip(name.as_deref())
                .zip(perms.as_deref())
                .map(|((i, n), p)| (i, n, p));
            Ok((env, grant(role.as_deref(), custom, ns.as_deref())?))
        })
        .collect()
}
