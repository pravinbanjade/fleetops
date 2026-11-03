//! The notification tables.

use super::model::{
    Destination, DestinationKind, DestinationSecrets, Message, PendingGroup, PendingItem,
    PendingKind, Rule, SmtpConfig, SmtpSecurity,
};
use crate::scan::db::PasswordChange;
use crate::scan::model::Severity;
use fleetops_core::{Error, Result};
use fleetops_store::Store;
use serde::{Deserialize, Serialize};
use sqlx::Row;

fn db_err(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict("that name is already taken".into())
        }
        _ => Error::Internal(format!("database: {e}")),
    }
}

fn json_err(e: serde_json::Error) -> Error {
    Error::Internal(format!("notifications: {e}"))
}

/// Grouping: a batch is sent once nothing was added for this long…
pub const QUIET_SECS: i64 = 120;
/// …or this long after its first item, whichever comes first.
pub const MAX_WAIT_SECS: i64 = 600;

fn dest_aad(id: i64) -> Vec<u8> {
    format!("notify-dest:{id}").into_bytes()
}

const SMTP_AAD: &[u8] = b"notify-smtp:1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DestSettings {
    host: Option<String>,
    #[serde(default)]
    recipients: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NewDestination {
    pub name: String,
    pub kind: DestinationKind,
    pub host: Option<String>,
    pub recipients: Vec<String>,
    pub secrets: DestinationSecrets,
}

fn seal(store: &Store, id: i64, s: &DestinationSecrets) -> Result<String> {
    let plain = serde_json::to_vec(s).map_err(json_err)?;
    Ok(store.secrets().seal(&dest_aad(id), &plain))
}

pub async fn create_destination(store: &Store, d: &NewDestination, now: i64) -> Result<i64> {
    let settings = serde_json::to_string(&DestSettings {
        host: d.host.clone(),
        recipients: d.recipients.clone(),
    })
    .map_err(json_err)?;
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let id = sqlx::query(
        "INSERT INTO ee_notify_destinations (name, kind, settings, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&d.name)
    .bind(d.kind.as_str())
    .bind(settings)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?
    .last_insert_rowid();
    sqlx::query("UPDATE ee_notify_destinations SET secrets = ? WHERE id = ?")
        .bind(seal(store, id, &d.secrets)?)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(id)
}

fn destination_from(r: &sqlx::sqlite::SqliteRow) -> Result<Destination> {
    let settings: DestSettings =
        serde_json::from_str(r.get::<String, _>("settings").as_str()).unwrap_or_default();
    Ok(Destination {
        id: r.get("id"),
        name: r.get("name"),
        kind: DestinationKind::parse(r.get::<String, _>("kind").as_str())
            .ok_or_else(|| Error::Internal("unknown destination kind".into()))?,
        host: settings.host,
        recipients: settings.recipients,
        last_error: r.get("last_error"),
        last_error_at: r.get("last_error_at"),
    })
}

const DEST_COLS: &str = "id, name, kind, settings, secrets, last_error, last_error_at";

pub async fn list_destinations(store: &Store) -> Result<Vec<Destination>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DEST_COLS} FROM ee_notify_destinations ORDER BY name"
    )))
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?
    .iter()
    .map(destination_from)
    .collect()
}

/// A destination with its secrets opened.
pub async fn get_destination(store: &Store, id: i64) -> Result<(Destination, DestinationSecrets)> {
    let r = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DEST_COLS} FROM ee_notify_destinations WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("destination {id}")))?;
    let secrets = match r.get::<Option<String>, _>("secrets") {
        None => DestinationSecrets::default(),
        Some(sealed) => {
            let plain = store.secrets().open(&dest_aad(id), &sealed).map_err(|_| {
                Error::Invalid(
                    "this destination's saved secret can't be read; enter it again".into(),
                )
            })?;
            serde_json::from_slice(&plain).map_err(json_err)?
        }
    };
    Ok((destination_from(&r)?, secrets))
}

