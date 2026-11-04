//! The scan engine: decides what to scan and when, and runs at most one scan
//! per host (four overall). The `ee_scans` table is the queue, so a restart
//! loses nothing.

use super::config::{effective, Effective, EnvOverride, GlobalConfig};
use super::db::{self, Status};
use super::inventory::{new_keys, Inventory, LiveInventory};
use super::model::{ImageKey, ImageUse, ScanTarget, Trigger, RESCAN_SECS};
use super::runner::{LiveRunner, Runner};
use super::trivy::parse_report;
use fleetops_api::AppState;
use fleetops_core::edition::Feature;
use fleetops_core::env::{EnvKind, EnvStatus, Environment};
use fleetops_core::{now_unix, Result};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Semaphore};
use utoipa::ToSchema;

/// Scans at once across the fleet.
pub const MAX_RUNNING: usize = 4;
/// How often the loop looks around, at most.
pub const TICK: Duration = Duration::from_secs(5);
/// Docker hosts are re-read this often even without events.
pub const INVENTORY_SECS: i64 = 600;
/// Clusters are re-read this often (no continuous watch).
pub const KUBE_INVENTORY_SECS: i64 = 120;
/// A scanner that could not be prepared is tried again after this.
pub const RETRY_PREPARE_SECS: i64 = 600;
/// Docker events wait this long for more before an inventory.
pub const DEBOUNCE_SECS: i64 = 5;
/// A scan longer than this is given up (the runners have their own limits).
pub const SCAN_DEADLINE: Duration = Duration::from_secs(25 * 60);

/// What the engine tells other features (notifications) as it works.
#[async_trait::async_trait]
pub trait ScanEvents: Send + Sync {
    /// A scan of `key` finished with `findings`.
    async fn scan_finished(
        &self,
        s: &AppState,
        key: &super::model::ImageKey,
        findings: &[super::model::Finding],
    );
    /// An environment's image uses were re-read. `enabling`: the first
    /// inventory after scanning was turned on for it.
    async fn inventory_changed(
        &self,
        s: &AppState,
        env_id: i64,
        before: &[ImageUse],
        after: &[ImageUse],
        enabling: bool,
    );
    /// Scanning was turned off for `env_id` (its uses were cleared).
    async fn scanning_disabled(&self, _s: &AppState, _env_id: i64) {}
}

/// Whether an environment's scanner can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScannerStatus {
    Off,
    Preparing,
    Ready,
    Unavailable { reason: String },
}

#[derive(Default)]
struct EnvState {
    status: Option<ScannerStatus>,
    prepared_at: i64,
    inventoried_at: i64,
    /// Docker events since the last inventory, and when the first came.
    dirty_since: Option<i64>,
    watcher: Option<tokio::task::AbortHandle>,
}

pub struct Engine {
    backend: OnceLock<(Arc<dyn Runner>, Arc<dyn Inventory>)>,
    envs: Mutex<HashMap<i64, EnvState>>,
    running: Arc<Semaphore>,
    /// Hosts with a scan running: `(env_id, node)`.
    busy: Mutex<HashSet<(i64, String)>>,
    in_flight: Mutex<HashSet<i64>>,
    idle: Notify,
    wake: Notify,
    signal_at: Mutex<(Option<Instant>, bool)>,
    pruned_at: Mutex<i64>,
    /// One enqueue at a time, so two inventories never both queue a digest.
    enqueueing: tokio::sync::Mutex<()>,
    events: OnceLock<Arc<dyn ScanEvents>>,
}

impl Engine {
    /// `backend` replaces the live runner and inventory (tests).
    pub fn new(backend: Option<(Arc<dyn Runner>, Arc<dyn Inventory>)>) -> Arc<Self> {
        let e = Engine {
            backend: OnceLock::new(),
            envs: Mutex::new(HashMap::new()),
            running: Arc::new(Semaphore::new(MAX_RUNNING)),
            busy: Mutex::new(HashSet::new()),
            in_flight: Mutex::new(HashSet::new()),
            idle: Notify::new(),
            wake: Notify::new(),
            signal_at: Mutex::new((None, false)),
            pruned_at: Mutex::new(0),
            enqueueing: tokio::sync::Mutex::new(()),
            events: OnceLock::new(),
        };
        if let Some(b) = backend {
            let _ = e.backend.set(b);
        }
        Arc::new(e)
    }

