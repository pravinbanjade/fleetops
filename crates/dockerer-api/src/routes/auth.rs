use crate::{
    auth::{
        cookie::{clear_cookie, session_cookie},
        extract::{ClientMeta, CurrentUser},
        limiter::limiter_key,
        password, token, SESSION_ABSOLUTE_SECS,
    },
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dockerer_core::{now_unix, validate_password, validate_username, Error, User};
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
    Json(body): Json<Credentials>,
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
    Json(body): Json<Credentials>,
) -> ApiResult<Response> {
    let key = limiter_key(&body.username, &meta.ip);
    let now = Instant::now();
    s.limiter.check(&key, now)?;

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
        s.limiter.record_failure(&key, now);
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