/// `secrets: None` keeps the stored ones.
pub async fn update_destination(
    store: &Store,
    id: i64,
    name: &str,
    host: Option<&str>,
    recipients: &[String],
    secrets: Option<&DestinationSecrets>,
    now: i64,
) -> Result<()> {
    let settings = serde_json::to_string(&DestSettings {
        host: host.map(String::from),
        recipients: recipients.to_vec(),
    })
    .map_err(json_err)?;
    let r = sqlx::query(
        "UPDATE ee_notify_destinations SET name = ?, settings = ?, updated_at = ? WHERE id = ?",
    )
    .bind(name)
    .bind(settings)
    .bind(now)
    .bind(id)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    if r.rows_affected() == 0 {
        return Err(Error::NotFound(format!("destination {id}")));
    }
    if let Some(s) = secrets {
        sqlx::query("UPDATE ee_notify_destinations SET secrets = ? WHERE id = ?")
            .bind(seal(store, id, s)?)
            .bind(id)
            .execute(store.pool())
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

pub async fn delete_destination(store: &Store, id: i64) -> Result<()> {
    let r = sqlx::query("DELETE FROM ee_notify_destinations WHERE id = ?")
        .bind(id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    if r.rows_affected() == 0 {
        return Err(Error::NotFound(format!("destination {id}")));
    }
    Ok(())
}

pub async fn set_destination_error(
    store: &Store,
    id: i64,
    error: Option<&str>,
    now: i64,
) -> Result<()> {
    sqlx::query("UPDATE ee_notify_destinations SET last_error = ?, last_error_at = ? WHERE id = ?")
        .bind(error)
        .bind(error.map(|_| now))
        .bind(id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// A rule as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    pub name: String,
    pub enabled: bool,
    pub envs: Vec<i64>,
    pub min_severity: Severity,
    pub fixable_only: bool,
    pub running_only: bool,
    pub destinations: Vec<i64>,
}

async fn set_rule_destinations(
    tx: &mut sqlx::SqliteConnection,
    rule: i64,
    dests: &[i64],
) -> Result<()> {
    sqlx::query("DELETE FROM ee_notify_rule_destinations WHERE rule_id = ?")
        .bind(rule)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    for d in dests {
        sqlx::query("INSERT OR IGNORE INTO ee_notify_rule_destinations (rule_id, destination_id) VALUES (?, ?)")
            .bind(rule)
            .bind(d)
            .execute(&mut *tx)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(x) if x.is_foreign_key_violation() => {
                    Error::Invalid(format!("no destination {d}"))
                }
                _ => db_err(e),
            })?;
    }
    Ok(())
}

pub async fn create_rule(store: &Store, r: &RuleRow, now: i64) -> Result<i64> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let id = sqlx::query(
        "INSERT INTO ee_notify_rules (name, enabled, envs, min_severity, fixable_only, running_only,
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&r.name)
    .bind(r.enabled)
    .bind(serde_json::to_string(&r.envs).map_err(json_err)?)
    .bind(r.min_severity.as_str())
    .bind(r.fixable_only)
    .bind(r.running_only)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?
    .last_insert_rowid();
    set_rule_destinations(&mut tx, id, &r.destinations).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(id)
}

pub async fn update_rule(store: &Store, id: i64, r: &RuleRow, now: i64) -> Result<()> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let n = sqlx::query(
        "UPDATE ee_notify_rules SET name = ?, enabled = ?, envs = ?, min_severity = ?,
             fixable_only = ?, running_only = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&r.name)
    .bind(r.enabled)
    .bind(serde_json::to_string(&r.envs).map_err(json_err)?)
    .bind(r.min_severity.as_str())
    .bind(r.fixable_only)
    .bind(r.running_only)
    .bind(now)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?
    .rows_affected();
    if n == 0 {
        return Err(Error::NotFound(format!("rule {id}")));
    }
    set_rule_destinations(&mut tx, id, &r.destinations).await?;
    tx.commit().await.map_err(db_err)
}