    fn backend(&self, s: &AppState) -> &(Arc<dyn Runner>, Arc<dyn Inventory>) {
        self.backend.get_or_init(|| {
            let tmp_dir = s
                .config
                .data_dir
                .clone()
                .unwrap_or_else(std::env::temp_dir)
                .join("tmp");
            (
                Arc::new(LiveRunner { tmp_dir }) as Arc<dyn Runner>,
                Arc::new(LiveInventory) as Arc<dyn Inventory>,
            )
        })
    }

    /// Who hears about finished scans and inventories (set once, at startup).
    pub fn set_events(&self, ev: Arc<dyn ScanEvents>) {
        let _ = self.events.set(ev);
    }

    pub fn status(&self, env_id: i64) -> ScannerStatus {
        self.envs
            .lock()
            .unwrap()
            .get(&env_id)
            .and_then(|e| e.status.clone())
            .unwrap_or(ScannerStatus::Off)
    }

    fn set_status(&self, env_id: i64, st: ScannerStatus) {
        self.envs.lock().unwrap().entry(env_id).or_default().status = Some(st);
    }

    /// Runs forever: recovers interrupted scans, then ticks.
    pub async fn run(self: Arc<Self>, s: AppState) {
        self.recover(&s).await;
        loop {
            if let Err(e) = self.tick(&s, now_unix()).await {
                tracing::warn!(error = %e, "scan engine tick failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    /// Queues again the scans a restart interrupted.
    pub async fn recover(&self, s: &AppState) {
        match db::recover_after_restart(&s.store, now_unix()).await {
            Ok(targets) => {
                for t in targets {
                    let _ = self.enqueue(s, t, Trigger::Arrival).await;
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not recover interrupted scans"),
        }
    }

    /// Queues a scan of `t`'s image, unless one is already queued or running.
    pub async fn enqueue(&self, s: &AppState, t: ScanTarget, trig: Trigger) -> Result<i64> {
        let _one = self.enqueueing.lock().await;
        if let Some(id) = db::active_scan(&s.store, &t.key).await? {
            let row = db::scan(&s.store, id).await?;
            let stuck = row.as_ref().is_some_and(|r| {
                r.status == Status::Queued
                    && r.target.env_id != t.env_id
                    && self.status(r.target.env_id) != ScannerStatus::Ready
            });
            if !stuck {
                return Ok(id);
            }
            // Its scanner can't run: a ready host scans the digest instead.
            db::finish_failed(&s.store, id, "scanned from another environment", now_unix()).await?;
        }
        let id = db::insert_scan(&s.store, &t, trig, now_unix()).await?;
        self.changed(s);
        self.wake.notify_one();
        Ok(id)
    }

    /// Tells pages to refetch, at most once a second; a skipped ping is
    /// sent by the next tick.
    fn changed(&self, s: &AppState) {
        let mut g = self.signal_at.lock().unwrap();
        if g.0.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            let _ = s.hubs.signal().send("security".into());
            *g = (Some(Instant::now()), false);
        } else {
            g.1 = true;
        }
    }

    fn flush_signal(&self, s: &AppState) {
        let pending = self.signal_at.lock().unwrap().1;
        if pending {
            self.changed(s);
        }
    }

    async fn configs(
        s: &AppState,
    ) -> Result<(
        GlobalConfig,
        Option<String>,
        HashMap<i64, (EnvOverride, Option<String>)>,
    )> {
        let (g, gpw) = db::get_global(&s.store).await?;
        let envs = db::list_envs(&s.store)
            .await?
            .into_iter()
            .map(|(id, o, pw)| (id, (o, pw)))
            .collect();
        Ok((g, gpw, envs))
    }

    async fn effective_for(s: &AppState, env_id: i64) -> Result<Effective> {
        let (g, gpw) = db::get_global(&s.store).await?;
        let o = db::get_env(&s.store, env_id).await?;
        Ok(effective(
            &g,
            gpw.as_deref(),
            o.as_ref().map(|(o, _)| o),
            o.as_ref().and_then(|(_, p)| p.as_deref()),
        ))
    }

    async fn stored_envs(s: &AppState) -> Result<Vec<Arc<dyn Environment>>> {
        Ok(s.store
            .list_environments()
            .await?
            .into_iter()
            .filter_map(|r| s.envs.get_stored(&r.name).ok())
            .collect())
    }

    /// One look around at `now`: settings, scanners, inventories, re-scans,
    /// then the queue.
    #[doc(hidden)]
    pub async fn tick(self: &Arc<Self>, s: &AppState, now: i64) -> Result<()> {
        self.flush_signal(s);
        if !s.entitlements.has(Feature::VulnScan) {
            return Ok(());
        }
        let (g, gpw, overrides) = Self::configs(s).await?;
        let envs = Self::stored_envs(s).await?;
        for env in &envs {
            let o = overrides.get(&env.id());
            let cfg = effective(
                &g,
                gpw.as_deref(),
                o.map(|(o, _)| o),
                o.and_then(|(_, p)| p.as_deref()),
            );
            self.reconcile(s, env, &cfg, now).await;
        }
        let ids: HashSet<i64> = envs.iter().map(|e| e.id()).collect();
        self.envs.lock().unwrap().retain(|id, st| {
            let keep = ids.contains(id);
            if !keep {
                if let Some(w) = st.watcher.take() {
                    w.abort();
                }
            }
            keep
        });
        db::fail_queued_of_gone_envs(&s.store, now).await?;
        let prune = {
            let mut p = self.pruned_at.lock().unwrap();
            let due = now - *p >= 86_400;
            if due {
                *p = now;
            }
            due
        };
        if prune {
            db::prune_unused(&s.store, now - 30 * 86_400).await?;
        }
        self.dispatch(s).await
    }

    /// Settings changed for `env_id`: act on it now rather than next tick.
    pub async fn env_changed(self: &Arc<Self>, s: &AppState, env_id: i64) -> Result<()> {
        let rec = s.store.get_environment(env_id).await?;
        let env = s.envs.get_stored(&rec.name)?;
        let cfg = Self::effective_for(s, env_id).await?;
        self.reconcile(s, &env, &cfg, now_unix()).await;
        self.wake.notify_one();
        Ok(())
    }

    async fn reconcile(
        self: &Arc<Self>,
        s: &AppState,
        env: &Arc<dyn Environment>,
        cfg: &Effective,
        now: i64,
    ) {
        let id = env.id();
        if !cfg.enabled {
            let was_on = {
                let mut m = self.envs.lock().unwrap();
                let st = m.entry(id).or_default();
                if let Some(w) = st.watcher.take() {
                    w.abort();
                }
                let was = !matches!(st.status, None | Some(ScannerStatus::Off));
                *st = EnvState {
                    status: Some(ScannerStatus::Off),
                    ..Default::default()
                };
                was
            };
            if was_on
                || !db::uses_for_envs(&s.store, &[id])
                    .await
                    .unwrap_or_default()
                    .is_empty()
            {
                let _ = db::replace_uses(&s.store, id, &[]).await;
                let _ = db::fail_queued_for_env(&s.store, id, "scanning turned off", now).await;
                if let Some(ev) = self.events.get() {
                    ev.scanning_disabled(s, id).await;
                }
                self.changed(s);
            }
            return;
        }
        let (status, prepared_at, inventoried_at, dirty_since, watching) = {
            let m = self.envs.lock().unwrap();
            let st = m.get(&id);
            (
                st.and_then(|s| s.status.clone()),
                st.map_or(0, |s| s.prepared_at),
                st.map_or(0, |s| s.inventoried_at),
                st.and_then(|s| s.dirty_since),
                st.is_some_and(|s| s.watcher.is_some()),
            )
        };
        let just_enabled = matches!(status, None | Some(ScannerStatus::Off));
        let retry = matches!(status, Some(ScannerStatus::Unavailable { .. }))
            && now - prepared_at >= RETRY_PREPARE_SECS;
        if just_enabled || retry {
            self.set_status(id, ScannerStatus::Preparing);
            self.changed(s);
            let me = self.clone();
            let (s2, env2, cfg2) = (s.clone(), env.clone(), cfg.clone());
            // Preparing pulls an image: never hold up the loop for it.
            tokio::spawn(async move {
                me.prepare(&s2, &env2, &cfg2, now, just_enabled).await;
            });
            return;
        }
        // An unavailable scanner still keeps the inventory current, so its
        // images show as waiting rather than vanishing.
        if status == Some(ScannerStatus::Preparing) {
            return;
        }
        if matches!(env.kind(), EnvKind::DockerLocal | EnvKind::DockerAgent) && !watching {
            self.watch(s, env);
        }
        let every = if env.kind().is_kube() {
            KUBE_INVENTORY_SECS
        } else {
            INVENTORY_SECS
        };
        let due =
            now - inventoried_at >= every || dirty_since.is_some_and(|t| now - t >= DEBOUNCE_SECS);
        if due {
            if env.kind().is_kube() {
                // Copied pull secrets of a scan a crash interrupted go here.
                let (runner, _) = self.backend(s).clone();
                let _ = runner.cleanup_orphans(env).await;
            }
            self.inventory(s, env, Trigger::Arrival, now).await;
        }
    }

    async fn prepare(
        self: &Arc<Self>,
        s: &AppState,
        env: &Arc<dyn Environment>,
        cfg: &Effective,
        now: i64,
        fresh: bool,
    ) {
        let (runner, _) = self.backend(s).clone();
        let nodes: Vec<String> = match env.swarm() {
            Some(sw) if env.kind().is_swarm() => match sw.nodes().await {
                Ok(n) => n.into_iter().map(|n| n.id).collect(),
                Err(e) => {
                    self.unavailable(s, env.id(), now, e.to_string());
                    return;
                }
            },
            _ => vec![String::new()],
        };
        let _ = runner.cleanup_orphans(env).await;
        let trig = if fresh {
            Trigger::Enable
        } else {
            Trigger::Arrival
        };
        let mut results = Vec::new();
        for node in &nodes {
            results.push((node.clone(), runner.prepare(env, node, cfg).await));
        }
        if let Err(reason) = prepared(results) {
            self.unavailable(s, env.id(), now, reason);
            self.inventory(s, env, trig, now).await;
            return;
        }
        {
            let mut m = self.envs.lock().unwrap();
            let st = m.entry(env.id()).or_default();
            st.status = Some(ScannerStatus::Ready);
            st.prepared_at = now;
        }
        self.changed(s);
        let trig = if fresh {
            Trigger::Enable
        } else {
            Trigger::Arrival
        };
        self.inventory(s, env, trig, now).await;
        self.wake.notify_one();
    }

    fn unavailable(&self, s: &AppState, env_id: i64, now: i64, reason: String) {
        tracing::warn!(env_id, reason, "scanner unavailable");
        {
            let mut m = self.envs.lock().unwrap();
            let st = m.entry(env_id).or_default();
            st.status = Some(ScannerStatus::Unavailable { reason });
            st.prepared_at = now;
        }
        self.changed(s);
    }

    fn watch(self: &Arc<Self>, s: &AppState, env: &Arc<dyn Environment>) {
        let mut rx =
            s.hubs
                .events(env)
                .subscribe(env.clone(), s.hubs.status(), s.config.ws_backoff);
        let me = self.clone();
        let id = env.id();
        let task = tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let image = ev.kind == "image"
                            && ["pull", "tag", "load", "import", "untag", "delete"]
                                .contains(&ev.action.as_str());
                        let container = ev.kind == "container"
                            && ["start", "destroy"].contains(&ev.action.as_str());
                        if image || container {
                            let mut m = me.envs.lock().unwrap();
                            let st = m.entry(id).or_default();
                            st.dirty_since.get_or_insert(now_unix());
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        let mut m = me.envs.lock().unwrap();
                        m.entry(id)
                            .or_default()
                            .dirty_since
                            .get_or_insert(now_unix());
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        });
        self.envs.lock().unwrap().entry(id).or_default().watcher = Some(task.abort_handle());
    }

    /// Re-reads where `env`'s images are and queues what needs a scan:
    /// images new to it (`trig`), never scanned, or not scanned for a day.
    async fn inventory(&self, s: &AppState, env: &Arc<dyn Environment>, trig: Trigger, now: i64) {
        let id = env.id();
        {
            let mut m = self.envs.lock().unwrap();
            let st = m.entry(id).or_default();
            st.inventoried_at = now;
            st.dirty_since = None;
        }
        let (_, inventory) = self.backend(s).clone();
        let after = match inventory.uses(env).await {
            Ok(u) => u,
            Err(e) => {
                tracing::info!(env = env.name(), error = %e, "image inventory failed");
                return;
            }
        };
        let before = db::uses_for_envs(&s.store, &[id]).await.unwrap_or_default();
        if let Err(e) = db::replace_uses(&s.store, id, &after).await {
            tracing::warn!(error = %e, "could not save the image inventory");
            return;
        }
        if before != after {
            self.changed(s);
        }
        if let Some(ev) = self.events.get() {
            ev.inventory_changed(s, id, &before, &after, trig == Trigger::Enable)
                .await;
        }
        let fresh: HashSet<ImageKey> = new_keys(&before, &after).into_iter().collect();
        let mut keys: Vec<ImageKey> = after.iter().map(|u| u.key.clone()).collect();
        keys.sort();
        keys.dedup();
        let states = db::latest(&s.store, &keys).await.unwrap_or_default();
        for key in keys {
            let st = states.get(&key);
            let never = st.is_none_or(|k| k.last_done.is_none() && k.last.status != Status::Failed);
            let old = st
                .and_then(|k| k.last_done.as_ref())
                .and_then(|d| d.finished_at)
                .is_some_and(|t| now - t >= RESCAN_SECS);
            let failed_long_ago = st.is_some_and(|k| {
                k.last_done.is_none()
                    && k.last.status == Status::Failed
                    && k.last.finished_at.is_some_and(|t| now - t >= RESCAN_SECS)
            });
            // A digest already scanned recently (on another host) shares that result.
            let recent = st
                .and_then(|k| k.last_done.as_ref())
                .and_then(|d| d.finished_at)
                .is_some_and(|t| now - t < RESCAN_SECS);
            let busy =
                st.is_some_and(|k| matches!(k.last.status, Status::Queued | Status::Running));
            let why = if fresh.contains(&key) && !recent && !busy {
                Some(trig)
            } else if never {
                Some(if trig == Trigger::Enable {
                    Trigger::Enable
                } else {
                    Trigger::Arrival
                })
            } else if old || failed_long_ago {
                Some(Trigger::Periodic)
            } else {
                None
            };
            if let Some(why) = why {
                if self.status(id) != ScannerStatus::Ready {
                    // Shown as waiting for the scanner; nothing is queued that can't run.
                    continue;
                }
                let u = target_use(&after, &key);
                if let Err(e) = self.enqueue(s, target_of(u), why).await {
                    tracing::warn!(error = %e, "could not queue a scan");
                }
            }
        }
    }

    async fn dispatch(self: &Arc<Self>, s: &AppState) -> Result<()> {
        let ready: Vec<i64> = self
            .envs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, st)| st.status == Some(ScannerStatus::Ready))
            .map(|(id, _)| *id)
            .collect();
        for row in db::queued(&s.store, &ready, 50).await? {
            if self.running.available_permits() == 0 {
                break;
            }
            if self.in_flight.lock().unwrap().contains(&row.id) {
                continue;
            }
            let env_id = row.target.env_id;
            if self.status(env_id) != ScannerStatus::Ready {
                continue;
            }
            let host = (env_id, row.target.node.clone());
            if !self.busy.lock().unwrap().insert(host.clone()) {
                continue;
            }
            let Ok(permit) = self.running.clone().try_acquire_owned() else {
                self.busy.lock().unwrap().remove(&host);
                break;
            };
            self.in_flight.lock().unwrap().insert(row.id);
            let me = self.clone();
            let s2 = s.clone();
            tokio::spawn(async move {
                me.run_one(&s2, row.id, row.target).await;
                drop(permit);
                me.busy.lock().unwrap().remove(&host);
                let empty = {
                    let mut f = me.in_flight.lock().unwrap();
                    f.remove(&row.id);
                    f.is_empty()
                };
                if empty {
                    me.idle.notify_waiters();
                }
                me.wake.notify_one();
            });
        }
        Ok(())
    }

    async fn run_one(&self, s: &AppState, id: i64, t: ScanTarget) {
        let _ = db::mark_running(&s.store, id, now_unix()).await;
        self.changed(s);
        let result = self.scan(s, id, &t).await;
        match result {
            Ok((image, findings)) => {
                match db::finish_ok(&s.store, id, &image, &findings, now_unix()).await {
                    Err(e) => tracing::warn!(error = %e, "could not save a scan result"),
                    Ok(_) => {
                        if let Some(ev) = self.events.get() {
                            ev.scan_finished(s, &t.key, &findings).await;
                        }
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = db::finish_failed(&s.store, id, &msg, now_unix()).await;
                let env_name = s
                    .store
                    .get_environment(t.env_id)
                    .await
                    .map(|r| r.name)
                    .unwrap_or_default();
                fleetops_api::audit_detail(
                    s,
                    None,
                    "security.scan.failed",
                    "image",
                    &t.image_ref,
                    "failure",
                    Some(json!({"env": env_name, "node": t.node, "error": msg})),
                )
                .await;
            }
        }
        self.changed(s);
    }

    async fn scan(
        &self,
        s: &AppState,
        id: i64,
        t: &ScanTarget,
    ) -> Result<(String, Vec<super::model::Finding>)> {
        let unreachable = || fleetops_core::Error::EnvUnavailable("environment unreachable".into());
        let rec = s
            .store
            .get_environment(t.env_id)
            .await
            .map_err(|_| unreachable())?;
        let env = s.envs.get_stored(&rec.name).map_err(|_| unreachable())?;
        if let EnvStatus::Down { .. } = env.status().await {
            return Err(unreachable());
        }
        let cfg = Self::effective_for(s, t.env_id).await?;
        let (runner, _) = self.backend(s).clone();
        let report = tokio::time::timeout(SCAN_DEADLINE, runner.scan(&env, t, &cfg, id))
            .await
            .map_err(|_| fleetops_core::Error::Internal("the scan took too long".into()))??;
        Ok((cfg.trivy_image, parse_report(&report)?))
    }

    /// Waits until no scan is running (tests).
    #[doc(hidden)]
    pub async fn settle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.in_flight.lock().unwrap().is_empty() {
                return;
            }
            notified.await;
        }
    }
}

/// Ready when any node (or the one host) prepared; a down Swarm node alone
/// does not stop the others.
pub fn prepared(results: Vec<(String, Result<()>)>) -> std::result::Result<(), String> {
    let mut first = None;
    for (node, r) in results {
        match r {
            Ok(()) => return Ok(()),
            Err(e) => {
                if !node.is_empty() {
                    tracing::info!(node, error = %e, "scanner not prepared on a swarm node");
                }
                first.get_or_insert(e.to_string());
            }
        }
    }
    Err(first.unwrap_or_else(|| "no nodes to scan on".into()))
}

/// The use a scan of `key` starts from: a running one, if any.
fn target_use<'a>(uses: &'a [ImageUse], key: &ImageKey) -> &'a ImageUse {
    let mine: Vec<&ImageUse> = uses.iter().filter(|u| &u.key == key).collect();
    // One stable platform per digest: the lowest known one, then unknown.
    mine.iter()
        .filter(|u| u.running)
        .min_by_key(|u| (u.platform.is_none(), u.platform.clone()))
        .copied()
        .unwrap_or(mine[0])
}

