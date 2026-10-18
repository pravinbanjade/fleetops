use crate::{
    auth::{
        cookie::{clear_cookie, session_cookie},
        extract::{ClientMeta, CurrentUser},
        limiter::limiter_key,
        password, plausible_login, token, SESSION_ABSOLUTE_SECS, SESSION_IDLE_SECS,
    },
    extract::{ApiJson, ApiPath},
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fleetops_core::{
    external::LoginProvider, now_unix, user::PASSWORD_MAX, user::USERNAME_MAX, validate_password,
    validate_username, Error, User,
};
use fleetops_store::NewSession;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct SessionInfo {
    pub user: User,
    pub csrf_token: String,
}

#[derive(Serialize, ToSchema)]
pub struct AuthStatus {
    /// No user exists yet.
    pub setup_required: bool,
    /// `POST /auth/setup` is still accepted (the setup window has not closed).
    pub setup_open: bool,
}

#[utoipa::path(
    get,
    path = "/auth/status",
    operation_id = "getAuthStatus",
    tag = "auth",
    summary = "Whether first-run setup is still needed",
    security(()),
    responses((status = 200, description = "Setup state", body = AuthStatus))
)]
pub async fn status(State(s): State<AppState>) -> ApiResult<Json<AuthStatus>> {
    let users = s.store.count_users().await?;
    Ok(Json(AuthStatus {
        setup_required: users == 0,
        setup_open: users == 0 && s.setup.is_open(Instant::now()),
    }))
}

#[utoipa::path(
    post,
    path = "/auth/setup",
    operation_id = "setup",
    tag = "auth",
    summary = "Create the first admin and log in",
    request_body = Credentials,
    security(()),
    responses((status = 201, description = "Admin created; the session cookie is set", body = SessionInfo))
)]
pub async fn setup(
    State(s): State<AppState>,
    meta: ClientMeta,
    ApiJson(body): ApiJson<Credentials>,
) -> ApiResult<Response> {
    if s.store.count_users().await? > 0 {
        return Err(Error::Conflict("setup already completed".into()).into());
    }
    if !s.setup.is_open(Instant::now()) {
        return Err(Error::Forbidden(
            "setup window expired; restart fleetops to create the admin account".into(),
        )
        .into());
    }
    validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = password::hash_password(body.password).await?;
    let user = s
        .store
        .create_first_admin(&body.username, &hash, now_unix())
        .await?;
    audit(
        &s,
        Some(user.id),
        "auth.setup",
        "user",
        &user.id.to_string(),
        "success",
    )
    .await;
    start_session(&s, user, &meta, StatusCode::CREATED, SESSION_ABSOLUTE_SECS).await
}

#[utoipa::path(
    post,
    path = "/auth/login",
    operation_id = "login",
    tag = "auth",
    summary = "Log in",
    request_body = Credentials,
    security(()),
    responses((status = 200, description = "Logged in; the session cookie is set", body = SessionInfo))
)]
pub async fn login(
    State(s): State<AppState>,
    meta: ClientMeta,
    ApiJson(body): ApiJson<Credentials>,
) -> ApiResult<Response> {
    if !plausible_login(&body.username, &body.password) {
        let shown: String = body.username.chars().take(USERNAME_MAX).collect();
        audit(&s, None, "auth.login", "user", &shown, "failure").await;
        return Err(Error::Unauthorized("invalid username or password".into()).into());
    }
    let key = limiter_key(&body.username, &meta.ip);
    s.limiter.try_acquire(&key, Instant::now())?;

    // A local account whose password matches always wins, so a directory
    // user can never shadow the break-glass admin (SSO spec, LDAP login).
    let verified = match s.store.find_credentials(&body.username).await? {
        Some((user, hash)) if s.external_auth.password_allowed(&user).await? => {
            password::verify_password(body.password.clone(), hash)
                .await
                .then_some((user, SESSION_ABSOLUTE_SECS))
        }
        _ => {
            password::burn_verify(body.password.clone()).await;
            None
        }
    };
    let verified = match verified {
        Some(v) => Some(v),
        None => s
            .external_auth
            .directory_login(&body.username, &body.password)
            .await?
            .map(|(user, hours)| (user, hours.map_or(SESSION_ABSOLUTE_SECS, |h| h * 3600))),
    };
    let Some((user, lifetime)) = verified else {
        audit(&s, None, "auth.login", "user", &body.username, "failure").await;
        return Err(Error::Unauthorized("invalid username or password".into()).into());
    };
    s.limiter.reset(&key);
    audit(
        &s,
        Some(user.id),
        "auth.login",
        "user",
        &user.id.to_string(),
        "success",
    )
    .await;
    start_session(&s, user, &meta, StatusCode::OK, lifetime).await
}

