//! Swarm stacks (swarm spec §5): the live swarm a plan is made against,
//! the deploy runner, and deploys a restart cut off.
//!
//! A deploy creates networks, then secrets and configs, then creates or
//! updates services, and waits for each to converge: its update status,
//! its task counts, and its new tasks from the shared task feed. A failed
//! deploy is not undone; Swarm already rolled each service out under its
//! own update policy.

use super::deploy::Timing;
use super::recover::INTERRUPTED;
use super::try_lock;
use crate::routes::swarm_protect::service_guard;
use crate::ws::swarm_tasks::FeedMsg;
use crate::AppState;
use fleetops_core::compose::docker::{network_hash, LABEL_HASH};
use fleetops_core::compose::files::FileValues;
use fleetops_core::compose::plan::{Plan, ResourceAction, ServiceAction};
use fleetops_core::compose::swarm::{named, object_name, service_name, service_spec};
use fleetops_core::compose::swarm_plan::{drift, SwarmLive};
use fleetops_core::compose::{FileKind, FileSource, Project, SwarmMode};
use fleetops_core::env::Environment;
use fleetops_core::streams::BoxStream;
use fleetops_core::swarm::{
    NewSwarmNetwork, ServiceChange, ServiceSummary, SwarmOps, TaskSummary, STACK_LABEL,
};
use fleetops_core::{Error, Result};
use fleetops_store::{RevisionRecord, StackRecord};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// The swarm as `stack`'s plan sees it: its services with their specs,
/// every secret, config and swarm network, and which of them are its own.
pub(crate) async fn live(ops: &dyn SwarmOps, stack: &str) -> Result<SwarmLive> {
    let ours = |s: &Option<String>| s.as_deref() == Some(stack);
    let mut services = Vec::new();
    for svc in ops.services().await?.into_iter().filter(|s| ours(&s.stack)) {
        if let Some(d) = ops.service(&svc.id).await? {
            services.push(d);
        }
    }
    let secrets = ops.secrets().await?;
    let configs = ops.configs().await?;
    let networks = ops.networks().await?;
    Ok(SwarmLive {
        services,
        stack_secrets: secrets
            .iter()
            .filter(|s| ours(&s.stack))
            .map(|s| s.name.clone())
            .collect(),
        secrets: secrets.into_iter().map(|s| s.name).collect(),
        stack_configs: configs
            .iter()
            .filter(|c| ours(&c.stack))
            .map(|c| c.name.clone())
            .collect(),
        configs: configs.into_iter().map(|c| c.name).collect(),
        network_hashes: networks
            .iter()
            .filter(|n| ours(&n.stack))
            .filter_map(|n| Some((n.name.clone(), n.labels.get(LABEL_HASH)?.clone())))
            .collect(),
        networks,
    })
}

/// Refuses a plan that would change or remove a service FleetOps runs on:
/// the deploy could stop itself half way.
pub(crate) fn protect(plan: &mut Plan, live: &SwarmLive, stack: &str) {
    for sp in &plan.services {
        if !matches!(sp.action, ServiceAction::Update | ServiceAction::Remove) {
            continue;
        }
        let name = format!("{stack}_{}", sp.name);
        if let Some(l) = live.service(&name) {
            if service_guard(&l.summary).is_some() {
                plan.blocking.push(format!(
                    "service '{name}' runs FleetOps; change it outside FleetOps"
                ));
            }
        }
    }
}

/// A swarm deploy a restart cut off is marked failed. Nothing is undone:
/// Swarm already rolled out what it was given.
pub(crate) async fn settle_interrupted(s: &AppState, stack: &StackRecord) -> Result<()> {
    let Some(_lock) = try_lock(s, stack.id) else {
        return Ok(());
    };
    let Some(latest) = s.store.list_revisions(stack.id).await?.into_iter().next() else {
        return Ok(());
    };
    if latest.status == "deploying" {
        s.store
            .finish_revision(stack.id, latest.number, Some(INTERRUPTED))
            .await?;
        tracing::info!(stack = %stack.name, revision = latest.number, "an interrupted swarm deploy was marked failed");
    }
    Ok(())
}

type Progress<'a> = &'a (dyn Fn(Value) + Send + Sync);

fn say(progress: Progress, step: &str, message: impl Into<String>) {
    progress(json!({"step": step, "message": message.into()}));
}

