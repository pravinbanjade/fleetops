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
        return Err(Error::Invalid(
            "save the client secret before enabling".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn audit(
    s: &AppState,
    admin: i64,
    action: &str,
    c: &Connection,
    mut detail: serde_json::Value,
) {
    detail["connection"] = json!(c.name);
    audit_detail(
        s,
        Some(admin),
        action,
        "sso_connection",
        &c.id.to_string(),
        "success",
        Some(detail),
    )
    .await;
}

#[utoipa::path(get, path = "/ee/sso/connections", operation_id = "listSsoConnections", tag = "enterprise",
    summary = "Single sign-on connections",
    responses((status = 200, description = "Connections by name", body = Vec<ConnectionView>)))]
pub async fn list(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<Vec<ConnectionView>>> {
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
    let session_hours =
        validate_session_hours(body.session_hours.unwrap_or(SESSION_HOURS_DEFAULT))?;
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
    audit(
        &s,
        admin.user.id,
        "sso.connection.create",
        &c,
        json!({"kind": settings.kind()}),
    )
    .await;
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
    axum::Extension(rt): super::flow::Rt,
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
        if settings.identity_source() != old.settings.identity_source()
            && !db::connection_users(s.store.pool(), id).await?.is_empty()
        {
            return Err(Error::Conflict(
                "users have signed in through this provider; add a new connection for a different one"
                    .into(),
            )
            .into());
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
    rt.oidc.forget();
    if old.enabled != c.enabled {
        let action = if c.enabled {
            "sso.connection.enable"
        } else {
            "sso.connection.disable"
        };
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
pub async fn get_settings(
    State(s): State<AppState>,
    _a: AdminUser,
) -> ApiResult<Json<SsoSettings>> {
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

#[derive(Serialize, ToSchema)]
pub struct DeletedConnection {
    /// SSO users the connection leaves behind; they can no longer sign in
    /// and an admin may delete them.
    pub users_left: i64,
}

#[utoipa::path(delete, path = "/ee/sso/connections/{id}", operation_id = "deleteSsoConnection", tag = "enterprise",
    summary = "Delete a connection: its group sync is undone and its users' sessions end",
    params(("id" = i64, Path, description = "Connection id")),
    responses((status = 200, description = "Deleted", body = DeletedConnection)))]
pub async fn delete(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<DeletedConnection>> {
    let c = db::get(s.store.pool(), id).await?;
    let users = db::connection_users(s.store.pool(), id).await?;
    // Before the delete: the cascade would turn sync rows into manual ones.
    super::provision::detach_connection(&s, id).await?;
    db::delete(s.store.pool(), id).await?;
    audit(
        &s,
        admin.user.id,
        "sso.connection.delete",
        &c,
        json!({"users_left": users.len()}),
    )
    .await;
    Ok(Json(DeletedConnection {
        users_left: users.len() as i64,
    }))
}

#[derive(Serialize, ToSchema)]
pub struct MappingView {
    pub id: i64,
    pub group: String,
    /// Null when the group grants admin.
    pub team_id: Option<i64>,
    pub team_name: Option<String>,
    pub admin: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct NewMapping {
    /// The group as the provider names it: a name, an id, an email or a DN.
    pub group: String,
    #[serde(default)]
    pub team_id: Option<i64>,
    /// Make members FleetOps admins instead of joining a team.
    #[serde(default)]
    pub admin: bool,
}

fn mapping_view(m: db::Mapping) -> MappingView {
    MappingView {
        id: m.id,
        group: m.group_value,
        team_id: m.team_id,
        team_name: m.team_name,
        admin: m.is_admin,
    }
}

#[utoipa::path(get, path = "/ee/sso/connections/{id}/mappings", operation_id = "listSsoMappings", tag = "enterprise",
    summary = "A connection's group mappings", params(("id" = i64, Path, description = "Connection id")),
    responses((status = 200, description = "Mappings", body = Vec<MappingView>)))]
pub async fn list_mappings(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Vec<MappingView>>> {
    db::get(s.store.pool(), id).await?;
    Ok(Json(
        db::mappings(s.store.pool(), id)
            .await?
            .into_iter()
            .map(mapping_view)
            .collect(),
    ))
}

#[utoipa::path(post, path = "/ee/sso/connections/{id}/mappings", operation_id = "createSsoMapping", tag = "enterprise",
    summary = "Map a group to a team or to admin (needs the sso feature); applies at each member's next sign-in",
    params(("id" = i64, Path, description = "Connection id")), request_body = NewMapping,
    responses((status = 201, description = "Created", body = MappingView)))]
pub async fn create_mapping(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<NewMapping>,
) -> ApiResult<(StatusCode, Json<MappingView>)> {
    require_feature(&s, Feature::Sso)?;
    let c = db::get(s.store.pool(), id).await?;
    let group = body.group.trim();
    if group.is_empty() || group.len() > 512 {
        return Err(Error::Invalid("group must be 1-512 characters".into()).into());
    }
    match (body.team_id, body.admin) {
        (Some(_), true) | (None, false) => {
            return Err(
                Error::Invalid("map the group to exactly one of a team or admin".into()).into(),
            )
        }
        (Some(t), false) => {
            crate::teams::db::get(s.store.pool(), t).await?;
        }
        (None, true) => {}
    }
    let mid = db::add_mapping(s.store.pool(), id, group, body.team_id, now_unix()).await?;
    let m = db::mappings(s.store.pool(), id)
        .await?
        .into_iter()
        .find(|m| m.id == mid)
        .ok_or_else(|| Error::Internal("mapping vanished".into()))?;
    audit(
        &s,
        admin.user.id,
        "sso.mapping.create",
        &c,
        json!({"group": m.group_value, "team": m.team_name, "admin": m.is_admin}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(mapping_view(m))))
}

#[utoipa::path(delete, path = "/ee/sso/connections/{id}/mappings/{mapping_id}", operation_id = "deleteSsoMapping",
    tag = "enterprise", summary = "Remove a mapping; members lose what it gave them now",
    params(("id" = i64, Path, description = "Connection id"), ("mapping_id" = i64, Path, description = "Mapping id")),
    responses((status = 204, description = "Gone")))]
