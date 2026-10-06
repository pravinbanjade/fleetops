//! One websocket connection: a reader that handles client frames and owns
//! the subscriptions, and a writer that drains every subscription's queue
//! fairly onto the socket.

use super::protocol::{self, decode_binary, ClientMsg, Out, Topic};
use super::queue::{Policy, SubQueue};
use super::topics;
use crate::{
    auth::{extract::CurrentUser, SESSION_IDLE_SECS, SESSION_TOUCH_SECS},
    AppState,
};
use axum::extract::ws::{Message, WebSocket};
use fleetops_core::docker::DockerOps;
use fleetops_core::{now_unix, Error, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{broadcast::error::RecvError, mpsc, Notify};

/// Subscriptions one connection may hold at once.
pub const MAX_SUBS: usize = 256;
/// Largest client frame accepted.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// How a terminal resize reaches the process.
pub(super) enum Resize {
    Docker {
        ops: Arc<dyn DockerOps>,
        exec_id: String,
    },
    Kube(mpsc::Sender<(u16, u16)>),
}

pub(super) struct ExecCtl {
    pub input: mpsc::Sender<Vec<u8>>,
    pub resize: Resize,
}

/// A live subscription. Dropping it (unsub, or the connection closing)
/// stops its producer.
pub(super) struct Sub {
    pub queue: Arc<SubQueue>,
    pub task: tokio::task::AbortHandle,
    pub exec: Option<ExecCtl>,
}

impl Drop for Sub {
    fn drop(&mut self) {
        self.queue.close();
        self.task.abort();
    }
}

/// State shared by reader and writer.
struct Shared {
    subs: Mutex<HashMap<u32, Sub>>,
    /// Connection-level messages: errors and env_status.
    control: Arc<SubQueue>,
    ready: Arc<Notify>,
    closing: AtomicBool,
}

impl Shared {
    fn say(&self, msg: Out) {
        self.control.push(msg, 1);
    }

    fn fail(&self, sub: Option<u32>, e: &Error) {
        self.say(protocol::error(sub, e.code(), &e.to_string()));
    }
}

pub async fn run(s: AppState, cu: CurrentUser, token_hash: String, socket: WebSocket) {
    let (mut sink, mut incoming) = socket.split();
    let ready = Arc::new(Notify::new());
    let shared = Arc::new(Shared {
        subs: Mutex::new(HashMap::new()),
        control: Arc::new(SubQueue::new(0, Policy::DropOldest, 256, ready.clone())),
        ready,
        closing: AtomicBool::new(false),
    });

    let gone = Arc::new(Notify::new());
    let writer = {
        let shared = shared.clone();
        let gone = gone.clone();
        tokio::spawn(async move {
            write_loop(&shared, &mut sink).await;
            gone.notify_one();
        })
    };

    let mut status = s.hubs.status().subscribe();
    let mut access = s.hubs.access().subscribe();
    let mut check = tokio::time::interval(s.config.ws_session_check);
    check.tick().await;
    let ping_every = s.config.ws_ping_interval;
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            frame = incoming.next() => {
                if matches!(frame, Some(Ok(_))) {
                    last_heard = tokio::time::Instant::now();
                }
                match frame {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(msg) => handle(&s, &cu, &shared, msg).await,
                    Err(e) => shared.fail(None, &Error::Invalid(format!("bad frame: {e}"))),
                },
                Some(Ok(Message::Binary(b))) => match decode_binary(&b) {
                    Some((sub, payload)) => input(&shared, sub, payload.to_vec()),
                    None => shared.fail(None, &Error::Invalid("binary frame shorter than 4 bytes".into())),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }},
            change = status.recv() => match change {
                Ok(c) => {
                    let visible = match s.envs.get(&c.env) {
                        Ok(env) => s.store.env_grant(cu.user.id, env.id()).await.ok().flatten().is_some(),
                        Err(_) => false,
                    };
                    if visible {
                        shared.say(protocol::env_status(&c.env, &c.status));
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {}
            },
            changed = access.recv() => {
                let mine = match changed {
                    Ok(uid) => uid == cu.user.id,
                    // Missed some: assume one was ours.
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => false,
                };
                if mine {
                    for (sub, entry) in shared.subs.lock().unwrap().iter() {
                        entry.task.abort();
                        entry.queue.finish(protocol::end(
                            *sub,
                            "access",
                            serde_json::json!({ "message": super::ACCESS_CHANGED }),
                        ));
                    }
                    shared.say(protocol::access_changed());
                }
            },
            _ = ping.tick() => {
                // Half-open sockets (a laptop that slept) never answer; drop
                // them so they stop holding Docker streams and the session.
                if last_heard.elapsed() > ping_every * 2 {
                    break;
                }
                shared.say(Out::Ping);
            }
            _ = check.tick() => {
                if !session_still_valid(&s, &token_hash, cu.user.id).await {
                    shared.fail(None, &Error::Unauthorized("session ended".into()));
                    break;
                }
            }
            // The writer stopped: the socket is gone.
            _ = gone.notified() => break,
        }
    }
    // Stop producers, flush what is queued for the client, then close.
    shared.subs.lock().unwrap().clear();
    shared.closing.store(true, Ordering::SeqCst);
    shared.ready.notify_one();
    let abort = writer.abort_handle();
    if tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .is_err()
    {
        abort.abort();
    }
}

/// Sends queued messages until the socket fails or the connection closes.
async fn write_loop<S>(shared: &Shared, sink: &mut S)
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    loop {
        let wake = shared.ready.notified();
        let mut sent = false;
        if let Some(m) = shared.control.pop() {
            if send(sink, m).await.is_err() {
                return;
            }
            sent = true;
        }
        // One message per subscription per round keeps it fair.
        let queues: Vec<_> = shared
            .subs
            .lock()
            .unwrap()
            .values()
            .map(|s| s.queue.clone())
            .collect();
        for q in queues {
            if let Some(m) = q.pop() {
                if send(sink, m).await.is_err() {
                    return;
                }
                sent = true;
            }
            if q.drained() {
                let mut subs = shared.subs.lock().unwrap();
                if subs
                    .get(&q.sub())
                    .is_some_and(|s| Arc::ptr_eq(&s.queue, &q))
                {
                    subs.remove(&q.sub());
                }
            }
        }
        if !sent {
            if shared.closing.load(Ordering::SeqCst) {
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
            wake.await;
        }
    }
}

async fn send<S>(sink: &mut S, m: Out) -> std::result::Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    sink.send(match m {
        Out::Text(t) => Message::Text(t.into()),
        Out::Binary(b) => Message::Binary(b.into()),
        Out::Ping => Message::Ping(Vec::new().into()),
    })
    .await
}