fn fail(message: String) -> Error {
    Error::Upstream {
        status: 500,
        message,
    }
}

/// The swarm's tasks, kept from the shared feed, and the ids that existed
/// before the deploy began: only a task started by this deploy can fail it.
pub(crate) struct TaskWatch {
    rx: broadcast::Receiver<FeedMsg>,
    tasks: BTreeMap<String, TaskSummary>,
    before: BTreeSet<String>,
    pending_since: BTreeMap<String, Instant>,
}

impl TaskWatch {
    pub(crate) fn new(
        first: Option<FeedMsg>,
        rx: broadcast::Receiver<FeedMsg>,
        before: BTreeSet<String>,
    ) -> Self {
        let mut w = TaskWatch {
            rx,
            tasks: BTreeMap::new(),
            before,
            pending_since: BTreeMap::new(),
        };
        if let Some(m) = first {
            w.apply(m);
        }
        w
    }

    fn apply(&mut self, msg: FeedMsg) {
        match msg {
            FeedMsg::Snapshot { tasks } => {
                self.tasks = tasks.into_iter().map(|t| (t.id.clone(), t)).collect();
            }
            FeedMsg::Changes { upsert, remove } => {
                for t in upsert {
                    self.tasks.insert(t.id.clone(), t);
                }
                for id in remove {
                    self.tasks.remove(&id);
                }
            }
            // The deploy's own timeout covers a feed that stays paused.
            FeedMsg::Paused { .. } => {}
        }
    }

    /// Takes in whatever the feed sent since the last look. A lagged
    /// receiver keeps what it has; later changes still arrive.
    fn drain(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(m) => self.apply(m),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }

    /// Why `service`'s new tasks fail the deploy, if they do.
    fn failure(&mut self, service_id: &str, key: &str, pending_cap: Duration) -> Option<String> {
        let now = Instant::now();
        for t in self.tasks.values() {
            if t.service_id != service_id || self.before.contains(&t.id) {
                continue;
            }
            match t.state.as_str() {
                "rejected" => {
                    let why = t.error.clone().unwrap_or_else(|| t.message.clone());
                    return Some(format!("{key}: task rejected: {why}"));
                }
                "pending" => {
                    let since = *self.pending_since.entry(t.id.clone()).or_insert(now);
                    if now.duration_since(since) > pending_cap {
                        return Some(format!(
                            "{key}: a task is still pending after {}s: {}",
                            pending_cap.as_secs(),
                            t.message
                        ));
                    }
                }
                _ => {
                    self.pending_since.remove(&t.id);
                }
            }
        }
        None
    }
}

/// Runs a retried removal: Docker refuses to remove an object a task that
/// is still stopping uses, so it is tried for up to 10 s.
async fn remove_retried<F, Fut>(timing: &Timing, mut f: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let start = Instant::now();
    loop {
        match f().await {
            Ok(()) => return Ok(()),
            Err(e) if start.elapsed() >= Duration::from_secs(10) => return Err(e),
            Err(_) => tokio::time::sleep(timing.poll).await,
        }
    }
}

