//! `/ee/notifications` (notifications spec, Enterprise): destinations,
//! rules, SMTP settings and the delivery log. Admins only. Without a
//! license, reading and deleting still work, so leftovers can be removed.

use super::db::{self, DeliveryStatus, NewDestination, RuleRow, SmtpRow};
use super::matcher::baseline_rule;
use super::model::{DestinationKind, DestinationSecrets, Rule, SmtpSecurity};
use super::notifier::Notifier;
use super::send::host_of;
use crate::scan::db::PasswordChange;
use crate::scan::model::Severity;
use axum::{extract::State, http::StatusCode, Extension, Json};
use fleetops_api::{
    audit_detail,
    auth::extract::AdminUser,
    authz::require_feature,
    extract::{ApiJson, ApiPath, ApiQuery},
    ApiResult, AppState,
};
use fleetops_core::edition::Feature;
use fleetops_core::{now_unix, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

type Note = Extension<Arc<Notifier>>;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DestinationView {
    pub id: i64,
    pub name: String,
    pub kind: DestinationKind,
    /// The URL's host; never the URL.
    pub host: Option<String>,
    pub recipients: Vec<String>,
    /// A URL or routing key is saved.
    pub secret_set: bool,
    pub signing_secret_set: bool,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct DestinationBody {
    pub name: String,
    pub kind: DestinationKind,
    /// Write-only. Omit on update to keep the saved one.
    pub url: Option<String>,
    /// PagerDuty only; write-only.
    pub routing_key: Option<String>,
    /// Generic webhook only; write-only. An empty string removes it.
    pub signing_secret: Option<String>,
    /// Email only.
    pub recipients: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RuleBody {
    pub name: String,
    pub enabled: bool,
    /// Environment names; empty is every environment.
    pub envs: Vec<String>,
    pub min_severity: Severity,
    pub fixable_only: bool,
    pub running_only: bool,
    pub destinations: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RuleView {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub envs: Vec<String>,
    pub min_severity: Severity,
    pub fixable_only: bool,
    pub running_only: bool,
    pub destinations: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SmtpBody {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    /// Write-only; omit to keep the saved one.
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
    pub from_address: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SmtpView {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password_set: bool,
    pub from_address: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SmtpTestBody {
    pub to: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeliveryView {
    pub id: i64,
    pub created_at: i64,
    pub sent_at: Option<i64>,
    pub rule: Option<String>,
    pub destination: Option<String>,
    pub title: String,
    pub status: DeliveryStatus,
    pub attempts: i64,
    pub error: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeliveriesQuery {
    /// Default 50, at most 200.
    pub limit: Option<i64>,
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn check_name(n: &str) -> Result<String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 64 {
        return Err(invalid("give a name of 1 to 64 characters"));
    }
    Ok(n.to_string())
}

fn check_url(kind: DestinationKind, url: &str) -> Result<()> {
    let u = url::Url::parse(url.trim()).map_err(|_| invalid("that is not a URL"))?;
    let ok = match u.scheme() {
        "https" => true,
        "http" => kind == DestinationKind::Webhook,
        _ => false,
    };
    if !ok || u.host_str().is_none() {
        return Err(invalid(if kind == DestinationKind::Webhook {
            "the URL must start with https:// or http://"
        } else {
            "the URL must start with https://"
        }));
    }
    Ok(())
}

fn check_recipients(r: &[String]) -> Result<Vec<String>> {
    if r.is_empty() || r.len() > 50 {
        return Err(invalid("give 1 to 50 email addresses"));
    }
    r.iter()
        .map(|a| {
            let a = a.trim();
            let valid = a
                .split_once('@')
                .is_some_and(|(l, d)| !l.is_empty() && d.contains('.'))
                && !a.chars().any(char::is_whitespace);
            if valid {
                Ok(a.to_string())
            } else {
                Err(invalid(format!("'{a}' is not an email address")))
            }
        })
        .collect()
}

fn view(d: &super::model::Destination, s: &DestinationSecrets) -> DestinationView {
    DestinationView {
        id: d.id,
        name: d.name.clone(),
        kind: d.kind,
        host: d.host.clone(),
        recipients: d.recipients.clone(),
        secret_set: s.url.is_some() || s.routing_key.is_some(),
        signing_secret_set: s.signing_secret.is_some(),
        last_error: d.last_error.clone(),
        last_error_at: d.last_error_at,
    }
}

async fn audit(s: &AppState, by: i64, action: &str, target: &str, detail: serde_json::Value) {
    audit_detail(
        s,
        Some(by),
        action,
        "notification",
        target,
        "success",
        Some(detail),
    )
    .await
}

#[utoipa::path(get, path = "/ee/notifications/destinations", operation_id = "listNotificationDestinations", tag = "enterprise",
    responses((status = 200, description = "Destinations", body = Vec<DestinationView>)))]
pub async fn list_destinations(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<DestinationView>>> {
    let mut out = Vec::new();
    for d in db::list_destinations(&s.store).await? {
        let secrets = db::get_destination(&s.store, d.id)
            .await
            .map(|x| x.1)
            .unwrap_or_default();
        out.push(view(&d, &secrets));
    }
    Ok(Json(out))
}

/// The secrets a body sets, merged over `current`; errors for missing ones.
fn secrets_of(
    b: &DestinationBody,
    current: Option<&DestinationSecrets>,
) -> Result<DestinationSecrets> {
    let mut s = current.cloned().unwrap_or_default();
    match b.kind {
        DestinationKind::Email => {}
        DestinationKind::PagerDuty => {
            if let Some(k) = b
                .routing_key
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
            {
                s.routing_key = Some(k.to_string());
            }
            if s.routing_key.is_none() {
                return Err(invalid("give the PagerDuty routing key"));
            }
        }
        kind => {
            if let Some(u) = b.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                check_url(kind, u)?;
                s.url = Some(u.to_string());
            }
            if s.url.is_none() {
                return Err(invalid("give the webhook URL"));
            }
            if kind == DestinationKind::Webhook {
                match b.signing_secret.as_deref() {
                    Some("") => s.signing_secret = None,
                    Some(x) => s.signing_secret = Some(x.to_string()),
                    None => {}
                }
            }
        }
    }
    Ok(s)
}

#[utoipa::path(post, path = "/ee/notifications/destinations", operation_id = "createNotificationDestination", tag = "enterprise",
    request_body = DestinationBody,
    responses((status = 200, description = "Created", body = DestinationView)))]
pub async fn create_destination(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<DestinationBody>,
) -> ApiResult<Json<DestinationView>> {
    require_feature(&s, Feature::Notifications)?;
    let name = check_name(&b.name)?;
    let recipients = if b.kind == DestinationKind::Email {
        check_recipients(b.recipients.as_deref().unwrap_or_default())?
    } else {
        vec![]
    };
    let secrets = secrets_of(&b, None)?;
    let host = secrets.url.as_deref().and_then(host_of);
    let id = db::create_destination(
        &s.store,
        &NewDestination {
            name: name.clone(),
            kind: b.kind,
            host,
            recipients,
            secrets,
        },
        now_unix(),
    )
    .await?;
    audit(
        &s,
        cu.user.id,
        "notification.destination.create",
        &name,
        json!({"kind": b.kind}),
    )
    .await;
    let (d, sec) = db::get_destination(&s.store, id).await?;
    Ok(Json(view(&d, &sec)))
}

#[utoipa::path(put, path = "/ee/notifications/destinations/{id}", operation_id = "updateNotificationDestination", tag = "enterprise",
    params(("id" = i64, Path)), request_body = DestinationBody,
    responses((status = 200, description = "Saved", body = DestinationView)))]
pub async fn update_destination(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(b): ApiJson<DestinationBody>,
) -> ApiResult<Json<DestinationView>> {
    require_feature(&s, Feature::Notifications)?;
    let (cur, cur_sec) = db::get_destination(&s.store, id).await?;
    if cur.kind != b.kind {
        return Err(invalid("a destination's kind can't change; add a new one").into());
    }
    let name = check_name(&b.name)?;
    let recipients = if b.kind == DestinationKind::Email {
        check_recipients(b.recipients.as_deref().unwrap_or(&cur.recipients))?
    } else {
        vec![]
    };
    let secrets = secrets_of(&b, Some(&cur_sec))?;
    let host = secrets.url.as_deref().and_then(host_of);
    db::update_destination(
        &s.store,
        id,
        &name,
        host.as_deref(),
        &recipients,
        Some(&secrets),
        now_unix(),
    )
    .await?;
    audit(
        &s,
        cu.user.id,
        "notification.destination.update",
        &name,
        json!({"kind": b.kind}),
    )
    .await;
    let (d, sec) = db::get_destination(&s.store, id).await?;
    Ok(Json(view(&d, &sec)))
}

#[utoipa::path(delete, path = "/ee/notifications/destinations/{id}", operation_id = "deleteNotificationDestination", tag = "enterprise",
    params(("id" = i64, Path)), responses((status = 204, description = "Deleted")))]
pub async fn delete_destination(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    let (d, _) = db::get_destination(&s.store, id).await?;
    let only = db::rules_only_sending_to(&s.store, id).await?;
    if !only.is_empty() {
        let one = only.len() == 1;
        return Err(Error::Conflict(format!(
            "{} only send{} here: give {} another destination or delete {} first",
            only.join(", "),
            if one { "s" } else { "" },
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
        ))
        .into());
    }
    db::delete_destination(&s.store, id).await?;
    audit(
        &s,
        cu.user.id,
        "notification.destination.delete",
        &d.name,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/ee/notifications/destinations/{id}/test", operation_id = "testNotificationDestination", tag = "enterprise",
    params(("id" = i64, Path)), responses((status = 204, description = "Delivered")))]
pub async fn test_destination(
    State(s): State<AppState>,
    Extension(n): Note,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Notifications)?;
    let (d, _) = db::get_destination(&s.store, id).await?;
    let r = n.test_destination(&s, id).await;
    audit(
        &s,
        cu.user.id,
        "notification.destination.test",
        &d.name,
        json!({"ok": r.is_ok()}),
    )
    .await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

async fn env_maps(s: &AppState) -> Result<(HashMap<String, i64>, HashMap<i64, String>)> {
    let recs = s.store.list_environments().await?;
    Ok((
        recs.iter().map(|r| (r.name.clone(), r.id)).collect(),
        recs.into_iter().map(|r| (r.id, r.name)).collect(),
    ))
}

fn rule_view(r: &Rule, names: &HashMap<i64, String>) -> RuleView {
    RuleView {
        id: r.id,
        name: r.name.clone(),
        enabled: r.enabled,
        envs: r
            .envs
            .iter()
            .filter_map(|id| names.get(id).cloned())
            .collect(),
        min_severity: r.min_severity,
        fixable_only: r.fixable_only,
        running_only: r.running_only,
        destinations: r.destinations.clone(),
    }
}

async fn rule_row(s: &AppState, b: &RuleBody) -> Result<RuleRow> {
    let (ids, _) = env_maps(s).await?;
    if b.destinations.is_empty() {
        return Err(invalid("send to at least one destination"));
    }
    if b.min_severity == Severity::Unknown {
        return Err(invalid("pick critical, high, medium or low"));
    }
    let envs = b
        .envs
        .iter()
        .map(|n| {
            ids.get(n)
                .copied()
                .ok_or_else(|| invalid(format!("no environment '{n}'")))
        })
        .collect::<Result<Vec<i64>>>()?;
    Ok(RuleRow {
        name: check_name(&b.name)?,
        enabled: b.enabled,
        envs,
        min_severity: b.min_severity,
        fixable_only: b.fixable_only,
        running_only: b.running_only,
        destinations: b.destinations.clone(),
    })
}

#[utoipa::path(get, path = "/ee/notifications/rules", operation_id = "listNotificationRules", tag = "enterprise",
    responses((status = 200, description = "Rules", body = Vec<RuleView>)))]
