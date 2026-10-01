//! `GET /api/v1/agent/connect`: authenticates an agent, upgrades to a
//! WebSocket, and runs the server side of its tunnel (sub-project 2 spec
//! §4.2, §4.3).

use super::{axum_io, new_agent_key, publish_status};
use crate::{
    auth::{extract::ClientMeta, token::hash_token},
    routes::audit,
    ApiError, AppState,
};
use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    http::{header::AUTHORIZATION, HeaderMap},
    response::{IntoResponse, Response},
};
use fleetops_core::{now_unix, Error};
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, Msg, PROTO_MAJOR},
    registry::AgentInfo,
    ws::MAX_WS_MESSAGE,
    Role, Session, StreamHandle,
};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    sync::{mpsc, oneshot},
};

const HANDSHAKE: Duration = Duration::from_secs(10);
/// At most one last-seen write per this long while a session is up.
const SEEN_EVERY: Duration = Duration::from_secs(60);
/// At most one `agent.disconnect` audit row per environment per this long.
const DISCONNECT_AUDIT_EVERY: Duration = Duration::from_secs(300);

enum Credential {
    Join { env_id: i64, join_hash: String },
    Key { env_id: i64 },
}

impl Credential {
    fn env_id(&self) -> i64 {
        match self {
            Credential::Join { env_id, .. } | Credential::Key { env_id } => *env_id,
        }
    }
}

async fn authenticate(s: &AppState, headers: &HeaderMap) -> Result<Credential, Error> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| Error::Unauthorized("agent credential required".into()))?;
    let hash = hash_token(bearer);
    if bearer.starts_with("fj_") {
        if let Some(env_id) = s.store.agent_by_join_token(&hash, now_unix()).await? {
            return Ok(Credential::Join {
                env_id,
                join_hash: hash,
            });
        }
    } else if bearer.starts_with("fk_") {
        if let Some(env_id) = s.store.agent_by_key(&hash).await? {
            return Ok(Credential::Key { env_id });
        }
    }
    Err(Error::Unauthorized(
        "unknown or expired agent credential".into(),
    ))
}

#[utoipa::path(
    get,
    path = "/agent/connect",
    operation_id = "agentConnect",
    tag = "agents",
    summary = "Agent tunnel (WebSocket upgrade, Bearer fj_… or fk_…). Not for browsers.",
    responses(
        (status = 101, description = "Tunnel open"),
        (status = 401, description = "Missing, unknown or expired credential", body = crate::error::ErrorBody),
        (status = 403, description = "Plain HTTP from a non-loopback address", body = crate::error::ErrorBody),
        (status = 429, description = "Too many failed attempts", body = crate::error::ErrorBody)
    )
)]
pub async fn handler(
    State(s): State<AppState>,
    meta: ClientMeta,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let loopback = meta.ip.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !meta.tls && !loopback {
        return ApiError::from(Error::Forbidden("agents must connect over HTTPS".into()))
            .into_response();
    }
    let limit_key = format!("agent|{}", meta.ip);
    if let Err(e) = s.limiter.try_acquire(&limit_key, Instant::now()) {
        return ApiError::from(e).into_response();
    }
    let cred = match authenticate(&s, &headers).await {
        Ok(c) => c,
        Err(e) => return ApiError::from(e).into_response(),
    };
    s.limiter.reset(&limit_key);
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run(s, cred, socket))
}

/// The agent's control stream, split.
struct Control {
    rx: ReadHalf<StreamHandle>,
    tx: WriteHalf<StreamHandle>,
}

async fn run(s: AppState, cred: Credential, socket: WebSocket) {
    let (first_tx, first_rx) = oneshot::channel();
    let mut first_tx = Some(first_tx);
    // The agent opens exactly one stream, the control stream. Any other
    // stream it opens is dropped, which resets it.
    let session = Session::spawn(axum_io::io(socket), Role::Server, move |stream| {
        if let Some(tx) = first_tx.take() {
            let _ = tx.send(stream);
        }
    });
    let env_id = cred.env_id();
    let reason = serve(&s, cred, &session, first_rx).await;
    tracing::info!(env_id, %reason, "agent session ended");
    session.close().await;
    publish_status(&s, env_id).await;
}

async fn handshake(
    first: oneshot::Receiver<StreamHandle>,
) -> Result<(Control, AgentInfo, u16), String> {
    let stream = tokio::time::timeout(HANDSHAKE, first)
        .await
        .map_err(|_| "no control stream".to_string())?
        .map_err(|_| "session closed before the control stream".to_string())?;
    let (rx, tx) = tokio::io::split(stream);
    let mut c = Control { rx, tx };
    match tokio::time::timeout(HANDSHAKE, read_msg(&mut c.rx)).await {
        Ok(Ok(Some(Msg::Hello {
            proto,
            agent_version,
            host,
        }))) => Ok((
            c,
            AgentInfo {
                agent_version,
                host,
                connected_at: now_unix(),
            },
            proto,
        )),
        other => Err(format!("expected hello, got {other:?}")),
    }
}