/// Applies `plan` to the swarm. On error nothing is undone.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute(
    ops: &dyn SwarmOps,
    watch: &mut TaskWatch,
    project: &Project,
    files: &FileValues,
    plan: &Plan,
    revision: i64,
    timing: &Timing,
    progress: Progress<'_>,
) -> Result<()> {
    if !plan.blocking.is_empty() {
        return Err(Error::Invalid(plan.blocking.join("; ")));
    }
    let stack = &project.name;
    let ours = BTreeMap::from([(STACK_LABEL.to_string(), stack.clone())]);

    for n in plan
        .networks
        .iter()
        .filter(|n| n.action == ResourceAction::Create)
    {
        let Some(net) = project.networks.values().find(|v| v.name == n.name) else {
            continue;
        };
        say(progress, "network", format!("creating network {}", n.name));
        let mut labels = net.labels.clone();
        labels.extend(ours.clone());
        labels.insert(LABEL_HASH.into(), network_hash(net));
        let mut options = net.driver_opts.clone();
        let encrypted = options.remove("encrypted").is_some();
        ops.create_network(
            &NewSwarmNetwork {
                name: net.name.clone(),
                attachable: net.attachable,
                internal: net.internal,
                encrypted,
                subnet: None,
            },
            &labels,
            &options,
        )
        .await?;
    }

    for (kind, list) in [
        (FileKind::Secret, &plan.secrets),
        (FileKind::Config, &plan.configs),
    ] {
        let decls = match kind {
            FileKind::Secret => &project.secrets,
            FileKind::Config => &project.configs,
        };
        for r in list.iter().filter(|r| r.action == ResourceAction::Create) {
            let Some(key) = decls
                .iter()
                .filter(|(_, src)| !matches!(src, FileSource::External { .. }))
                .map(|(k, _)| k)
                .find(|k| object_name(project, kind, k) == r.name)
            else {
                continue;
            };
            let Some(value) = files.get(&(kind, key.clone())) else {
                return Err(Error::Invalid(format!(
                    "{} {key} has no value",
                    kind.as_str()
                )));
            };
            say(
                progress,
                kind.as_str(),
                format!("creating {} {}", kind.as_str(), r.name),
            );
            // The error names the object, never its content.
            let created = match kind {
                FileKind::Secret => ops.create_secret(&r.name, value.as_bytes(), &ours).await,
                FileKind::Config => ops.create_config(&r.name, value.as_bytes(), &ours).await,
            };
            created.map_err(|e| fail(format!("create {} {}: {e}", kind.as_str(), r.name)))?;
        }
    }

    // Swarm references secrets and configs by id.
    let secret_ids: BTreeMap<String, String> = ops
        .secrets()
        .await?
        .into_iter()
        .map(|s| (s.name, s.id))
        .collect();
    let config_ids: BTreeMap<String, String> = ops
        .configs()
        .await?
        .into_iter()
        .map(|c| (c.name, c.id))
        .collect();
    let live = live(ops, stack).await?;

    // (key, service id) of every service created or updated.
    let mut changed: Vec<(String, String)> = Vec::new();
    for sp in plan
        .services
        .iter()
        .filter(|s| matches!(s.action, ServiceAction::Create | ServiceAction::Update))
    {
        let key = &sp.name;
        let mut spec = service_spec(project, key);
        for (what, field, name_key, id_key, ids) in [
            ("secret", "Secrets", "SecretName", "SecretID", &secret_ids),
            ("config", "Configs", "ConfigName", "ConfigID", &config_ids),
        ] {
            if let Some(refs) = spec["TaskTemplate"]["ContainerSpec"][field].as_array_mut() {
                for r in refs {
                    let name = r[name_key].as_str().unwrap_or_default().to_string();
                    let id = ids
                        .get(&name)
                        .ok_or_else(|| fail(format!("{key}: {what} {name} does not exist")))?;
                    r[id_key] = json!(id);
                }
            }
        }
        let name = service_name(project, key);
        let id = match live.service(&name) {
            None => {
                say(progress, "service", format!("creating {key}"));
                ops.create_service(&spec)
                    .await
                    .map_err(|e| fail(format!("create {key}: {e}")))?
            }
            Some(l) => {
                say(progress, "service", format!("updating {key}"));
                ops.update_service(
                    &l.summary.id,
                    ServiceChange::Spec {
                        spec,
                        version: l.version,
                    },
                )
                .await
                .map_err(|e| fail(format!("update {key}: {e}")))?;
                l.summary.id.clone()
            }
        };
        let job = matches!(
            project.services[key].swarm.as_ref().map(|d| d.mode),
            Some(SwarmMode::ReplicatedJob | SwarmMode::GlobalJob)
        );
        if !job {
            changed.push((key.clone(), id));
        }
    }

    converge(ops, watch, &changed, timing, progress).await?;

    for sp in plan
        .services
        .iter()
        .filter(|s| s.action == ServiceAction::Remove)
    {
        let name = format!("{stack}_{}", sp.name);
        if let Some(l) = live.service(&name) {
            say(progress, "service", format!("removing {}", sp.name));
            ops.remove_service(&l.summary.id)
                .await
                .map_err(|e| fail(format!("remove {}: {e}", sp.name)))?;
        }
    }

    commit(ops, plan, &live, timing, progress).await;
    say(progress, "commit", format!("revision {revision} deployed"));
    Ok(())
}