pub async fn delete_rule(store: &Store, id: i64) -> Result<()> {
    let r = sqlx::query("DELETE FROM ee_notify_rules WHERE id = ?")
        .bind(id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    if r.rows_affected() == 0 {
        return Err(Error::NotFound(format!("rule {id}")));
    }
    Ok(())
}

pub async fn list_rules(store: &Store) -> Result<Vec<Rule>> {
    let rows = sqlx::query(
        "SELECT id, name, enabled, envs, min_severity, fixable_only, running_only
         FROM ee_notify_rules ORDER BY name",
    )
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    let links: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rule_id, destination_id FROM ee_notify_rule_destinations ORDER BY destination_id",
    )
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    Ok(rows
        .iter()
        .map(|r| {
            let id: i64 = r.get("id");
            Rule {
                id,
                name: r.get("name"),
                enabled: r.get("enabled"),
                envs: serde_json::from_str(r.get::<String, _>("envs").as_str()).unwrap_or_default(),
                min_severity: Severity::parse(r.get::<String, _>("min_severity").as_str()),
                fixable_only: r.get("fixable_only"),
                running_only: r.get("running_only"),
                destinations: links
                    .iter()
                    .filter(|(rid, _)| *rid == id)
                    .map(|(_, d)| *d)
                    .collect(),
            }
        })
        .collect())
}

pub async fn get_rule(store: &Store, id: i64) -> Result<Option<Rule>> {
    Ok(list_rules(store).await?.into_iter().find(|r| r.id == id))
}

/// SMTP settings as written; the password goes separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpRow {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub from_address: String,
}

fn security_str(s: SmtpSecurity) -> &'static str {
    match s {
        SmtpSecurity::Starttls => "starttls",
        SmtpSecurity::Tls => "tls",
        SmtpSecurity::None => "none",
    }
}

pub async fn get_smtp(store: &Store) -> Result<Option<SmtpConfig>> {
    let Some(r) = sqlx::query(
        "SELECT host, port, security, username, password, from_address FROM ee_notify_smtp WHERE id = 1",
    )
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)?
    else {
        return Ok(None);
    };
    let password = match r.get::<Option<String>, _>("password") {
        None => None,
        Some(sealed) => Some(
            String::from_utf8_lossy(&store.secrets().open(SMTP_AAD, &sealed).map_err(|_| {
                Error::Invalid("the saved SMTP password can't be read; enter it again".into())
            })?)
            .into_owned(),
        ),
    };
    Ok(Some(SmtpConfig {
        host: r.get("host"),
        port: r.get::<i64, _>("port") as u16,
        security: match r.get::<String, _>("security").as_str() {
            "tls" => SmtpSecurity::Tls,
            "none" => SmtpSecurity::None,
            _ => SmtpSecurity::Starttls,
        },
        username: r.get("username"),
        password,
        from_address: r.get("from_address"),
    }))
}

pub async fn put_smtp(store: &Store, s: &SmtpRow, pw: PasswordChange, now: i64) -> Result<()> {
    let current: Option<Option<String>> =
        sqlx::query_scalar("SELECT password FROM ee_notify_smtp WHERE id = 1")
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    let sealed = match pw {
        PasswordChange::Keep => current.flatten(),
        PasswordChange::Clear => None,
        PasswordChange::Set(p) => Some(store.secrets().seal(SMTP_AAD, p.as_bytes())),
    };
    sqlx::query(
        "INSERT INTO ee_notify_smtp (id, host, port, security, username, password, from_address, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET host = excluded.host, port = excluded.port,
             security = excluded.security, username = excluded.username,
             password = excluded.password, from_address = excluded.from_address,
             updated_at = excluded.updated_at",
    )
    .bind(&s.host)
    .bind(i64::from(s.port))
    .bind(security_str(s.security))
    .bind(&s.username)
    .bind(sealed)
    .bind(&s.from_address)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

/// `None`: the rule doesn't know the repository; `Some(true)`: baseline pending.
pub async fn known(store: &Store, rule: i64, repo: &str) -> Result<Option<bool>> {
    sqlx::query_scalar(
        "SELECT baseline_pending FROM ee_notify_known WHERE rule_id = ? AND repository = ?",
    )
    .bind(rule)
    .bind(repo)
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)
}