pub async fn delete_mapping(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, mapping_id)): ApiPath<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let c = db::get(s.store.pool(), id).await?;
    let m = db::mappings(s.store.pool(), id)
        .await?
        .into_iter()
        .find(|m| m.id == mapping_id);
    if db::delete_mapping(s.store.pool(), id, mapping_id).await? {
        super::provision::resync_connection(&s, id).await?;
        if let Some(m) = m {
            audit(
                &s,
                admin.user.id,
                "sso.mapping.delete",
                &c,
                json!({"group": m.group_value, "team": m.team_name, "admin": m.is_admin}),
            )
            .await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema, Default)]
pub struct TestBody {
    /// Google: also list this user's groups through the service account.
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct TestResult {
    pub ok: bool,
    pub detail: String,
    /// The groups found for `email`, when asked.
    pub groups: Option<Vec<String>>,
}

#[utoipa::path(post, path = "/ee/sso/connections/{id}/test", operation_id = "testSsoConnection", tag = "enterprise",
    summary = "Check that FleetOps can reach the identity provider",
    params(("id" = i64, Path, description = "Connection id")), request_body = TestBody,
    responses((status = 200, description = "What the check found", body = TestResult)))]
pub async fn test(
    State(s): State<AppState>,
    axum::Extension(rt): super::flow::Rt,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<TestBody>,
) -> ApiResult<Json<TestResult>> {
    let c = db::get(s.store.pool(), id).await?;
    rt.oidc.forget();
    let outcome: Result<(String, Option<Vec<String>>), Error> = async {
        match &c.settings {
            ConnectionSettings::Github { .. } => {
                let api = super::github::api_base(&rt, &c);
                let _: serde_json::Value = rt.oidc.get_json(&format!("{api}/meta"), None).await?;
                Ok((format!("reached {api}"), None))
            }
            ConnectionSettings::Google { admin_email, .. } => {
                let issuer = super::flow::issuer(&rt, &c);
                rt.oidc.discover(&issuer).await?;
                match (
                    body.email.as_deref(),
                    db::secrets(&s.store, id).await?.service_account_json,
                    admin_email,
                ) {
                    (Some(email), Some(key), Some(admin)) => {
                        let g = super::google::groups(&rt, &key, admin, email).await?;
                        Ok((format!("found {} groups for {email}", g.len()), Some(g)))
                    }
                    (Some(_), _, _) => Err(Error::Invalid(
                        "save a service account key and admin email to read groups".into(),
                    )),
                    _ => Ok(("discovery and keys reachable".into(), None)),
                }
            }
            _ => {
                let issuer = super::flow::issuer(&rt, &c);
                let d = rt.oidc.discover(&issuer).await?;
                let _: serde_json::Value = rt.oidc.get_json(&d.jwks_uri, None).await?;
                Ok((format!("discovered {}", d.issuer), None))
            }
        }
    }
    .await;
    Ok(Json(match outcome {
        Ok((detail, groups)) => TestResult {
            ok: true,
            detail,
            groups,
        },
        Err(e) => TestResult {
            ok: false,
            detail: e.to_string(),
            groups: None,
        },
    }))
}
