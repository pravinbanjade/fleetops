//! The scanning tables: settings, scans and findings, and image uses.

use super::config::{EnvOverride, GlobalConfig, Mirror};
use super::model::{counts, Counts, Finding, ImageKey, ImageUse, ScanTarget, Severity, Trigger};
use fleetops_core::{Error, Result};
use fleetops_store::Store;
use sqlx::Row;
use std::collections::{HashMap, HashSet};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(format!("database: {e}"))
}

/// How many scan rows each image keeps.
pub const HISTORY: i64 = 20;

/// What a settings write does to the stored mirror password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordChange {
    Keep,
    Set(String),
    Clear,
}

fn global_aad() -> Vec<u8> {
    b"scan-global:1".to_vec()
}

fn env_aad(env_id: i64) -> Vec<u8> {
    format!("scan-env:{env_id}").into_bytes()
}

fn open(store: &Store, aad: &[u8], sealed: Option<String>) -> Result<Option<String>> {
    sealed
        .map(|s| {
            store
                .secrets()
                .open(aad, &s)
                .map(|p| String::from_utf8_lossy(&p).into_owned())
                .map_err(|_| {
                    Error::Invalid("the saved mirror password can't be read; set it again".into())
                })
        })
        .transpose()
}

fn mirror_json(m: &Option<Mirror>) -> Result<Option<String>> {
    m.as_ref()
        .map(|m| serde_json::to_string(m).map_err(|e| Error::Internal(e.to_string())))
        .transpose()
}

fn mirror_from(s: Option<String>) -> Result<Option<Mirror>> {
    s.map(|s| serde_json::from_str(&s).map_err(|e| Error::Internal(format!("scan mirror: {e}"))))
        .transpose()
}

/// The sealed column after `change`, given what is stored now.
fn sealed_after(
    store: &Store,
    aad: &[u8],
    now: Option<String>,
    change: PasswordChange,
) -> Option<String> {
    match change {
        PasswordChange::Keep => now,
        PasswordChange::Clear => None,
        PasswordChange::Set(p) => Some(store.secrets().seal(aad, p.as_bytes())),
    }
}

pub async fn get_global(store: &Store) -> Result<(GlobalConfig, Option<String>)> {
    let row: Option<(bool, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT enabled, mirror, mirror_secret FROM ee_scan_global WHERE id = 1")
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    let Some((enabled, mirror, secret)) = row else {
        return Ok((GlobalConfig::default(), None));
    };
    Ok((
        GlobalConfig {
            enabled,
            mirror: mirror_from(mirror)?,
        },
        open(store, &global_aad(), secret)?,
    ))
}

pub async fn put_global(
    store: &Store,
    g: &GlobalConfig,
    pw: PasswordChange,
    now: i64,
) -> Result<()> {
    let current: Option<Option<String>> =
        sqlx::query_scalar("SELECT mirror_secret FROM ee_scan_global WHERE id = 1")
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    let sealed = sealed_after(store, &global_aad(), current.flatten(), pw);
    sqlx::query(
        "INSERT INTO ee_scan_global (id, enabled, mirror, mirror_secret, updated_at)
         VALUES (1, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, mirror = excluded.mirror,
             mirror_secret = excluded.mirror_secret, updated_at = excluded.updated_at",
    )
    .bind(g.enabled)
    .bind(mirror_json(&g.mirror)?)
    .bind(sealed)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

type EnvRow = (
    i64,
    Option<bool>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn env_from(store: &Store, r: EnvRow) -> Result<(i64, EnvOverride, Option<String>)> {
    let (id, enabled, mirror, secret, socket_path) = r;
    Ok((
        id,
        EnvOverride {
            enabled,
            mirror: mirror_from(mirror)?,
            socket_path,
        },
        open(store, &env_aad(id), secret)?,
    ))
}

const ENV_COLS: &str = "env_id, enabled, mirror, mirror_secret, socket_path";

pub async fn get_env(store: &Store, env_id: i64) -> Result<Option<(EnvOverride, Option<String>)>> {
    let row: Option<EnvRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ENV_COLS} FROM ee_scan_env WHERE env_id = ?"
    )))
    .bind(env_id)
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)?;
    row.map(|r| env_from(store, r).map(|(_, o, p)| (o, p)))
        .transpose()
}

pub async fn list_envs(store: &Store) -> Result<Vec<(i64, EnvOverride, Option<String>)>> {
    let rows: Vec<EnvRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ENV_COLS} FROM ee_scan_env ORDER BY env_id"
    )))
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    rows.into_iter().map(|r| env_from(store, r)).collect()
}