pub async fn list_rules(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<RuleView>>> {
    let (_, names) = env_maps(&s).await?;
    Ok(Json(
        db::list_rules(&s.store)
            .await?
            .iter()
            .map(|r| rule_view(r, &names))
            .collect(),
    ))
}

async fn one_rule(s: &AppState, id: i64) -> Result<RuleView> {
    let (_, names) = env_maps(s).await?;
    let r = db::get_rule(&s.store, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("rule {id}")))?;
    Ok(rule_view(&r, &names))
}

#[utoipa::path(post, path = "/ee/notifications/rules", operation_id = "createNotificationRule", tag = "enterprise",
    request_body = RuleBody, responses((status = 200, description = "Created", body = RuleView)))]
pub async fn create_rule(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<RuleBody>,
) -> ApiResult<Json<RuleView>> {
    require_feature(&s, Feature::Notifications)?;
    let row = rule_row(&s, &b).await?;
    // Created off, baselined, then turned on: a scan finishing meanwhile can't
    // make the existing state look new.
    let off = RuleRow {
        enabled: false,
        ..row.clone()
    };
    let id = db::create_rule(&s.store, &off, now_unix()).await?;
    baseline_rule(&s, id).await?;
    if row.enabled {
        db::update_rule(&s.store, id, &row, now_unix()).await?;
    }
    audit(
        &s,
        cu.user.id,
        "notification.rule.create",
        &row.name,
        json!({"envs": b.envs, "min_severity": b.min_severity}),
    )
    .await;
    Ok(Json(one_rule(&s, id).await?))
}

