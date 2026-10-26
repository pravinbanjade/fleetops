//! What a sign-in does to FleetOps (SSO spec, "Provisioning"): find or
//! create the user, then bring their sync-made team memberships and admin
//! role in line with their groups.

use super::{db as sso_db, db::Connection, db::Mapping, NO_PASSWORD};
use fleetops_api::{audit_detail, AppState};
use fleetops_core::{edition::Feature, now_unix, user::USERNAME_MAX, Error, Result, Role, User};
use serde_json::json;
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashSet};

/// Who the identity provider says signed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// Stable id at the provider; never the username or email.
    pub subject: String,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    /// GitHub login or LDAP username.
    pub login: Option<String>,
    pub groups: Vec<String>,
}

/// Why a sign-in is refused; each is a `sso_error` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Denied,
    NotAllowed,
    SeatLimit,
    Unlicensed,
    Disabled,
    /// The account exists, but an admin disabled it.
    AccountDisabled,
    Invalid,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Denied => "denied",
            Refusal::NotAllowed => "not_allowed",
            Refusal::SeatLimit => "seat_limit",
            Refusal::Unlicensed => "unlicensed",
            Refusal::Disabled => "disabled",
            Refusal::AccountDisabled => "account_disabled",
            Refusal::Invalid => "invalid",
        }
    }
}