pub async fn set_known(store: &Store, rule: i64, repo: &str, baseline_pending: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO ee_notify_known (rule_id, repository, baseline_pending) VALUES (?, ?, ?)
         ON CONFLICT (rule_id, repository) DO UPDATE SET baseline_pending = excluded.baseline_pending",
    )
    .bind(rule)
    .bind(repo)
    .bind(baseline_pending)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn is_reported(store: &Store, rule: i64, repo: &str, cve: &str) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ee_notify_reported WHERE rule_id = ? AND repository = ? AND cve = ?",
    )
    .bind(rule)
    .bind(repo)
    .bind(cve)
    .fetch_one(store.pool())
    .await
    .map_err(db_err)?;
    Ok(n > 0)
}

pub async fn mark_reported(
    store: &Store,
    rule: i64,
    repo: &str,
    cves: &[String],
    now: i64,
) -> Result<()> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    for c in cves {
        sqlx::query(
            "INSERT OR IGNORE INTO ee_notify_reported (rule_id, repository, cve, reported_at) VALUES (?, ?, ?, ?)",
        )
        .bind(rule)
        .bind(repo)
        .bind(c)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)
}

/// Adds `items` to a batch, keeping its first time and moving its last.
pub async fn add_pending(
    store: &Store,
    rule: i64,
    kind: PendingKind,
    key: &str,
    items: &[PendingItem],
    now: i64,
) -> Result<()> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let cur: Option<String> = sqlx::query_scalar(
        "SELECT items FROM ee_notify_pending WHERE rule_id = ? AND kind = ? AND group_key = ?",
    )
    .bind(rule)
    .bind(kind.as_str())
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    let mut all: Vec<PendingItem> = cur
        .map(|c| serde_json::from_str(&c).unwrap_or_default())
        .unwrap_or_default();
    for i in items {
        if !all.contains(i) {
            all.push(i.clone());
        }
    }
    sqlx::query(
        "INSERT INTO ee_notify_pending (rule_id, kind, group_key, items, first_at, last_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (rule_id, kind, group_key) DO UPDATE SET items = excluded.items,
             last_at = excluded.last_at",
    )
    .bind(rule)
    .bind(kind.as_str())
    .bind(key)
    .bind(serde_json::to_string(&all).map_err(json_err)?)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)
}

/// Batches quiet for [`QUIET_SECS`] or older than [`MAX_WAIT_SECS`].
pub async fn due_pending(store: &Store, now: i64) -> Result<Vec<PendingGroup>> {
    let rows = sqlx::query(
        "SELECT rule_id, kind, group_key, items, first_at, last_at FROM ee_notify_pending
         WHERE ? - last_at >= ? OR ? - first_at >= ? ORDER BY first_at",
    )
    .bind(now)
    .bind(QUIET_SECS)
    .bind(now)
    .bind(MAX_WAIT_SECS)
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    Ok(rows
        .iter()
        .map(|r| PendingGroup {
            rule_id: r.get("rule_id"),
            kind: if r.get::<String, _>("kind") == "image" {
                PendingKind::Image
            } else {
                PendingKind::Cve
            },
            group_key: r.get("group_key"),
            items: serde_json::from_str(r.get::<String, _>("items").as_str()).unwrap_or_default(),
            first_at: r.get("first_at"),
            last_at: r.get("last_at"),
        })
        .collect())
}

