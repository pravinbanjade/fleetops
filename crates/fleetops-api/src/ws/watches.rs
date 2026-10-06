//! Shared Kubernetes watches (spec §2.4): one upstream watch per
//! (environment, kind, namespace), however many subscribers, folded into
//! the current rows so a late or lagging subscriber starts from a snapshot.

use fleetops_core::env::Environment;
use fleetops_core::kube::{KubeKind, KubeOps, KubeRow, WatchEvent};
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};

pub const FEED_BUFFER: usize = 256;

#[derive(Debug, Clone)]
enum Feed {
    Event(WatchEvent),
    End(String),
}

#[derive(Default)]
struct State {
    rows: BTreeMap<(Option<String>, String), KubeRow>,
    /// A snapshot has arrived and no resync is in progress.
    ready: bool,
    ended: Option<String>,
    subscribers: usize,
}

impl State {
    fn snapshot(&self) -> WatchEvent {
        WatchEvent::Snapshot {
            rows: self.rows.values().cloned().collect(),
        }
    }

    fn apply(&mut self, ev: &WatchEvent) {
        match ev {
            WatchEvent::Snapshot { rows } => {
                self.rows = rows
                    .iter()
                    .map(|r| ((r.namespace.clone(), r.name.clone()), r.clone()))
                    .collect();
                self.ready = true;
            }
            WatchEvent::Added { row } | WatchEvent::Modified { row } => {
                self.rows
                    .insert((row.namespace.clone(), row.name.clone()), row.clone());
            }
            WatchEvent::Deleted { namespace, name } => {
                self.rows.remove(&(namespace.clone(), name.clone()));
            }
            WatchEvent::Resync => self.ready = false,
        }
    }
}

struct Shared {
    tx: broadcast::Sender<Feed>,
    state: Mutex<State>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Shared {
    fn start(ops: Arc<dyn KubeOps>, kind: KubeKind, ns: Option<String>) -> Arc<Shared> {
        let shared = Arc::new(Shared {
            tx: broadcast::channel(FEED_BUFFER).0,
            state: Mutex::new(State::default()),
            task: Mutex::new(None),
        });
        let s = shared.clone();
        let handle = tokio::spawn(async move {
            let mut stream = ops.watch(kind, ns.as_deref());
            let end = loop {
                match stream.next().await {
                    Some(Ok(ev)) => {
                        // Update and send under one lock, so a subscriber
                        // never sees a snapshot and then an event it already holds.
                        let mut st = s.state.lock().unwrap();
                        st.apply(&ev);
                        let _ = s.tx.send(Feed::Event(ev));
                    }
                    Some(Err(e)) => break e.detail(),
                    None => break "the watch ended".to_string(),
                }
            };
            let mut st = s.state.lock().unwrap();
            st.ended = Some(end.clone());
            let _ = s.tx.send(Feed::End(end));
        });
        *shared.task.lock().unwrap() = Some(handle.abort_handle());
        shared
    }

    fn stop(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }

    fn running(&self) -> bool {
        self.task
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }
}

type Key = (i64, usize, KubeKind, Option<String>);

#[derive(Default)]
pub struct Watches {
    map: Mutex<HashMap<Key, Arc<Shared>>>,
}

impl Watches {
    pub fn subscribe(
        self: &Arc<Self>,
        env: &Arc<dyn Environment>,
        ops: Arc<dyn KubeOps>,
        kind: KubeKind,
        ns: Option<String>,
        grace: Duration,
    ) -> WatchSub {
        // The instance pointer is part of the key: a re-created environment
        // gets a fresh watch, as Docker event hubs do.
        let key = (
            env.id(),
            Arc::as_ptr(env) as *const () as usize,
            kind,
            ns.clone(),
        );
        let mut map = self.map.lock().unwrap();
        let shared = match map.get(&key) {
            Some(s) if s.state.lock().unwrap().ended.is_none() => s.clone(),
            _ => {
                let s = Shared::start(ops, kind, ns);
                map.insert(key.clone(), s.clone());
                s
            }
        };
        let (pending, rx) = {
            let mut st = shared.state.lock().unwrap();
            st.subscribers += 1;
            (st.ready.then(|| st.snapshot()), shared.tx.subscribe())
        };
        WatchSub {
            shared,
            rx,
            pending,
            owner: Arc::downgrade(self),
            key,
            grace,
        }
    }