/// Re-checks the session behind a long-lived connection, so logging out
/// or revoking a session also ends its websockets. Keeps the session from
/// idling out while a tab is open.
async fn session_still_valid(s: &AppState, token_hash: &str, user_id: i64) -> bool {
    let now = now_unix();
    let Ok(Some(session)) = s.store.find_session(token_hash).await else {
        return false;
    };
    if session.user_id != user_id || !session.is_active(now, SESSION_IDLE_SECS) {
        return false;
    }
    if now - session.last_seen_at >= SESSION_TOUCH_SECS {
        let _ = s.store.touch_session(session.id, now).await;
    }
    s.store.get_user(user_id).await.is_ok()
}

fn input(shared: &Shared, sub: u32, data: Vec<u8>) {
    let subs = shared.subs.lock().unwrap();
    match subs.get(&sub).and_then(|s| s.exec.as_ref()) {
        Some(ctl) => {
            if ctl.input.try_send(data).is_err() {
                shared.fail(
                    Some(sub),
                    &Error::Conflict("terminal input is backed up".into()),
                );
            }
        }
        None => shared.fail(
            Some(sub),
            &Error::NotFound(format!("no exec subscription {sub}")),
        ),
    }
}

async fn handle(s: &AppState, cu: &CurrentUser, shared: &Arc<Shared>, msg: ClientMsg) {
    match msg {
        ClientMsg::Sub {
            sub,
            topic,
            env,
            id,
            opts,
        } => {
            {
                let subs = shared.subs.lock().unwrap();
                if subs.contains_key(&sub) {
                    drop(subs);
                    return shared
                        .fail(Some(sub), &Error::Conflict(format!("sub {sub} is in use")));
                }
                if subs.len() >= MAX_SUBS {
                    drop(subs);
                    return shared.fail(
                        Some(sub),
                        &Error::Conflict(format!(
                            "at most {MAX_SUBS} subscriptions per connection"
                        )),
                    );
                }
            }
            match start(s, cu, shared, sub, (topic, env, id, opts)).await {
                Ok(entry) => {
                    shared.subs.lock().unwrap().insert(sub, entry);
                }
                Err(e) => shared.fail(Some(sub), &e),
            }
        }
        ClientMsg::Unsub { sub } => {
            shared.subs.lock().unwrap().remove(&sub);
        }
        ClientMsg::Input { sub, data } => input(shared, sub, data.into_bytes()),
        ClientMsg::Resize { sub, cols, rows } => {
            enum Do {
                Docker(Arc<dyn DockerOps>, String),
                Kube(mpsc::Sender<(u16, u16)>),
            }
            let target = shared.subs.lock().unwrap().get(&sub).and_then(|s| {
                s.exec.as_ref().map(|c| match &c.resize {
                    Resize::Docker { ops, exec_id } => Do::Docker(ops.clone(), exec_id.clone()),
                    Resize::Kube(tx) => Do::Kube(tx.clone()),
                })
            });
            match target {
                Some(Do::Docker(ops, exec_id)) => {
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ops.exec_resize(&exec_id, cols, rows).await {
                            shared.fail(Some(sub), &e);
                        }
                    });
                }
                // A full channel means resizes are arriving faster than the
                // process takes them; the next one supersedes this one.
                Some(Do::Kube(tx)) => {
                    let _ = tx.try_send((cols, rows));
                }
                None => shared.fail(
                    Some(sub),
                    &Error::NotFound(format!("no exec subscription {sub}")),
                ),
            }
        }
    }
}

async fn start(
    s: &AppState,
    cu: &CurrentUser,
    shared: &Shared,
    sub: u32,
    msg: (Topic, Option<String>, Option<String>, Value),
) -> Result<Sub> {
    let (topic, env, id, opts) = msg;
    let c = topics::Ctx {
        s,
        cu,
        sub,
        ready: shared.ready.clone(),
    };
    match topic {
        Topic::Events => topics::events(&c, env, id).await,
        Topic::Stats => topics::stats(&c, env, id).await,
        Topic::Logs => topics::logs(&c, env, id, opts).await,
        Topic::Exec => topics::exec(&c, env, id, opts).await,
        Topic::Task => topics::task(&c, id).await,
        Topic::KubeWatch => topics::kube_watch(&c, env, id).await,
    }
}