/// A username from the claims: core's charset, at most 64 characters, and
/// suffixed `-2`, `-3`, … while `taken` says it is in use.
pub fn derive_username(c: &Claims, taken: impl Fn(&str) -> bool) -> String {
    let source = c
        .preferred_username
        .as_deref()
        .or(c.email.as_deref().map(|e| e.split('@').next().unwrap_or(e)))
        .or(c.login.as_deref())
        .unwrap_or("");
    let mut base: String = source
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    base.truncate(USERNAME_MAX);
    if base.is_empty() {
        base = "user".into();
    }
    if !taken(&base) {
        return base;
    }
    for n in 2.. {
        let suffix = format!("-{n}");
        let mut head = base.clone();
        head.truncate(USERNAME_MAX - suffix.len());
        let candidate = format!("{head}{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// What sync changes for one user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Teams to join as sync members.
    pub add: Vec<i64>,
    /// Sync memberships to drop.
    pub remove: Vec<i64>,
    pub admin: bool,
}

fn norm(g: &str) -> String {
    g.trim().to_lowercase()
}

/// Pure: the mappings that match `groups`, against the user's current
/// sync-made (`synced`) and manual (`manual`) team memberships.
pub fn plan(mappings: &[Mapping], groups: &[String], synced: &[i64], manual: &[i64]) -> Plan {
    let groups: HashSet<String> = groups.iter().map(|g| norm(g)).collect();
    let matching = mappings
        .iter()
        .filter(|m| groups.contains(&norm(&m.group_value)));
    let mut want = BTreeSet::new();
    let mut admin = false;
    for m in matching {
        match m.team_id {
            Some(t) => {
                want.insert(t);
            }
            None => admin |= m.is_admin,
        }
    }
    Plan {
        add: want
            .iter()
            .copied()
            .filter(|t| !synced.contains(t) && !manual.contains(t))
            .collect(),
        remove: synced
            .iter()
            .copied()
            .filter(|t| !want.contains(t))
            .collect(),
        admin,
    }
}

fn db_err(e: sqlx::Error) -> Error {
    sso_db::db_err(e)
}

/// (sync-made, manual) team ids of `user_id` for connection `conn_id`.
async fn memberships(
    pool: &SqlitePool,
    user_id: i64,
    conn_id: i64,
) -> Result<(Vec<i64>, Vec<i64>)> {
    let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT team_id, source_connection_id FROM ee_team_members WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    let synced = rows
        .iter()
        .filter(|r| r.1 == Some(conn_id))
        .map(|r| r.0)
        .collect();
    let manual = rows
        .iter()
        .filter(|r| r.1 != Some(conn_id))
        .map(|r| r.0)
        .collect();
    Ok((synced, manual))
}

/// Applies `p` to `user`: team rows, then the role. Signals and audits a
/// change. Returns the user as stored afterwards.
async fn apply(s: &AppState, conn: &Connection, user: User, p: &Plan) -> Result<User> {
    let pool = s.store.pool();
    let now = now_unix();
    for t in &p.add {
        sqlx::query(
            "INSERT INTO ee_team_members (team_id, user_id, created_at, source_connection_id)
             VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(t)
        .bind(user.id)
        .bind(now)
        .bind(conn.id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    }
    for t in &p.remove {
        sqlx::query(
            "DELETE FROM ee_team_members WHERE team_id = ? AND user_id = ? AND source_connection_id = ?",
        )
        .bind(t)
        .bind(user.id)
        .bind(conn.id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    }
    let want = if p.admin { Role::Admin } else { Role::User };
    let mut detail = json!({"connection": conn.name, "added": p.add, "removed": p.remove});
    let mut user = user;
    if user.role != want {
        match s.store.set_user_role(user.id, want, now).await {
            Ok(u) => {
                detail["role"] = json!(want.as_str());
                user = u;
            }
            Err(Error::Conflict(_)) => {
                tracing::warn!(user = user.id, "single sign-on kept the last admin");
                detail["kept_last_admin"] = json!(true);
            }
            Err(e) => return Err(e),
        }
    }
    if !p.add.is_empty() || !p.remove.is_empty() || detail.get("role").is_some() {
        s.hubs.access_changed(user.id);
    }
    if detail.get("role").is_some()
        || detail.get("kept_last_admin").is_some()
        || !p.add.is_empty()
        || !p.remove.is_empty()
    {
        audit_detail(
            s,
            Some(user.id),
            "sso.sync",
            "user",
            &user.id.to_string(),
            "success",
            Some(detail),
        )
        .await;
    }
    Ok(user)
}

/// Records a refused sign-in.
pub async fn refused(s: &AppState, conn: &Connection, why: Refusal, detail: &str) {
    tracing::info!(
        connection = conn.id,
        code = why.code(),
        detail,
        "single sign-on refused"
    );
    audit_detail(
        s,
        None,
        "sso.login",
        "sso_connection",
        &conn.id.to_string(),
        "failure",
        Some(json!({"connection": conn.name, "code": why.code(), "detail": detail})),
    )
    .await;
}

/// Finds or creates the user for `c`, then syncs them. A refusal is
/// audited here.
pub async fn login(
    s: &AppState,
    conn: &Connection,
    c: Claims,
) -> Result<std::result::Result<User, Refusal>> {
    let pool = s.store.pool();
    if !conn.enabled {
        refused(s, conn, Refusal::Disabled, "connection disabled").await;
        return Ok(Err(Refusal::Disabled));
    }
    let mappings = sso_db::mappings(pool, conn.id).await?;
    let groups_json = json!(c.groups).to_string();
    let now = now_unix();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT user_id FROM ee_sso_identities WHERE connection_id = ? AND subject = ?",
    )
    .bind(conn.id)
    .bind(&c.subject)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let user = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE ee_sso_identities SET email = ?, groups = ?, last_login_at = ? WHERE user_id = ?",
            )
            .bind(&c.email)
            .bind(&groups_json)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await
            .map_err(db_err)?;
            s.store.get_user(id).await?
        }
        None => {
            if !s.entitlements.has(Feature::Sso) {
                refused(s, conn, Refusal::Unlicensed, "new users need a license").await;
                return Ok(Err(Refusal::Unlicensed));
            }
            if let Some(max) = s.entitlements.max_users() {
                if s.store.count_users().await? >= i64::from(max) {
                    refused(s, conn, Refusal::SeatLimit, "user limit reached").await;
                    return Ok(Err(Refusal::SeatLimit));
                }
            }
            let first = plan(&mappings, &c.groups, &[], &[]);
            if conn.require_mapped_group && first.add.is_empty() && !first.admin {
                refused(s, conn, Refusal::Denied, "no mapped group").await;
                return Ok(Err(Refusal::Denied));
            }
            let taken: HashSet<String> = s
                .store
                .list_users()
                .await?
                .into_iter()
                .map(|u| u.username.to_lowercase())
                .collect();
            let username = derive_username(&c, |n| taken.contains(&n.to_lowercase()));
            let role = if first.admin { Role::Admin } else { Role::User };
            let user = s
                .store
                .create_user(&username, NO_PASSWORD, role, now)
                .await?;
            let linked = sqlx::query(
                "INSERT INTO ee_sso_identities (user_id, connection_id, subject, email, groups, last_login_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user.id)
            .bind(conn.id)
            .bind(&c.subject)
            .bind(&c.email)
            .bind(&groups_json)
            .bind(now)
            .execute(pool)
            .await;
            if let Err(e) = linked {
                // Another sign-in for the same subject won the race.
                let _ = s.store.delete_user(user.id).await;
                return Err(db_err(e));
            }
            audit_detail(
                s,
                Some(user.id),
                "sso.user_create",
                "user",
                &user.id.to_string(),
                "success",
                Some(json!({"connection": conn.name, "username": user.username})),
            )
            .await;
            user
        }
    };
    let (synced, manual) = memberships(pool, user.id, conn.id).await?;
    let p = plan(&mappings, &c.groups, &synced, &manual);
    let user = apply(s, conn, user, &p).await?;
    audit_detail(
        s,
        Some(user.id),
        "sso.login",
        "sso_connection",
        &conn.id.to_string(),
        "success",
        Some(json!({"connection": conn.name})),
    )
    .await;
    Ok(Ok(user))
}

/// Re-applies every user's stored groups after a mapping changed.
pub async fn resync_connection(s: &AppState, conn_id: i64) -> Result<()> {
    let pool = s.store.pool();
    let conn = sso_db::get(pool, conn_id).await?;
    let mappings = sso_db::mappings(pool, conn_id).await?;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT user_id, groups FROM ee_sso_identities WHERE connection_id = ?")
            .bind(conn_id)
            .fetch_all(pool)
            .await
            .map_err(db_err)?;
    for (user_id, groups) in rows {
        let groups: Vec<String> = serde_json::from_str(&groups).unwrap_or_default();
        let (synced, manual) = memberships(pool, user_id, conn_id).await?;
        let p = plan(&mappings, &groups, &synced, &manual);
        let user = s.store.get_user(user_id).await?;
        apply(s, &conn, user, &p).await?;
    }
    Ok(())
}

/// Turns a connection's effect off: drops its sync memberships, demotes the
/// admins it made (never the last admin), and ends its users' sessions.
pub async fn detach_connection(s: &AppState, conn_id: i64) -> Result<()> {
    let pool = s.store.pool();
    let conn = sso_db::get(pool, conn_id).await?;
    for user_id in sso_db::connection_users(pool, conn_id).await? {
        let (synced, _) = memberships(pool, user_id, conn_id).await?;
        let p = Plan {
            add: vec![],
            remove: synced,
            admin: false,
        };
        let user = s.store.get_user(user_id).await?;
        apply(s, &conn, user, &p).await?;
        s.store.delete_user_sessions(user_id, None).await?;
        s.hubs.access_changed(user_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(pref: Option<&str>, email: Option<&str>, login: Option<&str>) -> Claims {
        Claims {
            subject: "s".into(),
            preferred_username: pref.map(Into::into),
            email: email.map(Into::into),
            login: login.map(Into::into),
            groups: vec![],
        }
    }

    fn m(id: i64, group: &str, team: Option<i64>) -> Mapping {
        Mapping {
            id,
            connection_id: 1,
            group_value: group.into(),
            team_id: team,
            team_name: None,
            is_admin: team.is_none(),
        }
    }

    #[test]
    fn derive_username_sanitises_and_suffixes() {
        let none = |_: &str| false;
        assert_eq!(
            derive_username(&claims(Some("Jane Doe@x"), None, None), none),
            "Jane_Doe_x"
        );
        assert_eq!(
            derive_username(&claims(None, Some("jane@acme.io"), None), none),
            "jane"
        );
        assert_eq!(
            derive_username(&claims(None, None, Some("octo")), none),
            "octo"
        );
        assert_eq!(derive_username(&claims(None, None, None), none), "user");
        assert_eq!(
            derive_username(&claims(Some("jane"), None, None), |n| n == "jane"),
            "jane-2"
        );
        assert_eq!(
            derive_username(&claims(Some("jane"), None, None), |n| n == "jane"
                || n == "jane-2"),
            "jane-3"
        );
        let long = "a".repeat(80);
        let full = "a".repeat(64);
        let got = derive_username(&claims(Some(&long), None, None), |n| n == full);
        assert!(got.len() <= 64 && got.ends_with("-2"), "{got}");
    }

    #[test]
    fn plan_adds_mapped_and_removes_unmapped_sync_rows() {
        let maps = [m(1, "ops", Some(10)), m(2, "dev", Some(20))];
        let p = plan(&maps, &["ops".into()], &[20, 30], &[]);
        assert_eq!(p.add, vec![10]);
        assert_eq!(p.remove, vec![20, 30]);
        assert!(!p.admin);
    }

    #[test]
    fn plan_never_adds_over_a_manual_row() {
        let maps = [m(1, "ops", Some(10))];
        let p = plan(&maps, &["ops".into()], &[], &[10]);
        assert!(p.add.is_empty() && p.remove.is_empty());
    }

    #[test]
    fn plan_group_match_ignores_case_and_space() {
        let maps = [m(1, " Platform-Ops ", Some(10))];
        let p = plan(&maps, &["platform-ops".into()], &[], &[]);
        assert_eq!(p.add, vec![10]);
    }

    #[test]
    fn plan_admin_from_any_admin_mapping() {
        let maps = [m(1, "ops", Some(10)), m(2, "fleet-admins", None)];
        assert!(plan(&maps, &["FLEET-ADMINS".into()], &[], &[]).admin);
        assert!(!plan(&maps, &["ops".into()], &[], &[]).admin);
    }
}