/// Runs one agent session to its end; returns why it ended.
async fn serve(
    s: &AppState,
    cred: Credential,
    session: &Session,
    first: oneshot::Receiver<StreamHandle>,
) -> String {
    let (mut c, info, proto) = match handshake(first).await {
        Ok(h) => h,
        Err(reason) => return reason,
    };
    let (env_id, join_hash) = match cred {
        Credential::Join { env_id, join_hash } => (env_id, Some(join_hash)),
        Credential::Key { env_id } => (env_id, None),
    };
    let env = match s.store.get_environment(env_id).await {
        Ok(env) => env,
        Err(e) => return e.to_string(),
    };
    let host_json = serde_json::to_string(&info.host).ok();

    if proto != PROTO_MAJOR {
        let reason = format!(
            "agent v{} uses protocol {proto}; server needs {PROTO_MAJOR}",
            info.agent_version
        );
        let _ = write_msg(
            &mut c.tx,
            &Msg::Bye {
                reason: ByeReason::Incompatible,
            },
        )
        .await;
        let _ = s
            .store
            .record_agent_seen(
                env_id,
                now_unix(),
                Some(&info.agent_version),
                host_json.as_deref(),
                Some(&reason),
            )
            .await;
        s.agents.set_offline_reason(env_id, &reason);
        audit(s, None, "agent.reject", "environment", &env.name, "failure").await;
        return reason;
    }

    // Enrollment: the join token is spent only now, after a compatible hello.
    let agent_key = match join_hash {
        Some(join_hash) => {
            let key = new_agent_key();
            match s
                .store
                .enroll_agent(env_id, &join_hash, &hash_token(&key), now_unix())
                .await
            {
                Ok(true) => {}
                Ok(false) => return "join token already used".into(),
                Err(e) => return e.to_string(),
            }
            audit(s, None, "agent.enroll", "environment", &env.name, "success").await;
            Some(key)
        }
        None => None,
    };
    let welcome = Msg::Welcome {
        env_name: env.name.clone(),
        server_version: env!("CARGO_PKG_VERSION").into(),
        agent_key,
    };
    if let Err(e) = write_msg(&mut c.tx, &welcome).await {
        return e.to_string();
    }

    let (bye_tx, mut bye_rx) = mpsc::channel(1);
    let id = s
        .agents
        .insert(env_id, session.clone(), info.clone(), bye_tx);
    let _ = s
        .store
        .record_agent_seen(
            env_id,
            now_unix(),
            Some(&info.agent_version),
            host_json.as_deref(),
            None,
        )
        .await;
    audit(
        s,
        None,
        "agent.connect",
        "environment",
        &env.name,
        "success",
    )
    .await;
    publish_status(s, env_id).await;

    let reason = control_loop(s, env_id, &mut c, session, &mut bye_rx).await;

    s.agents.remove(env_id, id, &reason);
    let _ = s
        .store
        .record_agent_seen(env_id, now_unix(), None, None, None)
        .await;
    if disconnect_audit_due(s, env_id) {
        audit(
            s,
            None,
            "agent.disconnect",
            "environment",
            &env.name,
            "success",
        )
        .await;
    }
    reason
}

fn disconnect_audit_due(s: &AppState, env_id: i64) -> bool {
    let mut last = s.agent_disconnects.lock().unwrap();
    let now = Instant::now();
    match last.get(&env_id) {
        Some(t) if now.duration_since(*t) < DISCONNECT_AUDIT_EVERY => false,
        _ => {
            last.insert(env_id, now);
            true
        }
    }
}

/// Runs until the session ends; returns why, for the environment's status.
async fn control_loop(
    s: &AppState,
    env_id: i64,
    c: &mut Control,
    session: &Session,
    bye_rx: &mut mpsc::Receiver<ByeReason>,
) -> String {
    let mut ping = tokio::time::interval(s.config.agent_ping_interval);
    ping.tick().await; // the first tick is immediate
    let mut seen = tokio::time::interval(SEEN_EVERY);
    seen.tick().await;
    let mut n = 0u64;
    let mut unanswered = 0u32;
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if unanswered >= 2 {
                    return "no answer to two pings".into();
                }
                n += 1;
                unanswered += 1;
                if write_msg(&mut c.tx, &Msg::Ping { n }).await.is_err() {
                    return "connection lost".into();
                }
            }
            msg = read_msg(&mut c.rx) => match msg {
                Ok(Some(Msg::Pong { .. })) => unanswered = 0,
                Ok(Some(Msg::Bye { reason })) => return format!("agent said bye ({reason:?})"),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return "connection lost".into(),
            },
            Some(reason) = bye_rx.recv() => {
                let _ = write_msg(&mut c.tx, &Msg::Bye { reason }).await;
                return match reason {
                    ByeReason::Replaced => "replaced by a newer connection".into(),
                    ByeReason::Revoked => "key revoked".into(),
                    other => format!("{other:?}"),
                };
            }
            _ = session.closed() => return "connection lost".into(),
            _ = seen.tick() => {
                // Fails only if the environment was deleted; the bye follows.
                let _ = s.store.record_agent_seen(env_id, now_unix(), None, None, None).await;
            }
        }
    }
}