pub fn target_of(u: &ImageUse) -> ScanTarget {
    ScanTarget {
        env_id: u.env_id,
        node: u.node.clone(),
        key: u.key.clone(),
        image_ref: u
            .image_id
            .clone()
            .filter(|_| u.key.digest().is_none())
            .unwrap_or_else(|| u.image_ref.clone()),
        namespace: u.namespace.clone(),
        pull_secrets: u.pull_secrets.clone(),
        platform: u.platform.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::Error;

    #[test]
    fn target_prefers_running_lowest_platform() {
        let u = |running: bool, platform: Option<&str>| ImageUse {
            env_id: 1,
            node: String::new(),
            key: ImageKey("sha256:d".into()),
            image_ref: "app:1".into(),
            image_id: None,
            namespace: "a".into(),
            workload: "Deployment/app".into(),
            container: "app".into(),
            running,
            pull_secrets: vec![],
            platform: platform.map(String::from),
        };
        let uses = [
            u(false, None),
            u(true, Some("linux/arm64")),
            u(true, Some("linux/amd64")),
            u(true, None),
        ];
        let t = target_of(target_use(&uses, &ImageKey("sha256:d".into())));
        assert_eq!(t.platform.as_deref(), Some("linux/amd64"));
    }

    #[test]
    fn one_ready_node_is_enough() {
        let r = prepared(vec![
            ("n1".into(), Err(Error::EnvUnavailable("down".into()))),
            ("n2".into(), Ok(())),
        ]);
        assert_eq!(r, Ok(()));
        let none = prepared(vec![(
            "n1".into(),
            Err(Error::Invalid("pull failed".into())),
        )]);
        assert!(none.unwrap_err().contains("pull failed"));
    }
}
