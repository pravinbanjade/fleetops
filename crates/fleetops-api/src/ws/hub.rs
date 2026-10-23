//! Shared Docker streams (spec §5.2 fan-out): one events stream per
//! environment and one stats stream per container, however many clients
//! watch them. Each stops when its last subscriber leaves.

use fleetops_core::env::{EnvStatus, Environment};
use fleetops_core::streams::{DockerEvent, StatsSample};
use futures_util::StreamExt;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::broadcast;

pub const EVENTS_BUFFER: usize = 1024;
pub const STATS_BUFFER: usize = 16;
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How often an idle events stream checks whether anyone still listens.
const IDLE_CHECK: Duration = Duration::from_secs(1);

/// An environment's status changed; every connection forwards it.
#[derive(Debug, Clone)]
pub struct StatusChange {
    pub env: String,
    pub status: Value,
}

pub struct EventHub {
    tx: broadcast::Sender<DockerEvent>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl EventHub {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(EVENTS_BUFFER).0,
            task: Mutex::new(None),
        }
    }

    /// Subscribes to `env`'s events, starting the shared stream if needed.
    pub fn subscribe(
        &self,
        env: Arc<dyn Environment>,
        status: broadcast::Sender<StatusChange>,
        backoff: Duration,
    ) -> broadcast::Receiver<DockerEvent> {
        let rx = self.tx.subscribe();
        let mut task = self.task.lock().unwrap();
        if task.as_ref().is_none_or(|t| t.is_finished()) {
            *task = Some(tokio::spawn(run_events(
                env,
                self.tx.clone(),
                status,
                backoff,
            )));
        }
        rx
    }
}

fn status_json(s: &EnvStatus) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// Sleeps out a reconnect backoff, or less if the environment says its
/// reachability changed (an agent reconnected).
async fn backoff_sleep(env: &Arc<dyn Environment>, d: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = env.wait_change() => {}
    }
}

