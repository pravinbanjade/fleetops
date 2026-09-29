use super::{
    cookie::{read_cookie, SESSION_COOKIE},
    token::{constant_time_eq, hash_token},
    CSRF_HEADER, SESSION_IDLE_SECS, SESSION_TOUCH_SECS,
};
use crate::{ApiError, AppState, IsTls};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header::USER_AGENT, request::Parts, Method},
};
use fleetops_core::{now_unix, Error, Role, User};
use fleetops_store::SessionRecord;
use std::{convert::Infallible, net::SocketAddr};

/// Connection metadata recorded on sessions and used for rate limiting.
pub struct ClientMeta {
    pub ip: String,
    pub user_agent: Option<String>,
    pub tls: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for ClientMeta {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        let user_agent = parts
            .headers
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.chars().take(256).collect());
        let tls = parts.extensions.get::<IsTls>().is_some_and(|t| t.0);
        Ok(ClientMeta {
            ip,
            user_agent,
            tls,
        })
    }
}

/// An authenticated user. Extracting this enforces CSRF on every method
/// other than GET/HEAD/OPTIONS, so handlers cannot forget it.
pub struct CurrentUser {
    pub user: User,
    pub session: SessionRecord,
}

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn unauthorized() -> ApiError {
    Error::Unauthorized("authentication required".into()).into()
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = read_cookie(&parts.headers, SESSION_COOKIE).ok_or_else(unauthorized)?;
        let session = state
            .store
            .find_session(&hash_token(&token))
            .await?
            .ok_or_else(unauthorized)?;
        let now = now_unix();
        if !session.is_active(now, SESSION_IDLE_SECS) {
            state.store.delete_session(session.id).await?;
            return Err(unauthorized());
        }
        if !is_safe_method(&parts.method) {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !constant_time_eq(sent, &session.csrf_token) {
                return Err(Error::Forbidden("missing or invalid CSRF token".into()).into());
            }
        }
        if now - session.last_seen_at >= SESSION_TOUCH_SECS {
            state.store.touch_session(session.id, now).await?;
        }
        let user = match state.store.get_user(session.user_id).await {
            Ok(u) => u,
            Err(Error::NotFound(_)) => return Err(unauthorized()),
            Err(e) => return Err(e.into()),
        };
        Ok(CurrentUser { user, session })
    }
}

pub struct AdminUser(pub CurrentUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let cu = CurrentUser::from_request_parts(parts, state).await?;
        if cu.user.role != Role::Admin {
            return Err(Error::Forbidden("admin role required".into()).into());
        }
        Ok(AdminUser(cu))
    }
}
