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
use dockerer_core::{
    now_unix, user::PASSWORD_MAX, user::USERNAME_MAX, validate_password, validate_username, Error,
    User,
};
use dockerer_store::NewSession;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub user: User,
    pub csrf_token: String,
}

pub async fn status(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let users = s.store.count_users().await?;
    Ok(Json(json!({
        "setup_required": users == 0,
        "setup_open": users == 0 && s.setup.is_open(Instant::now()),
    })))
}

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
            "setup window expired; restart dockerer to create the admin account".into(),
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
    start_session(&s, user, &meta, StatusCode::CREATED).await
}

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

    let verified = match s.store.find_credentials(&body.username).await? {
        Some((user, hash)) => password::verify_password(body.password, hash)
            .await
            .then_some(user),
        None => {
            password::burn_verify(body.password).await;
            None
        }
    };
    let Some(user) = verified else {
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
    start_session(&s, user, &meta, StatusCode::OK).await
}

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

pub async fn me(cu: CurrentUser) -> Json<SessionInfo> {
    Json(SessionInfo {
        user: cu.user,
        csrf_token: cu.session.csrf_token,
    })
}

async fn start_session(
    s: &AppState,
    user: User,
    meta: &ClientMeta,
    status: StatusCode,
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
            expires_at: now + SESSION_ABSOLUTE_SECS,
            user_agent: meta.user_agent.as_deref(),
            ip: Some(&meta.ip),
        })
        .await?;
    let mut res = (status, Json(SessionInfo { user, csrf_token })).into_response();
    res.headers_mut().insert(
        SET_COOKIE,
        session_cookie(&session_token, SESSION_ABSOLUTE_SECS, meta.tls),
    );
    Ok(res)
}

#[derive(Serialize)]
pub struct SessionView {
    pub id: i64,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub current: bool,
}

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

#[derive(Deserialize)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

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