/// Waits until every service in `changed` runs as asked, or fails.
async fn converge(
    ops: &dyn SwarmOps,
    watch: &mut TaskWatch,
    changed: &[(String, String)],
    timing: &Timing,
    progress: Progress<'_>,
) -> Result<()> {
    let start = Instant::now();
    let mut waiting: BTreeMap<&str, &str> = changed
        .iter()
        .map(|(k, id)| (id.as_str(), k.as_str()))
        .collect();
    let mut shown: BTreeMap<&str, (u64, u64, Option<String>)> = BTreeMap::new();
    while !waiting.is_empty() {
        watch.drain();
        let services = ops.services().await?;
        for (id, key) in waiting.clone() {
            let Some(svc) = services.iter().find(|s| s.id == id) else {
                return Err(fail(format!("{key} disappeared during the deploy")));
            };
            let state = svc.update_state.clone();
            let now = (svc.tasks_running, svc.tasks_desired, state.clone());
            if shown.get(id) != Some(&now) {
                let extra = state
                    .as_deref()
                    .map(|s| format!(", {s}"))
                    .unwrap_or_default();
                say(
                    progress,
                    "service",
                    format!("{key}: {}/{} running{extra}", now.0, now.1),
                );
                shown.insert(id, now);
            }
            if let Some(
                s @ ("paused" | "rollback_started" | "rollback_paused" | "rollback_completed"),
            ) = state.as_deref()
            {
                let detail = ops
                    .service(id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|d| d.update_status)
                    .and_then(|u| u["Message"].as_str().map(str::to_string))
                    .unwrap_or_default();
                return Err(fail(format!("{key}: update {s}: {detail}")));
            }
            if let Some(why) = watch.failure(id, key, timing.swarm_pending) {
                return Err(fail(why));
            }
            let updated = matches!(state.as_deref(), None | Some("completed"));
            if updated && svc.tasks_running >= svc.tasks_desired {
                waiting.remove(id);
            }
        }
        if waiting.is_empty() {
            break;
        }
        if start.elapsed() >= timing.health_cap {
            let key = waiting.values().next().copied().unwrap_or_default();
            return Err(fail(format!(
                "{key} did not converge within {}s",
                timing.health_cap.as_secs()
            )));
        }
        tokio::time::sleep(timing.poll).await;
    }
    Ok(())
}

/// Removes the stack's secrets, configs and networks nothing uses any more.
/// Past this point the deploy stands; failures are reported, not undone.
async fn commit(
    ops: &dyn SwarmOps,
    plan: &Plan,
    live: &SwarmLive,
    timing: &Timing,
    progress: Progress<'_>,
) {
    let secrets = ops.secrets().await.unwrap_or_default();
    for r in plan
        .secrets
        .iter()
        .filter(|r| r.action == ResourceAction::Remove)
    {
        if let Some(sec) = secrets.iter().find(|s| s.name == r.name) {
            if let Err(e) = remove_retried(timing, || ops.remove_secret(&sec.id)).await {
                say(
                    progress,
                    "commit",
                    format!("could not remove secret {}: {e}", r.name),
                );
            }
        }
    }
    let configs = ops.configs().await.unwrap_or_default();
    for r in plan
        .configs
        .iter()
        .filter(|r| r.action == ResourceAction::Remove)
    {
        if let Some(c) = configs.iter().find(|c| c.name == r.name) {
            if let Err(e) = remove_retried(timing, || ops.remove_config(&c.id)).await {
                say(
                    progress,
                    "commit",
                    format!("could not remove config {}: {e}", r.name),
                );
            }
        }
    }
    for r in plan
        .networks
        .iter()
        .filter(|r| r.action == ResourceAction::Remove)
    {
        if let Some(n) = live.networks.iter().find(|n| n.name == r.name) {
            if let Err(e) = remove_retried(timing, || ops.remove_network(&n.id)).await {
                say(
                    progress,
                    "commit",
                    format!("could not remove network {}: {e}", r.name),
                );
            }
        }
    }
}