pub async fn put_env(
    store: &Store,
    env_id: i64,
    o: &EnvOverride,
    pw: PasswordChange,
    now: i64,
) -> Result<()> {
    let current: Option<Option<String>> =
        sqlx::query_scalar("SELECT mirror_secret FROM ee_scan_env WHERE env_id = ?")
            .bind(env_id)
            .fetch_optional(store.pool())
            .await
            .map_err(db_err)?;
    let sealed = sealed_after(store, &env_aad(env_id), current.flatten(), pw);
    sqlx::query(
        "INSERT INTO ee_scan_env (env_id, enabled, mirror, mirror_secret, socket_path, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (env_id) DO UPDATE SET enabled = excluded.enabled, mirror = excluded.mirror,
             mirror_secret = excluded.mirror_secret, socket_path = excluded.socket_path,
             updated_at = excluded.updated_at",
    )
    .bind(env_id)
    .bind(o.enabled)
    .bind(mirror_json(&o.mirror)?)
    .bind(sealed)
    .bind(&o.socket_path)
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

fn json_list(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn list_from(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

pub async fn replace_uses(store: &Store, env_id: i64, uses: &[ImageUse]) -> Result<()> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    sqlx::query("DELETE FROM ee_scan_uses WHERE env_id = ?")
        .bind(env_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    for u in uses {
        sqlx::query(
            "INSERT INTO ee_scan_uses (env_id, node, image_key, image_ref, image_id, namespace,
                 workload, container, running, pull_secrets) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(env_id)
        .bind(&u.node)
        .bind(u.key.as_str())
        .bind(&u.image_ref)
        .bind(&u.image_id)
        .bind(&u.namespace)
        .bind(&u.workload)
        .bind(&u.container)
        .bind(u.running)
        .bind(json_list(&u.pull_secrets))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)
}

pub async fn uses_for_envs(store: &Store, env_ids: &[i64]) -> Result<Vec<ImageUse>> {
    if env_ids.is_empty() {
        return Ok(vec![]);
    }
    let marks = vec!["?"; env_ids.len()].join(", ");
    let mut q = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT env_id, node, image_key, image_ref, image_id, namespace, workload, container,
             running, pull_secrets FROM ee_scan_uses WHERE env_id IN ({marks})
         ORDER BY env_id, image_ref, namespace, workload, container"
    )));
    for id in env_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(store.pool()).await.map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|r| ImageUse {
            env_id: r.get("env_id"),
            node: r.get("node"),
            key: ImageKey(r.get("image_key")),
            image_ref: r.get("image_ref"),
            image_id: r.get("image_id"),
            namespace: r.get("namespace"),
            workload: r.get("workload"),
            container: r.get("container"),
            running: r.get("running"),
            pull_secrets: list_from(r.get::<String, _>("pull_secrets").as_str()),
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

impl Status {
    fn parse(s: &str) -> Status {
        match s {
            "queued" => Status::Queued,
            "running" => Status::Running,
            "done" => Status::Done,
            _ => Status::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanRow {
    pub id: i64,
    pub target: ScanTarget,
    pub trigger: Trigger,
    pub status: Status,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    pub trivy_image: Option<String>,
    /// Set on a finished scan.
    pub counts: Option<Counts>,
}

const SCAN_COLS: &str = "id, image_key, env_id, node, image_ref, namespace, pull_secrets, trigger,
    status, queued_at, started_at, finished_at, error, trivy_image, critical, high, medium, low,
    unknown, critical_fixable, high_fixable";

fn scan_from(r: &sqlx::sqlite::SqliteRow) -> ScanRow {
    let n = |c: &str| r.get::<Option<i64>, _>(c).map(|v| v as u32);
    let counts = n("critical").map(|critical| Counts {
        critical,
        high: n("high").unwrap_or(0),
        medium: n("medium").unwrap_or(0),
        low: n("low").unwrap_or(0),
        unknown: n("unknown").unwrap_or(0),
        critical_fixable: n("critical_fixable").unwrap_or(0),
        high_fixable: n("high_fixable").unwrap_or(0),
    });
    ScanRow {
        id: r.get("id"),
        target: ScanTarget {
            env_id: r.get("env_id"),
            node: r.get("node"),
            key: ImageKey(r.get("image_key")),
            image_ref: r.get("image_ref"),
            namespace: r.get("namespace"),
            pull_secrets: list_from(r.get::<String, _>("pull_secrets").as_str()),
        },
        trigger: Trigger::parse(r.get::<String, _>("trigger").as_str()).unwrap_or(Trigger::Manual),
        status: Status::parse(r.get::<String, _>("status").as_str()),
        queued_at: r.get("queued_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
        error: r.get("error"),
        trivy_image: r.get("trivy_image"),
        counts,
    }
}

pub async fn insert_scan(store: &Store, t: &ScanTarget, trigger: Trigger, now: i64) -> Result<i64> {
    let r = sqlx::query(
        "INSERT INTO ee_scans (image_key, env_id, node, image_ref, namespace, pull_secrets, trigger,
             status, queued_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?)",
    )
    .bind(t.key.as_str())
    .bind(t.env_id)
    .bind(&t.node)
    .bind(&t.image_ref)
    .bind(&t.namespace)
    .bind(json_list(&t.pull_secrets))
    .bind(trigger.as_str())
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(r.last_insert_rowid())
}

/// A queued or running scan of `key`, if there is one.
pub async fn active_scan(store: &Store, key: &ImageKey) -> Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM ee_scans WHERE image_key = ? AND status IN ('queued', 'running')
         ORDER BY id LIMIT 1",
    )
    .bind(key.as_str())
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)
}

pub async fn scan(store: &Store, id: i64) -> Result<Option<ScanRow>> {
    let r = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SCAN_COLS} FROM ee_scans WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(store.pool())
    .await
    .map_err(db_err)?;
    Ok(r.as_ref().map(scan_from))
}

pub async fn mark_running(store: &Store, id: i64, now: i64) -> Result<()> {
    sqlx::query("UPDATE ee_scans SET status = 'running', started_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Records a finished scan and returns the findings its image did not have
/// in its previous finished scan. Older scans of the image lose their
/// findings, and only the newest [`HISTORY`] scan rows are kept.
pub async fn finish_ok(
    store: &Store,
    id: i64,
    trivy_image: &str,
    findings: &[Finding],
    now: i64,
) -> Result<Vec<Finding>> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let key: String = sqlx::query_scalar("SELECT image_key FROM ee_scans WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
    let prev: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM ee_scans WHERE image_key = ? AND status = 'done' AND id != ?
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&key)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    let before: HashSet<(String, String)> = match prev {
        None => HashSet::new(),
        Some(p) => {
            sqlx::query_as("SELECT vuln_id, pkg_name FROM ee_scan_findings WHERE scan_id = ?")
                .bind(p)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_err)?
                .into_iter()
                .collect()
        }
    };
    let c = counts(findings);
    sqlx::query(
        "UPDATE ee_scans SET status = 'done', finished_at = ?, error = NULL, trivy_image = ?,
             critical = ?, high = ?, medium = ?, low = ?, unknown = ?, critical_fixable = ?,
             high_fixable = ? WHERE id = ?",
    )
    .bind(now)
    .bind(trivy_image)
    .bind(c.critical)
    .bind(c.high)
    .bind(c.medium)
    .bind(c.low)
    .bind(c.unknown)
    .bind(c.critical_fixable)
    .bind(c.high_fixable)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    for f in findings {
        sqlx::query(
            "INSERT OR IGNORE INTO ee_scan_findings (scan_id, vuln_id, pkg_name, installed_version,
                 fixed_version, severity, title, url) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&f.vuln_id)
        .bind(&f.pkg_name)
        .bind(&f.installed_version)
        .bind(&f.fixed_version)
        .bind(f.severity.as_str())
        .bind(&f.title)
        .bind(&f.url)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT OR IGNORE INTO ee_scan_first_seen (image_key, vuln_id, pkg_name, first_seen_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&key)
        .bind(&f.vuln_id)
        .bind(&f.pkg_name)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    sqlx::query(
        "DELETE FROM ee_scan_findings WHERE scan_id IN
             (SELECT id FROM ee_scans WHERE image_key = ? AND id != ?)",
    )
    .bind(&key)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM ee_scans WHERE image_key = ? AND status IN ('done', 'failed') AND id NOT IN
             (SELECT id FROM ee_scans WHERE image_key = ? ORDER BY id DESC LIMIT ?)",
    )
    .bind(&key)
    .bind(&key)
    .bind(HISTORY)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut seen = HashSet::new();
    Ok(findings
        .iter()
        .filter(|f| !before.contains(&(f.vuln_id.clone(), f.pkg_name.clone())))
        .filter(|f| seen.insert((f.vuln_id.clone(), f.pkg_name.clone())))
        .cloned()
        .collect())
}