    /// Upstream watches running, for tests and metrics.
    pub fn open(&self) -> usize {
        self.map
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.running())
            .count()
    }
}

pub struct WatchSub {
    shared: Arc<Shared>,
    rx: broadcast::Receiver<Feed>,
    pending: Option<WatchEvent>,
    owner: Weak<Watches>,
    key: Key,
    grace: Duration,
}

impl WatchSub {
    /// The next event; `Err(message)` once when the watch has failed.
    pub async fn next(&mut self) -> Option<Result<WatchEvent, String>> {
        if let Some(ev) = self.pending.take() {
            return Some(Ok(ev));
        }
        loop {
            match self.rx.recv().await {
                Ok(Feed::Event(ev)) => return Some(Ok(ev)),
                Ok(Feed::End(m)) => return Some(Err(m)),
                Err(RecvError::Lagged(_)) => {
                    let st = self.shared.state.lock().unwrap();
                    self.rx = self.shared.tx.subscribe();
                    if let Some(m) = &st.ended {
                        return Some(Err(m.clone()));
                    }
                    if st.ready {
                        return Some(Ok(st.snapshot()));
                    }
                    // Mid-resync: the coming snapshot is on the new receiver.
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for WatchSub {
    fn drop(&mut self) {
        let left = {
            let mut st = self.shared.state.lock().unwrap();
            st.subscribers -= 1;
            st.subscribers
        };
        if left > 0 {
            return;
        }
        let (owner, key, shared, grace) = (
            self.owner.clone(),
            self.key.clone(),
            self.shared.clone(),
            self.grace,
        );
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let Some(watches) = owner.upgrade() else {
                return;
            };
            let mut map = watches.map.lock().unwrap();
            if shared.state.lock().unwrap().subscribers > 0 {
                return;
            }
            if map.get(&key).is_some_and(|s| Arc::ptr_eq(s, &shared)) {
                map.remove(&key);
            }
            shared.stop();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use fleetops_core::env::{EnvKind, EnvStatus};
    use fleetops_core::kube::*;
    use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions};
    use fleetops_core::{Error, Result};
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct Scripted {
        txs: Mutex<Vec<mpsc::UnboundedSender<Result<WatchEvent>>>>,
    }
    impl Scripted {
        fn send(&self, ev: Result<WatchEvent>) {
            let txs = self.txs.lock().unwrap();
            let tx = txs.last().expect("a watch is open");
            let _ = tx.send(ev);
        }
        fn started(&self) -> usize {
            self.txs.lock().unwrap().len()
        }
        fn open(&self) -> usize {
            self.txs
                .lock()
                .unwrap()
                .iter()
                .filter(|t| !t.is_closed())
                .count()
        }
    }
    /// The upstream watch starts in a spawned task; wait for watch `n`.
    async fn started(ops: &Scripted, n: usize) {
        for _ in 0..200 {
            if ops.started() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("watch {n} never started");
    }

    #[async_trait]
    impl KubeOps for Scripted {
        fn watch(&self, _: KubeKind, _: Option<&str>) -> BoxStream<WatchEvent> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.txs.lock().unwrap().push(tx);
            Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|v| (v, rx))
            }))
        }
        async fn version(&self) -> Result<KubeVersion> {
            unreachable!()
        }
        async fn list(&self, _: KubeKind, _: Option<&str>) -> Result<KubeList> {
            unreachable!()
        }
        async fn get(&self, _: KubeKind, _: &str, _: &str) -> Result<KubeObject> {
            unreachable!()
        }
        async fn events(&self, _: KubeKind, _: &str, _: &str) -> Result<Vec<KubeEvent>> {
            unreachable!()
        }
        async fn delete(&self, _: KubeKind, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        async fn scale(&self, _: KubeKind, _: &str, _: &str, _: u32) -> Result<()> {
            unreachable!()
        }
        async fn restart(&self, _: KubeKind, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn logs(&self, _: &str, _: &str, _: &LogOptions) -> BoxStream<LogLine> {
            unreachable!()
        }
        async fn exec_start(&self, _: &str, _: &str, _: &ExecOptions) -> Result<KubeExec> {
            unreachable!()
        }
        async fn get_editable(&self, _: KubeKind, _: &str, _: &str) -> Result<EditableObject> {
            unreachable!()
        }
        async fn apply(&self, _: &str, _: &str, _: ApplyMode) -> Result<Vec<ApplyResult>> {
            unreachable!()
        }
        async fn replace(
            &self,
            _: KubeKind,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: ApplyMode,
        ) -> Result<ApplyResult> {
            unreachable!()
        }
    }

    struct Env;
    #[async_trait]
    impl Environment for Env {
        fn id(&self) -> i64 {
            1
        }
        fn name(&self) -> &str {
            "k"
        }
        fn kind(&self) -> EnvKind {
            EnvKind::KubeConfig
        }
        async fn status(&self) -> EnvStatus {
            unreachable!()
        }
        fn docker(&self) -> Option<Arc<dyn fleetops_core::docker::DockerOps>> {
            None
        }
    }

    fn row(name: &str) -> KubeRow {
        KubeRow {
            name: name.into(),
            namespace: Some("apps".into()),
            created: 1,
            status: "Running".into(),
            health: RowHealth::Ok,
            columns: Default::default(),
        }
    }
    fn snap(names: &[&str]) -> WatchEvent {
        WatchEvent::Snapshot {
            rows: names.iter().map(|n| row(n)).collect(),
        }
    }
    const GRACE: Duration = Duration::from_millis(50);

    fn setup() -> (Arc<Watches>, Arc<dyn Environment>, Arc<Scripted>) {
        (
            Arc::new(Watches::default()),
            Arc::new(Env),
            Arc::new(Scripted::default()),
        )
    }

    #[tokio::test]
    async fn subscribers_share_one_upstream_and_a_late_one_starts_from_a_snapshot() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(
            &env,
            ops.clone(),
            KubeKind::Pods,
            Some("apps".into()),
            GRACE,
        );
        started(&ops, 1).await;
        ops.send(Ok(snap(&["x"])));
        assert_eq!(a.next().await, Some(Ok(snap(&["x"]))));
        ops.send(Ok(WatchEvent::Added { row: row("y") }));
        assert!(matches!(a.next().await, Some(Ok(WatchEvent::Added { .. }))));
        let mut b = w.subscribe(
            &env,
            ops.clone(),
            KubeKind::Pods,
            Some("apps".into()),
            GRACE,
        );
        assert_eq!(b.next().await, Some(Ok(snap(&["x", "y"]))));
        assert_eq!(ops.started(), 1);
    }

    #[tokio::test]
    async fn a_lagging_subscriber_gets_a_snapshot() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        started(&ops, 1).await;
        ops.send(Ok(snap(&[])));
        assert_eq!(a.next().await, Some(Ok(snap(&[]))));
        for i in 0..(FEED_BUFFER + 10) {
            ops.send(Ok(WatchEvent::Added {
                row: row(&format!("p{i:04}")),
            }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        match a.next().await {
            Some(Ok(WatchEvent::Snapshot { rows })) => assert_eq!(rows.len(), FEED_BUFFER + 10),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failure_reaches_every_subscriber_and_the_next_subscribe_restarts() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        started(&ops, 1).await;
        ops.send(Err(Error::Forbidden("pods is forbidden".into())));
        assert_eq!(a.next().await, Some(Err("pods is forbidden".into())));
        let _b = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        started(&ops, 2).await;
        assert_eq!(ops.started(), 2);
    }

    #[tokio::test]
    async fn the_upstream_stops_after_the_grace_period_unless_someone_returns() {
        let (w, env, ops) = setup();
        let a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        drop(a);
        let b = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        tokio::time::sleep(GRACE * 3).await;
        assert_eq!(
            (ops.started(), ops.open()),
            (1, 1),
            "a quick return keeps the watch"
        );
        drop(b);
        tokio::time::sleep(GRACE * 3).await;
        assert_eq!(ops.open(), 0);
        assert_eq!(w.open(), 0);
    }
}