#[utoipa::path(
    get,
    path = "/auth/providers",
    operation_id = "getAuthProviders",
    tag = "auth",
    summary = "Single sign-on providers for the login page",
    security(()),
    responses((status = 200, description = "Enabled providers; empty in Community", body = Vec<LoginProvider>))
)]
pub async fn providers(State(s): State<AppState>) -> ApiResult<Json<Vec<LoginProvider>>> {
    Ok(Json(s.external_auth.providers().await?))
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    operation_id = "logout",
    tag = "auth",
    summary = "End this session",
    responses((status = 204, description = "Logged out; the cookie is cleared"))
)]
pub async fn logout(
    State(s): State<AppState>,
    meta: ClientMeta,
    cu: CurrentUser,
) -> ApiResult<Response> {
    s.store.delete_session(cu.session.id).await?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(SET_COOKIE, clear_cookie(meta.tls));
    Ok(res)
}

#[utoipa::path(
    get,
    path = "/auth/me",
    operation_id = "getMe",
    tag = "auth",
    summary = "The current user and CSRF token",
    responses((status = 200, description = "Current session", body = SessionInfo))
)]
pub async fn me(cu: CurrentUser) -> Json<SessionInfo> {
    Json(SessionInfo {
        user: cu.user,
        csrf_token: cu.session.csrf_token,
    })
}

/// Starts a session for `user` that ends `absolute_secs` from now at the
/// latest, and sets its cookie.
pub async fn start_session(
    s: &AppState,
    user: User,
    meta: &ClientMeta,
    status: StatusCode,
    absolute_secs: i64,
) -> ApiResult<Response> {
    let session_token = token::generate_token();
    let csrf_token = token::generate_token();
    let now = now_unix();
    s.store
        .create_session(&NewSession {
            user_id: user.id,
            token_hash: &token::hash_token(&session_token),
            csrf_token: &csrf_token,
            now,
            expires_at: now + absolute_secs,
            user_agent: meta.user_agent.as_deref(),
            ip: Some(&meta.ip),
        })
        .await?;
    let mut res = (status, Json(SessionInfo { user, csrf_token })).into_response();
    res.headers_mut().insert(
        SET_COOKIE,
        session_cookie(&session_token, absolute_secs, meta.tls),
    );
    Ok(res)
}

#[derive(Serialize, ToSchema)]
pub struct SessionView {
    pub id: i64,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub current: bool,
}

#[utoipa::path(
    get,
    path = "/auth/sessions",
    operation_id = "listSessions",
    tag = "auth",
    summary = "The current user's active sessions",
    responses((status = 200, description = "Active sessions", body = Vec<SessionView>))
)]
pub async fn list_sessions(
    State(s): State<AppState>,
    cu: CurrentUser,
) -> ApiResult<Json<Vec<SessionView>>> {
    let now = now_unix();
    let views = s
        .store
        .list_sessions(cu.user.id)
        .await?
        .into_iter()
        .filter(|r| r.is_active(now, SESSION_IDLE_SECS))
        .map(|r| SessionView {
            current: r.id == cu.session.id,
            id: r.id,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
            user_agent: r.user_agent,
            ip: r.ip,
        })
        .collect();
    Ok(Json(views))
}

#[utoipa::path(
    delete,
    path = "/auth/sessions/{id}",
    operation_id = "revokeSession",
    tag = "auth",
    summary = "Log out one of your sessions",
    params(("id" = i64, Path, description = "Session id")),
    responses((status = 204, description = "Revoked"))
)]
pub async fn revoke_session(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    if !s.store.delete_user_session(cu.user.id, id).await? {
        return Err(Error::NotFound(format!("session {id}")).into());
    }
    audit(
        &s,
        Some(cu.user.id),
        "auth.session.revoke",
        "session",
        &id.to_string(),
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

#[utoipa::path(
    put,
    path = "/auth/password",
    operation_id = "changePassword",
    tag = "auth",
    summary = "Change your password and end your other sessions",
    request_body = ChangePassword,
    responses((status = 204, description = "Changed"))
)]
pub async fn change_password(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiJson(body): ApiJson<ChangePassword>,
) -> ApiResult<StatusCode> {
    let (_, hash) = s
        .store
        .find_credentials(&cu.user.username)
        .await?
        .ok_or_else(|| Error::Unauthorized("authentication required".into()))?;
    if body.current_password.chars().count() > PASSWORD_MAX
        || !password::verify_password(body.current_password, hash).await
    {
        return Err(Error::Invalid("current password is incorrect".into()).into());
    }
    validate_password(&body.new_password)?;
    let new_hash = password::hash_password(body.new_password).await?;
    s.store
        .set_password_hash(cu.user.id, &new_hash, now_unix())
        .await?;
    s.store
        .delete_user_sessions(cu.user.id, Some(cu.session.id))
        .await?;
    audit(
        &s,
        Some(cu.user.id),
        "auth.password.change",
        "user",
        &cu.user.id.to_string(),
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