#[utoipa::path(put, path = "/ee/notifications/rules/{id}", operation_id = "updateNotificationRule", tag = "enterprise",
    params(("id" = i64, Path)), request_body = RuleBody, responses((status = 200, description = "Saved", body = RuleView)))]
pub async fn update_rule(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(b): ApiJson<RuleBody>,
) -> ApiResult<Json<RuleView>> {
    require_feature(&s, Feature::Notifications)?;
    let row = rule_row(&s, &b).await?;
    let before = db::get_rule(&s.store, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("rule {id}")))?;
    // A lower threshold, dropping a filter, or turning back on makes CVEs it
    // was never told about match: record them silently, with the rule off.
    let rebaseline = row.min_severity < before.min_severity
        || (before.fixable_only && !row.fixable_only)
        || (before.running_only && !row.running_only)
        || (!before.enabled && row.enabled);
    let off = RuleRow {
        enabled: false,
        ..row.clone()
    };
    db::update_rule(&s.store, id, &off, now_unix()).await?;
    if rebaseline {
        db::rebaseline_rule(&s.store, id).await?;
    }
    baseline_rule(&s, id).await?;
    if row.enabled {
        db::update_rule(&s.store, id, &row, now_unix()).await?;
    }
    audit(
        &s,
        cu.user.id,
        "notification.rule.update",
        &row.name,
        json!({"envs": b.envs, "min_severity": b.min_severity}),
    )
    .await;
    Ok(Json(one_rule(&s, id).await?))
}