pub async fn delete_pending(store: &Store, rule: i64, kind: PendingKind, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM ee_notify_pending WHERE rule_id = ? AND kind = ? AND group_key = ?")
        .bind(rule)
        .bind(kind.as_str())
        .bind(key)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    Queued,
    Sent,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryRow {
    pub id: i64,
    pub destination_id: Option<i64>,
    pub rule_id: Option<i64>,
    pub title: String,
    pub message: Message,
    pub status: DeliveryStatus,
    pub attempts: i64,
    pub next_at: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub sent_at: Option<i64>,
}

fn delivery_from(r: &sqlx::sqlite::SqliteRow) -> Result<DeliveryRow> {
    Ok(DeliveryRow {
        id: r.get("id"),
        destination_id: r.get("destination_id"),
        rule_id: r.get("rule_id"),
        title: r.get("title"),
        message: serde_json::from_str(r.get::<String, _>("message").as_str()).map_err(json_err)?,
        status: match r.get::<String, _>("status").as_str() {
            "sent" => DeliveryStatus::Sent,
            "failed" => DeliveryStatus::Failed,
            _ => DeliveryStatus::Queued,
        },
        attempts: r.get("attempts"),
        next_at: r.get("next_at"),
        error: r.get("error"),
        created_at: r.get("created_at"),
        sent_at: r.get("sent_at"),
    })
}

const DELIVERY_COLS: &str =
    "id, destination_id, rule_id, title, message, status, attempts, next_at, error, created_at, sent_at";

pub async fn enqueue_delivery(
    store: &Store,
    dest: i64,
    rule: Option<i64>,
    m: &Message,
    now: i64,
) -> Result<i64> {
    Ok(sqlx::query(
        "INSERT INTO ee_notify_deliveries (destination_id, rule_id, title, message, status, next_at, created_at)
         VALUES (?, ?, ?, ?, 'queued', ?, ?)",
    )
    .bind(dest)
    .bind(rule)
    .bind(m.title())
    .bind(serde_json::to_string(m).map_err(json_err)?)
    .bind(now)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?
    .last_insert_rowid())
}

/// Queued deliveries whose time has come.
pub async fn due_deliveries(store: &Store, now: i64, limit: i64) -> Result<Vec<DeliveryRow>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DELIVERY_COLS} FROM ee_notify_deliveries WHERE status = 'queued' AND next_at <= ?
         ORDER BY next_at, id LIMIT ?"
    )))
    .bind(now)
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?
    .iter()
    .map(delivery_from)
    .collect()
}

pub async fn delivery_sent(store: &Store, id: i64, now: i64) -> Result<()> {
    sqlx::query(
        "UPDATE ee_notify_deliveries SET status = 'sent', sent_at = ?, error = NULL,
             attempts = attempts + 1 WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn delivery_retry(store: &Store, id: i64, error: &str, next_at: i64) -> Result<()> {
    sqlx::query(
        "UPDATE ee_notify_deliveries SET error = ?, next_at = ?, attempts = attempts + 1 WHERE id = ?",
    )
    .bind(error)
    .bind(next_at)
    .bind(id)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn delivery_failed(store: &Store, id: i64, error: &str, _now: i64) -> Result<()> {
    sqlx::query(
        "UPDATE ee_notify_deliveries SET status = 'failed', error = ?, attempts = attempts + 1 WHERE id = ?",
    )
    .bind(error)
    .bind(id)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn recent_deliveries(store: &Store, limit: i64) -> Result<Vec<DeliveryRow>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DELIVERY_COLS} FROM ee_notify_deliveries ORDER BY id DESC LIMIT ?"
    )))
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?
    .iter()
    .map(delivery_from)
    .collect()
}