pub async fn finish_failed(store: &Store, id: i64, error: &str, now: i64) -> Result<()> {
    sqlx::query("UPDATE ee_scans SET status = 'failed', finished_at = ?, error = ? WHERE id = ?")
        .bind(now)
        .bind(error)
        .bind(id)
        .execute(store.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}

/// An image's newest scan, and its newest finished one.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyState {
    pub last: ScanRow,
    pub last_done: Option<ScanRow>,
}

pub async fn latest(store: &Store, keys: &[ImageKey]) -> Result<HashMap<ImageKey, KeyState>> {
    let mut out = HashMap::new();
    for chunk in keys.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut q = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SCAN_COLS} FROM ee_scans WHERE image_key IN ({marks}) ORDER BY id DESC"
        )));
        for k in chunk {
            q = q.bind(k.as_str());
        }
        for r in q.fetch_all(store.pool()).await.map_err(db_err)? {
            let row = scan_from(&r);
            let key = row.target.key.clone();
            let entry = out.entry(key).or_insert_with(|| KeyState {
                last: row.clone(),
                last_done: None,
            });
            if entry.last_done.is_none() && row.status == Status::Done {
                entry.last_done = Some(row);
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindingRow {
    pub finding: Finding,
    pub first_seen_at: i64,
}

pub async fn findings(store: &Store, scan_id: i64) -> Result<Vec<FindingRow>> {
    let rows = sqlx::query(
        "SELECT f.vuln_id, f.pkg_name, f.installed_version, f.fixed_version, f.severity, f.title,
             f.url, COALESCE(s.first_seen_at, 0) AS first_seen_at
         FROM ee_scan_findings f JOIN ee_scans sc ON sc.id = f.scan_id
         LEFT JOIN ee_scan_first_seen s ON s.image_key = sc.image_key AND s.vuln_id = f.vuln_id
             AND s.pkg_name = f.pkg_name
         WHERE f.scan_id = ?",
    )
    .bind(scan_id)
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|r| FindingRow {
            finding: Finding {
                vuln_id: r.get("vuln_id"),
                pkg_name: r.get("pkg_name"),
                installed_version: r.get("installed_version"),
                fixed_version: r.get("fixed_version"),
                severity: Severity::parse(r.get::<String, _>("severity").as_str()),
                title: r.get("title"),
                url: r.get("url"),
            },
            first_seen_at: r.get("first_seen_at"),
        })
        .collect())
}