#[utoipa::path(delete, path = "/ee/notifications/rules/{id}", operation_id = "deleteNotificationRule", tag = "enterprise",
    params(("id" = i64, Path)), responses((status = 204, description = "Deleted")))]
pub async fn delete_rule(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    let r = db::get_rule(&s.store, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("rule {id}")))?;
    db::delete_rule(&s.store, id).await?;
    audit(
        &s,
        cu.user.id,
        "notification.rule.delete",
        &r.name,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/ee/notifications/smtp", operation_id = "getSmtpSettings", tag = "enterprise",
    responses((status = 200, description = "SMTP settings, if saved", body = Option<SmtpView>)))]
pub async fn get_smtp(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Option<SmtpView>>> {
    Ok(Json(db::get_smtp(&s.store).await?.map(|c| SmtpView {
        host: c.host,
        port: c.port,
        security: c.security,
        username: c.username,
        password_set: c.password.is_some(),
        from_address: c.from_address,
    })))
}

#[utoipa::path(put, path = "/ee/notifications/smtp", operation_id = "putSmtpSettings", tag = "enterprise",
    request_body = SmtpBody, responses((status = 200, description = "Saved", body = Option<SmtpView>)))]
pub async fn put_smtp(
    State(s): State<AppState>,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<SmtpBody>,
) -> ApiResult<Json<Option<SmtpView>>> {
    require_feature(&s, Feature::Notifications)?;
    if b.host.trim().is_empty() || b.port == 0 {
        return Err(invalid("give the mail server's host and port").into());
    }
    check_recipients(std::slice::from_ref(&b.from_address))?;
    let pw = if b.clear_password {
        PasswordChange::Clear
    } else {
        match b.password.as_deref() {
            Some(p) if !p.is_empty() => PasswordChange::Set(p.to_string()),
            _ => PasswordChange::Keep,
        }
    };
    let row = SmtpRow {
        host: b.host.trim().to_string(),
        port: b.port,
        security: b.security,
        username: b
            .username
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        from_address: b.from_address.trim().to_string(),
    };
    db::put_smtp(&s.store, &row, pw, now_unix()).await?;
    audit(
        &s,
        cu.user.id,
        "notification.smtp.update",
        &row.host,
        json!({"port": row.port, "security": row.security}),
    )
    .await;
    get_smtp(State(s), AdminUser(cu)).await
}

#[utoipa::path(post, path = "/ee/notifications/smtp/test", operation_id = "testSmtp", tag = "enterprise",
    request_body = SmtpTestBody, responses((status = 204, description = "Sent")))]
pub async fn test_smtp(
    State(s): State<AppState>,
    Extension(n): Note,
    AdminUser(cu): AdminUser,
    ApiJson(b): ApiJson<SmtpTestBody>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Notifications)?;
    let to = check_recipients(std::slice::from_ref(&b.to))?.remove(0);
    let r = n.test_smtp(&s, &to).await;
    audit(
        &s,
        cu.user.id,
        "notification.smtp.test",
        &to,
        json!({"ok": r.is_ok()}),
    )
    .await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/ee/notifications/deliveries", operation_id = "listNotificationDeliveries", tag = "enterprise",
    params(DeliveriesQuery), responses((status = 200, description = "Newest first", body = Vec<DeliveryView>)))]
pub async fn list_deliveries(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiQuery(q): ApiQuery<DeliveriesQuery>,
) -> ApiResult<Json<Vec<DeliveryView>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let dests: HashMap<i64, String> = db::list_destinations(&s.store)
        .await?
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();
    let rules: HashMap<i64, String> = db::list_rules(&s.store)
        .await?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect();
    Ok(Json(
        db::recent_deliveries(&s.store, limit)
            .await?
            .into_iter()
            .map(|d| DeliveryView {
                id: d.id,
                created_at: d.created_at,
                sent_at: d.sent_at,
                rule: d.rule_id.and_then(|r| rules.get(&r).cloned()),
                destination: d.destination_id.and_then(|x| dests.get(&x).cloned()),
                title: d.title,
                status: d.status,
                attempts: d.attempts,
                error: d.error,
            })
            .collect(),
    ))
}