/// Runs a swarm deploy in the background, once polled, and streams its
/// progress for the task manager. A failure is recorded on the revision and
/// sent as the last message's `error`. The stack's lock is released either
/// way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn deploy_stream(
    state: AppState,
    env: Arc<dyn Environment>,
    ops: Arc<dyn SwarmOps>,
    stack: StackRecord,
    revision: RevisionRecord,
    project: Project,
    files: FileValues,
    plan: Plan,
    timing: Timing,
) -> BoxStream<Value> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let job = async move {
        let send = {
            let tx = tx.clone();
            move |v: Value| {
                let _ = tx.send(v);
            }
        };
        let outcome = async {
            let (first, feed) = state
                .hubs
                .swarm_tasks
                .subscribe(env.clone(), state.config.swarm_task_poll);
            let before = ops.tasks().await?.into_iter().map(|t| t.id).collect();
            let mut watch = TaskWatch::new(first, feed, before);
            execute(
                ops.as_ref(),
                &mut watch,
                &project,
                &files,
                &plan,
                revision.number,
                &timing,
                &send,
            )
            .await
        }
        .await;
        let error = outcome.err().map(|e| {
            format!("deploy failed: {e}; what was applied stays: roll back to an earlier revision to undo it")
        });
        if let Err(e) = state
            .store
            .finish_revision(stack.id, revision.number, error.as_deref())
            .await
        {
            tracing::warn!(error = %e, stack = %stack.name, "recording a deploy failed");
        }
        if let Some(msg) = error {
            let _ = tx.send(json!({"error": msg}));
        }
        state.stack_locks.lock().unwrap().remove(&stack.id);
    };
    let started = futures_util::stream::once(async move {
        tokio::spawn(job);
        rx
    });
    Box::pin(started.flat_map(|rx| {
        futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|v| (Ok(v), rx)) },
        )
    }))
}

/// Removes everything labelled with `stack`: its services first, then its
/// secrets, configs and networks once nothing uses them. Volumes live on
/// each node and stay.
pub(crate) async fn remove(ops: &dyn SwarmOps, stack: &str, timing: &Timing) -> Result<()> {
    let ours = |s: &Option<String>| s.as_deref() == Some(stack);
    let mut errors = Vec::new();
    for svc in ops.services().await?.into_iter().filter(|s| ours(&s.stack)) {
        if let Err(e) = ops.remove_service(&svc.id).await {
            errors.push(format!("{}: {e}", svc.name));
        }
    }
    for sec in ops.secrets().await?.into_iter().filter(|s| ours(&s.stack)) {
        if let Err(e) = remove_retried(timing, || ops.remove_secret(&sec.id)).await {
            errors.push(format!("secret {}: {e}", sec.name));
        }
    }
    for c in ops.configs().await?.into_iter().filter(|c| ours(&c.stack)) {
        if let Err(e) = remove_retried(timing, || ops.remove_config(&c.id)).await {
            errors.push(format!("config {}: {e}", c.name));
        }
    }
    for n in ops.networks().await?.into_iter().filter(|n| ours(&n.stack)) {
        if let Err(e) = remove_retried(timing, || ops.remove_network(&n.id)).await {
            errors.push(format!("network {}: {e}", n.name));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(fail(errors.join("; ")))
    }
}

/// One service of a Swarm stack, as its stack page shows it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct StackService {
    /// The Compose key (`web` for `shop_web`).
    pub key: String,
    pub service: ServiceSummary,
    /// Deployed by FleetOps: it carries the spec-hash label.
    pub managed: bool,
    /// What changed on it since it was deployed; empty when nothing did.
    pub drift: Vec<String>,
}

/// `stack`'s live services, with what drifted from the deployed revision.
pub(crate) async fn stack_services(
    s: &AppState,
    ops: &dyn SwarmOps,
    stack: &StackRecord,
) -> Result<Vec<StackService>> {
    let deployed = super::start::deployed(s, stack, fleetops_core::compose::Mode::Swarm)
        .await?
        .and_then(|(_, _, p)| p);
    let live = live(ops, &stack.name).await?;
    let names = live.network_names();
    let prefix = format!("{}_", stack.name);
    let mut out: Vec<StackService> = live
        .services
        .iter()
        .map(|d| {
            let key = d
                .summary
                .name
                .strip_prefix(&prefix)
                .unwrap_or(&d.summary.name)
                .to_string();
            let spec = named(&d.spec, &names);
            StackService {
                managed: spec["Labels"][fleetops_core::compose::swarm::LABEL_SPEC_HASH].is_string(),
                drift: drift(&spec, deployed.as_ref(), &key),
                service: d.summary.clone(),
                key,
            }
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}
