//! `/api/v1/ws`: one multiplexed websocket per browser tab (spec §5.2).

pub mod conn;
/// Why a user's subscriptions end when their access changes.
pub const ACCESS_CHANGED: &str = "your access changed";

pub mod hub;
pub mod protocol;
pub mod queue;
pub mod topics;
pub mod swarm_tasks;
pub mod watches;

use crate::{
    auth::{cookie::read_cookie, cookie::SESSION_COOKIE, extract::CurrentUser, token::hash_token},
    extract::rejection,
    ApiError, AppState,
};
use axum::{
    extract::{ws::rejection::WebSocketUpgradeRejection, State, WebSocketUpgrade},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};
use fleetops_core::Error;

/// Browsers always send `Origin` on websocket requests; it must name this
/// server, so other sites cannot ride the session cookie. Non-browser
/// clients may omit it.
pub fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin.to_str().ok().and_then(|o| o.split_once("://")), host) {
        (Some((_, authority)), Some(host)) => authority.eq_ignore_ascii_case(host),
        _ => false,
    }
}

#[utoipa::path(
    get,
    path = "/ws",
    operation_id = "openWebsocket",
    tag = "websocket",
    summary = "Open the real-time websocket",
    responses((status = 101, description = "Switching protocols. Text frames are `ClientMsg` from the client and `ServerMsg` from the server; exec I/O uses binary frames `[u32 sub id, big-endian][bytes]`."))
)]
pub async fn handler(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !origin_allowed(&headers) {
        return ApiError(Error::Forbidden(
            "websocket Origin does not match this server".into(),
        ))
        .into_response();
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(r) => return rejection(r.status(), r.body_text()),
    };
    // CurrentUser already validated this cookie.
    let token_hash = read_cookie(&headers, SESSION_COOKIE)
        .map(|t| hash_token(&t))
        .unwrap_or_default();
    ws.max_message_size(conn::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| conn::run(s, cu, token_hash, socket))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, v.parse().unwrap());
        }
        m
    }

    #[test]
    fn origin_must_match_host() {
        assert!(origin_allowed(&h(&[("host", "fleet.lan:9000")])));
        assert!(origin_allowed(&h(&[
            ("host", "fleet.lan:9000"),
            ("origin", "http://fleet.lan:9000")
        ])));
        assert!(origin_allowed(&h(&[
            ("host", "Fleet.LAN:9443"),
            ("origin", "https://fleet.lan:9443")
        ])));
        for bad in [
            "http://evil.example",
            "http://fleet.lan:9001",
            "null",
            "fleet.lan:9000",
        ] {
            assert!(
                !origin_allowed(&h(&[("host", "fleet.lan:9000"), ("origin", bad)])),
                "{bad}"
            );
        }
        assert!(!origin_allowed(&h(&[("origin", "http://fleet.lan:9000")])));
    }
}