/// After a restart nothing is running: running and queued scans are failed
/// "interrupted by restart", and their targets returned to queue again.
pub async fn recover_after_restart(store: &Store, now: i64) -> Result<Vec<ScanTarget>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SCAN_COLS} FROM ee_scans WHERE status IN ('queued', 'running') ORDER BY id"
    )))
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    sqlx::query(
        "UPDATE ee_scans SET status = 'failed', finished_at = ?, error = 'interrupted by restart'
         WHERE status IN ('queued', 'running')",
    )
    .bind(now)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(rows.iter().map(|r| scan_from(r).target).collect())
}

pub async fn fail_queued_for_env(store: &Store, env_id: i64, reason: &str, now: i64) -> Result<()> {
    sqlx::query(
        "UPDATE ee_scans SET status = 'failed', finished_at = ?, error = ?
         WHERE env_id = ? AND status = 'queued'",
    )
    .bind(now)
    .bind(reason)
    .bind(env_id)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Forgets images no environment uses any more and nothing scanned since
/// `older_than`.
pub async fn prune_unused(store: &Store, older_than: i64) -> Result<()> {
    let gone = "SELECT image_key FROM ee_scans GROUP BY image_key
        HAVING MAX(queued_at) < ? AND image_key NOT IN (SELECT image_key FROM ee_scan_uses)";
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM ee_scan_first_seen WHERE image_key IN ({gone})"
    )))
    .bind(older_than)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM ee_scans WHERE image_key IN ({gone})"
    )))
    .bind(older_than)
    .execute(store.pool())
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Queued scans, oldest first.
pub async fn queued(store: &Store, limit: i64) -> Result<Vec<ScanRow>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SCAN_COLS} FROM ee_scans WHERE status = 'queued' ORDER BY id LIMIT ?"
    )))
    .bind(limit)
    .fetch_all(store.pool())
    .await
    .map_err(db_err)?;
    Ok(rows.iter().map(scan_from).collect())
}
