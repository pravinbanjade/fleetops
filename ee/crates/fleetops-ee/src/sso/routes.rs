//! `/ee/sso` (SSO spec, "API"). Admins only, except the sign-in redirect
//! routes in `flow`. Without a license, changes that add access are
//! refused; turning things off always works.

use super::{
    db::{self, Connection, NewConnection},
    model::{
        validate_name, validate_secrets, validate_session_hours, ConnectionSettings, Secrets,
        SESSION_HOURS_DEFAULT,
    },
    password_login, PasswordLogin, PASSWORD_LOGIN_KEY,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_api::{
    audit_detail,
    auth::extract::AdminUser,
    authz::require_feature,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use fleetops_core::{edition::Feature, now_unix, Error};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

/// A connection as the API shows it: secrets are never returned.
#[derive(Serialize, ToSchema)]
pub struct ConnectionView {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub settings: ConnectionSettings,
    /// The names of the secrets that are saved.
    pub secrets_set: Vec<String>,
    pub require_mapped_group: bool,
    pub session_hours: i64,
    /// Where the identity provider sends people back; null until
    /// `FLEETOPS_PUBLIC_URL` is set.
    pub callback_url: Option<String>,
    pub user_count: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct NewConnectionBody {
    pub name: String,
    /// Tagged by `kind`.
    #[schema(value_type = ConnectionSettings)]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub secrets: Secrets,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub require_mapped_group: bool,
    #[serde(default)]
    pub session_hours: Option<i64>,
}

/// Absent fields keep their value; absent secrets keep the saved ones.
#[derive(Deserialize, ToSchema, Default)]
pub struct UpdateConnectionBody {
    pub name: Option<String>,
    #[schema(value_type = Option<ConnectionSettings>)]
    pub settings: Option<serde_json::Value>,
    pub secrets: Option<Secrets>,
    pub enabled: Option<bool>,
    pub require_mapped_group: Option<bool>,
    pub session_hours: Option<i64>,
}

impl UpdateConnectionBody {
    /// Only turns the connection off: allowed without a license.
    fn only_disables(&self) -> bool {
        self.enabled == Some(false)
            && self.name.is_none()
            && self.settings.is_none()
            && self.secrets.is_none()
            && self.require_mapped_group.is_none()
            && self.session_hours.is_none()
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SsoSettings {
    pub password_login: PasswordLogin,
}

pub(crate) fn callback_url(s: &AppState, id: i64) -> Option<String> {
    s.config
        .public_url
        .as_ref()
        .map(|u| format!("{u}/api/v1/ee/sso/{id}/callback"))
}

async fn view(s: &AppState, c: Connection) -> ApiResult<ConnectionView> {
    let secrets = db::secrets(&s.store, c.id).await?;
    let users = db::user_counts(s.store.pool()).await?;
    Ok(ConnectionView {
        callback_url: callback_url(s, c.id),
        user_count: users.get(&c.id).copied().unwrap_or(0),
        kind: c.settings.kind().into(),
        secrets_set: secrets.names(),
        id: c.id,
        name: c.name,
        enabled: c.enabled,
        settings: c.settings,
        require_mapped_group: c.require_mapped_group,
        session_hours: c.session_hours,
    })
}

/// Enabling needs what a sign-in redirect needs.
fn check_enable(s: &AppState, secrets: &Secrets) -> Result<(), Error> {
    if s.config.public_url.is_none() {
        return Err(Error::Invalid(
            "set FLEETOPS_PUBLIC_URL to enable sign-in redirects".into(),
        ));
    }
    if secrets.client_secret.is_none() {
        return Err(Error::Invalid("save the client secret before enabling".into()));
    }
    Ok(())
}

pub(crate) async fn audit(s: &AppState, admin: i64, action: &str, c: &Connection, mut detail: serde_json::Value) {
    detail["connection"] = json!(c.name);
    audit_detail(s, Some(admin), action, "sso_connection", &c.id.to_string(), "success", Some(detail)).await;
}

#[utoipa::path(get, path = "/ee/sso/connections", operation_id = "listSsoConnections", tag = "enterprise",
    summary = "Single sign-on connections",
    responses((status = 200, description = "Connections by name", body = Vec<ConnectionView>)))]
pub async fn list(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<Vec<ConnectionView>>> {
    let mut out = vec![];
    for c in db::list(s.store.pool()).await? {
        out.push(view(&s, c).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(post, path = "/ee/sso/connections", operation_id = "createSsoConnection", tag = "enterprise",
    summary = "Add a single sign-on connection (needs the sso feature)", request_body = NewConnectionBody,
    responses((status = 201, description = "Created", body = ConnectionView)))]
pub async fn create(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiJson(body): ApiJson<NewConnectionBody>,
) -> ApiResult<(StatusCode, Json<ConnectionView>)> {
    require_feature(&s, Feature::Sso)?;
    let name = validate_name(&body.name)?;
    let settings = ConnectionSettings::parse(body.settings)?.validated()?;
    validate_secrets(&body.secrets)?;
    let session_hours = validate_session_hours(body.session_hours.unwrap_or(SESSION_HOURS_DEFAULT))?;
    if body.enabled {
        check_enable(&s, &body.secrets)?;
    }
    let c = db::create(
        &s.store,
        &NewConnection {
            name: &name,
            settings: &settings,
            secrets: &body.secrets,
            enabled: body.enabled,
            require_mapped_group: body.require_mapped_group,
            session_hours,
            now: now_unix(),
        },
    )
    .await?;
    audit(&s, admin.user.id, "sso.connection.create", &c, json!({"kind": settings.kind()})).await;
    Ok((StatusCode::CREATED, Json(view(&s, c).await?)))
}

#[utoipa::path(get, path = "/ee/sso/connections/{id}", operation_id = "getSsoConnection", tag = "enterprise",
    summary = "A single sign-on connection", params(("id" = i64, Path, description = "Connection id")),
    responses((status = 200, description = "The connection", body = ConnectionView)))]
pub async fn get(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<ConnectionView>> {
    let c = db::get(s.store.pool(), id).await?;
    Ok(Json(view(&s, c).await?))
}

#[utoipa::path(patch, path = "/ee/sso/connections/{id}", operation_id = "updateSsoConnection", tag = "enterprise",
    summary = "Change a connection; without a license only disabling is allowed",
    params(("id" = i64, Path, description = "Connection id")), request_body = UpdateConnectionBody,
    responses((status = 200, description = "The connection", body = ConnectionView)))]
pub async fn update(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<UpdateConnectionBody>,
) -> ApiResult<Json<ConnectionView>> {
    if !body.only_disables() {
        require_feature(&s, Feature::Sso)?;
    }
    let old = db::get(s.store.pool(), id).await?;
    let mut c = old.clone();
    if let Some(name) = &body.name {
        c.name = validate_name(name)?;
    }
    if let Some(settings) = body.settings {
        let settings = ConnectionSettings::parse(settings)?.validated()?;
        if settings.kind() != old.settings.kind() {
            return Err(Error::Invalid("a connection's kind cannot change".into()).into());
        }
        c.settings = settings;
    }
    if let Some(h) = body.session_hours {
        c.session_hours = validate_session_hours(h)?;
    }
    if let Some(r) = body.require_mapped_group {
        c.require_mapped_group = r;
    }
    let mut secrets = db::secrets(&s.store, id).await?;
    if let Some(new) = body.secrets {
        validate_secrets(&new)?;
        secrets = secrets.merged(new);
    }
    if let Some(e) = body.enabled {
        c.enabled = e;
    }
    if c.enabled {
        check_enable(&s, &secrets)?;
    }
    let c = db::update(&s.store, &c, &secrets, now_unix()).await?;
    if old.enabled != c.enabled {
        let action = if c.enabled { "sso.connection.enable" } else { "sso.connection.disable" };
        audit(&s, admin.user.id, action, &c, json!({})).await;
        if !c.enabled {
            super::provision::detach_connection(&s, c.id).await?;
        }
    }
    if old.name != c.name
        || old.settings != c.settings
        || old.session_hours != c.session_hours
        || old.require_mapped_group != c.require_mapped_group
    {
        audit(&s, admin.user.id, "sso.connection.update", &c, json!({})).await;
    }
    Ok(Json(view(&s, c).await?))
}

#[utoipa::path(get, path = "/ee/sso/settings", operation_id = "getSsoSettings", tag = "enterprise",
    summary = "Who may log in with a local password",
    responses((status = 200, description = "Settings", body = SsoSettings)))]
pub async fn get_settings(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<SsoSettings>> {
    Ok(Json(SsoSettings {
        password_login: password_login(&s.store).await?,
    }))
}

#[utoipa::path(put, path = "/ee/sso/settings", operation_id = "putSsoSettings", tag = "enterprise",
    summary = "Limit password login to local admins, or allow everyone",
    request_body = SsoSettings, responses((status = 200, description = "Settings", body = SsoSettings)))]
pub async fn put_settings(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiJson(body): ApiJson<SsoSettings>,
) -> ApiResult<Json<SsoSettings>> {
    if body.password_login == PasswordLogin::LocalAdmins {
        require_feature(&s, Feature::Sso)?;
        if !db::local_admin_exists(s.store.pool()).await? {
            return Err(Error::Conflict(
                "create a local admin first; it is the way in if single sign-on breaks".into(),
            )
            .into());
        }
    }
    s.store
        .set_setting(PASSWORD_LOGIN_KEY, &json!(body.password_login))
        .await?;
    audit_detail(
        &s,
        Some(admin.user.id),
        "sso.settings.update",
        "sso_settings",
        "password_login",
        "success",
        Some(json!({"password_login": body.password_login})),
    )
    .await;
    Ok(Json(body))
}