/// Keeps one events stream open while anyone listens, reconnecting with
/// exponential backoff and announcing up/down transitions.
async fn run_events(
    env: Arc<dyn Environment>,
    tx: broadcast::Sender<DockerEvent>,
    status: broadcast::Sender<StatusChange>,
    initial_backoff: Duration,
) {
    let mut backoff = initial_backoff;
    // Assume up: the first failure is a transition worth announcing.
    let mut up = true;
    let announce = |s: &EnvStatus| {
        let _ = status.send(StatusChange {
            env: env.name().to_string(),
            status: status_json(s),
        });
    };
    // One interval for the whole task: a sleep recreated on every event
    // would never fire on a busy host.
    let mut idle = tokio::time::interval(IDLE_CHECK);
    idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while tx.receiver_count() > 0 {
        let current = env.status().await;
        // A Docker host's own stream, or a swarm's serving manager's.
        let source = env
            .docker()
            .map(|d| d.events())
            .or_else(|| env.swarm().map(|s| s.events()));
        let mut events = match (&current, source) {
            (EnvStatus::Up { .. }, Some(events)) => events,
            _ => {
                if up {
                    up = false;
                    announce(&current);
                }
                backoff_sleep(&env, backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        if !up {
            up = true;
            announce(&current);
        }
        // Connected again: the next drop backs off from the start.
        backoff = initial_backoff;
        loop {
            tokio::select! {
                item = events.next() => match item {
                    Some(Ok(e)) => {
                        // Fails only when nobody listens any more.
                        if tx.send(e).is_err() {
                            return;
                        }
                    }
                    // Dropped: go round and reconnect.
                    Some(Err(_)) | None => break,
                },
                _ = idle.tick() => {
                    if tx.receiver_count() == 0 {
                        return;
                    }
                }
            }
        }
        // Every drop is a transition, even when Docker is back before the
        // next status check: events in the gap are lost, so clients must
        // hear down/up to know to refetch.
        if up {
            up = false;
            announce(&EnvStatus::Down {
                reason: "Docker event stream dropped; reconnecting".into(),
            });
        }
        if tx.receiver_count() > 0 {
            backoff_sleep(&env, backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

/// One container's shared stats stream. `None` on the channel means the
/// stream ended (the container stopped or the daemon went away).
pub struct SharedStats {
    tx: broadcast::Sender<Option<StatsSample>>,
    task: tokio::task::AbortHandle,
}

impl Drop for SharedStats {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SharedStats {
    pub fn subscribe(&self) -> broadcast::Receiver<Option<StatsSample>> {
        self.tx.subscribe()
    }
}

#[derive(Default)]
pub struct StatsHub {
    streams: Mutex<HashMap<String, Weak<SharedStats>>>,
}

impl StatsHub {
    /// A handle to `id`'s shared stats stream plus a receiver on it. The
    /// stream lives while any handle does.
    pub fn subscribe(
        &self,
        ops: Arc<dyn fleetops_core::docker::DockerOps>,
        id: &str,
    ) -> (Arc<SharedStats>, broadcast::Receiver<Option<StatsSample>>) {
        let mut map = self.streams.lock().unwrap();
        map.retain(|_, w| w.strong_count() > 0);
        if let Some(shared) = map.get(id).and_then(Weak::upgrade) {
            if !shared.task.is_finished() {
                let rx = shared.subscribe();
                return (shared, rx);
            }
        }
        let (tx, rx) = broadcast::channel(STATS_BUFFER);
        let producer = tx.clone();
        let mut stream = ops.stats(id);
        let task = tokio::spawn(async move {
            while let Some(Ok(sample)) = stream.next().await {
                let _ = producer.send(Some(sample));
            }
            let _ = producer.send(None);
        })
        .abort_handle();
        let shared = Arc::new(SharedStats { tx, task });
        map.insert(id.to_string(), Arc::downgrade(&shared));
        (shared, rx)
    }

    /// Open shared streams, for tests and metrics.
    pub fn open(&self) -> usize {
        self.streams
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.upgrade().is_some_and(|s| !s.task.is_finished()))
            .count()
    }
}

/// Identifies one live environment instance. Re-creating an environment
/// under the same name, or changing its URL, builds a new instance, so its
/// subscribers get a fresh Docker connection rather than the old one.
pub(crate) type EnvKey = (i64, usize);

pub(crate) fn key(env: &Arc<dyn Environment>) -> EnvKey {
    (env.id(), Arc::as_ptr(env) as *const () as usize)
}

/// Per-environment hubs.
#[derive(Default)]
pub struct Hubs {
    events: Mutex<HashMap<EnvKey, Arc<EventHub>>>,
    stats: Mutex<HashMap<EnvKey, Arc<StatsHub>>>,
    status: Mutex<Option<broadcast::Sender<StatusChange>>>,
    /// User ids whose access changed (spec §3.3).
    access: Mutex<Option<broadcast::Sender<i64>>>,
    /// Shared Kubernetes watches (spec §2.4).
    pub kube_watches: Arc<crate::ws::watches::Watches>,
    /// Shared Swarm task feeds (swarm spec §7).
    pub swarm_tasks: crate::ws::swarm_tasks::SwarmTasks,
}

impl Hubs {
    pub fn events(&self, env: &Arc<dyn Environment>) -> Arc<EventHub> {
        let mut map = self.events.lock().unwrap();
        // Forget hubs nobody listens to any more.
        map.retain(|_, h| h.tx.receiver_count() > 0);
        map.entry(key(env))
            .or_insert_with(|| Arc::new(EventHub::new()))
            .clone()
    }

    pub fn stats(&self, env: &Arc<dyn Environment>) -> Arc<StatsHub> {
        let mut map = self.stats.lock().unwrap();
        map.retain(|_, h| h.open() > 0);
        map.entry(key(env)).or_default().clone()
    }

    /// The bus that tells connections a user's access changed.
    pub fn access(&self) -> broadcast::Sender<i64> {
        self.access
            .lock()
            .unwrap()
            .get_or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }

    /// Ends `user_id`'s subscriptions on every connection they have open.
    pub fn access_changed(&self, user_id: i64) {
        let _ = self.access().send(user_id);
    }

    /// The bus every connection listens on for `env_status`.
    pub fn status(&self) -> broadcast::Sender<StatusChange> {
        self.status
            .lock()
            .unwrap()
            .get_or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }
}