/// Forgets finished deliveries older than `before`.
pub async fn prune_deliveries(store: &Store, before: i64) -> Result<()> {
    sqlx::query("DELETE FROM ee_notify_deliveries WHERE status != 'queued' AND created_at < ?")
        .bind(before)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Turns one due batch into deliveries, in one transaction: queues `message`
/// for each of `destinations`, records each (repository, CVE) of the batch as
/// reported, marks image batches' repositories known, and drops the batch.
pub async fn flush_group(
    store: &Store,
    group: &PendingGroup,
    message: &Message,
    destinations: &[i64],
    now: i64,
) -> Result<()> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let body = serde_json::to_string(message).map_err(json_err)?;
    for d in destinations {
        sqlx::query(
            "INSERT INTO ee_notify_deliveries (destination_id, rule_id, title, message, status, next_at, created_at)
             VALUES (?, ?, ?, ?, 'queued', ?, ?)",
        )
        .bind(d)
        .bind(group.rule_id)
        .bind(message.title())
        .bind(&body)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    for i in &group.items {
        sqlx::query(
            "INSERT OR IGNORE INTO ee_notify_reported (rule_id, repository, cve, reported_at) VALUES (?, ?, ?, ?)",
        )
        .bind(group.rule_id)
        .bind(&i.repository)
        .bind(&i.cve)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if group.kind == PendingKind::Image {
            sqlx::query(
                "INSERT INTO ee_notify_known (rule_id, repository, baseline_pending) VALUES (?, ?, 0)
                 ON CONFLICT (rule_id, repository) DO UPDATE SET baseline_pending = 0",
            )
            .bind(group.rule_id)
            .bind(&i.repository)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
    }
    sqlx::query("DELETE FROM ee_notify_pending WHERE rule_id = ? AND kind = ? AND group_key = ?")
        .bind(group.rule_id)
        .bind(group.kind.as_str())
        .bind(&group.group_key)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)
}

pub async fn add_baseline_key(store: &Store, rule: i64, repo: &str, key: &str) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO ee_notify_baseline_keys (rule_id, repository, image_key) VALUES (?, ?, ?)")
        .bind(rule)
        .bind(repo)
        .bind(key)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Closes `key`'s part of the baseline; how many digests remain open.
pub async fn remove_baseline_key(store: &Store, rule: i64, repo: &str, key: &str) -> Result<i64> {
    sqlx::query("DELETE FROM ee_notify_baseline_keys WHERE rule_id = ? AND repository = ? AND image_key = ?")
        .bind(rule)
        .bind(repo)
        .bind(key)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM ee_notify_baseline_keys WHERE rule_id = ? AND repository = ?",
    )
    .bind(rule)
    .bind(repo)
    .fetch_one(store.pool())
    .await
    .map_err(db_err)
}

pub async fn env_baselined(store: &Store, env_id: i64) -> Result<bool> {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ee_notify_env_baselined WHERE env_id = ?")
            .bind(env_id)
            .fetch_one(store.pool())
            .await
            .map_err(db_err)?;
    Ok(n > 0)
}

pub async fn set_env_baselined(store: &Store, env_id: i64, on: bool) -> Result<()> {
    let q = if on {
        "INSERT OR IGNORE INTO ee_notify_env_baselined (env_id) VALUES (?)"
    } else {
        "DELETE FROM ee_notify_env_baselined WHERE env_id = ?"
    };
    sqlx::query(q)
        .bind(env_id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Sets every repository `rule` knows back to baseline-pending.
pub async fn rebaseline_rule(store: &Store, rule: i64) -> Result<()> {
    sqlx::query("UPDATE ee_notify_known SET baseline_pending = 1 WHERE rule_id = ?")
        .bind(rule)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Rules whose only destination is `dest`.
pub async fn rules_only_sending_to(store: &Store, dest: i64) -> Result<Vec<String>> {
    Ok(list_rules(store)
        .await?
        .into_iter()
        .filter(|r| r.destinations == [dest])
        .map(|r| r.name)
        .collect())
}
