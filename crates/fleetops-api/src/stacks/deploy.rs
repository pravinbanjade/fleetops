//! Applies a plan (stacks spec §4). Nothing is deleted before commit:
//! replaced, scaled-down and orphaned containers are stopped and renamed
//! aside, and only removed once every service is up. Any failure before
//! that is undone by [`super::restore`].

use super::build::{cleanup, Prepared};
use super::restore::old_suffix;
use super::{is_old, service_of, stack_containers};
use crate::AppState;
use fleetops_core::compose::docker::{
    container_body, container_name, network_create, start_order, volume_create, LABEL_NUMBER,
};
use fleetops_core::compose::files::{archive, FileValues};
use fleetops_core::compose::plan::{Plan, ResourceAction, ServiceAction};
use fleetops_core::compose::{DependsCondition, Project, Service};
use fleetops_core::docker::{split_image_reference, ContainerAction, DockerOps};
use fleetops_core::streams::BoxStream;
use fleetops_core::{Error, Result};
use fleetops_store::{RevisionRecord, StackRecord};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the runner waits, and how often it looks.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// After starting a container whose restart policy is `no`, how long to
    /// wait before checking it did not exit non-zero.
    pub exit_check: Duration,
    /// How often a dependency's state is polled.
    pub poll: Duration,
    /// The longest a `service_healthy` or `service_completed_successfully`
    /// wait may take; on a swarm, the longest its services may take to
    /// converge.
    pub health_cap: Duration,
    /// On a swarm, how long a new task may stay pending (no node fits it)
    /// before the deploy fails.
    pub swarm_pending: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            exit_check: Duration::from_secs(10),
            poll: Duration::from_secs(1),
            health_cap: Duration::from_secs(300),
            swarm_pending: Duration::from_secs(120),
        }
    }
}

type Progress<'a> = &'a (dyn Fn(Value) + Send + Sync);

fn say(progress: Progress, step: &str, message: impl Into<String>) {
    progress(json!({"step": step, "message": message.into()}));
}

fn needs_exit_check(svc: &Service) -> bool {
    matches!(svc.restart.as_deref(), None | Some("no"))
}

/// Runs the plan's steps against the environment. `files` holds the
/// content of every secret and config the project's services use. On
/// error nothing is undone here; the caller runs `restore`.
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ops: &dyn DockerOps,
    project: &Project,
    files: &FileValues,
    builds: &BTreeMap<String, Prepared>,
    plan: &Plan,
    revision: i64,
    timing: &Timing,
    progress: Progress<'_>,
) -> Result<()> {
    if !plan.blocking.is_empty() {
        return Err(Error::Invalid(plan.blocking.join("; ")));
    }
    for img in plan.images.iter().filter(|i| i.pull) {
        say(progress, "pull", format!("pulling {}", img.image));
        let (repo, tag) = split_image_reference(&img.image)?;
        let mut s = ops.pull_image(&repo, &tag);
        while let Some(item) = s.next().await {
            let v = item?;
            if let Some(msg) = v["error"].as_str().or(v["errorDetail"]["message"].as_str()) {
                return Err(Error::Upstream {
                    status: 500,
                    message: format!("pull {}: {msg}", img.image),
                });
            }
            if let Some(status) = v["status"].as_str() {
                say(progress, "pull", format!("{}: {status}", img.image));
            }
        }
    }
    for img in plan.images.iter().filter(|i| i.build) {
        let prepared = builds
            .get(&img.image)
            .ok_or_else(|| Error::Internal(format!("no build was prepared for {}", img.image)))?;
        for note in &prepared.notes {
            say(progress, "build", note.clone());
        }
        say(progress, "build", format!("building {}", img.image));
        let fail = |msg: String| Error::Upstream {
            status: 500,
            message: format!("build {}: {msg}", img.image),
        };
        let mut s = ops.build_image(prepared.spec.clone());
        while let Some(item) = s.next().await {
            let v = item.map_err(|e| fail(e.to_string()))?;
            if let Some(msg) = v["error"].as_str().or(v["errorDetail"]["message"].as_str()) {
                return Err(fail(msg.to_string()));
            }
            if let Some(line) = v["stream"]
                .as_str()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
            {
                say(progress, "build", line.to_string());
            }
        }
    }
    for n in plan
        .networks
        .iter()
        .filter(|n| n.action == ResourceAction::Create)
    {
        if let Some((key, net)) = project.networks.iter().find(|(_, v)| v.name == n.name) {
            say(progress, "network", format!("creating network {}", n.name));
            ops.create_network(&network_create(project, key, net, revision))
                .await?;
        }
    }
    for v in plan
        .volumes
        .iter()
        .filter(|v| v.action == ResourceAction::Create)
    {
        if let Some((key, vol)) = project.volumes.iter().find(|(_, x)| x.name == v.name) {
            say(progress, "volume", format!("creating volume {}", v.name));
            ops.create_volume(&volume_create(project, key, vol, revision))
                .await?;
        }
    }

    let all = ops.list_containers().await?;
    // (id, name, state, replica number) per service.
    let mut current: BTreeMap<String, Vec<(String, String, String, u32)>> = BTreeMap::new();
    for c in stack_containers(&all, &project.name).filter(|c| !is_old(c)) {
        if let Some(svc) = service_of(c) {
            let number = c
                .labels
                .get(LABEL_NUMBER)
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            current.entry(svc.to_string()).or_default().push((
                c.id.clone(),
                c.name.clone(),
                c.state.clone(),
                number,
            ));
        }
    }
    let actions: BTreeMap<&str, ServiceAction> = plan
        .services
        .iter()
        .map(|s| (s.name.as_str(), s.action))
        .collect();
    // The ids each service runs as after this deploy, for dependency waits.
    let mut running: BTreeMap<String, Vec<String>> = current
        .iter()
        .map(|(s, cs)| (s.clone(), cs.iter().map(|c| c.0.clone()).collect()))
        .collect();

    // A restarting or paused container still holds its ports, so anything
    // not already stopped is stopped; those that were up are restarted by a
    // restore.
    let set_aside = |id: String, name: String, state: String| async move {
        let was_running = matches!(state.as_str(), "running" | "restarting" | "paused");
        if !matches!(state.as_str(), "exited" | "created" | "dead") {
            ops.container_action(&id, ContainerAction::Stop).await?;
        }
        ops.rename_container(&id, &format!("{name}{}", old_suffix(revision, was_running)))
            .await
    };

    for sname in start_order(project) {
        let svc = &project.services[&sname];
        let action = actions
            .get(sname.as_str())
            .copied()
            .unwrap_or(ServiceAction::Unchanged);
        if action == ServiceAction::Unchanged {
            continue;
        }
        for (dep, cond) in &svc.depends_on {
            let ids = running.get(dep).cloned().unwrap_or_default();
            wait_for(ops, project, dep, *cond, &ids, timing, progress).await?;
        }
        let existing = current.remove(&sname).unwrap_or_default();
        let mut keep: Vec<String> = Vec::new();
        let mut to_create: Vec<u32> = Vec::new();
        // Anonymous volumes of the replicas being replaced, by replica number,
        // so their data carries over as `compose up` does.
        let mut carried: BTreeMap<u32, Vec<(String, String)>> = BTreeMap::new();
        match action {
            ServiceAction::Create | ServiceAction::Recreate => {
                for (id, name, state, n) in existing {
                    carried.insert(n.max(1), anonymous_volumes(ops, project, &id).await?);
                    say(progress, "service", format!("setting aside {name}"));
                    set_aside(id, name, state).await?;
                }
                to_create = (1..=svc.replicas).collect();
            }
            ServiceAction::Scale => {
                for (id, name, state, n) in existing {
                    if n >= 1 && n <= svc.replicas {
                        keep.push(id);
                    } else {
                        say(progress, "service", format!("setting aside {name}"));
                        set_aside(id, name, state).await?;
                    }
                }
                let have: Vec<u32> = current_numbers(ops, project, &sname).await?;
                to_create = (1..=svc.replicas).filter(|n| !have.contains(n)).collect();
            }
            _ => {}
        }
        let mut started = Vec::new();
        for n in to_create {
            let name = container_name(project, svc, n);
            say(progress, "service", format!("creating {name}"));
            let mut body = container_body(project, svc, n, revision);
            if let Some(vols) = carried.get(&n) {
                carry_volumes(&mut body, vols);
            }
            let id = ops.create_container_raw(&name, &body).await?;
            if !svc.files.is_empty() {
                let n = svc.files.len();
                let what = if n == 1 { "file" } else { "files" };
                say(progress, "service", format!("writing {n} {what} to {name}"));
                // The error names the container, never a file's content.
                ops.put_archive(&id, "/", archive(svc, files))
                    .await
                    .map_err(|e| Error::Upstream {
                        status: 500,
                        message: format!("write files to {name}: {e}"),
                    })?;
            }
            ops.container_action(&id, ContainerAction::Start)
                .await
                .map_err(|e| Error::Upstream {
                    status: 500,
                    message: format!("start {name}: {e}"),
                })?;
            started.push((id, name));
        }
        if !started.is_empty() && needs_exit_check(svc) {
            tokio::time::sleep(timing.exit_check).await;
            for (id, name) in &started {
                let st = &ops.inspect_container(id).await?["State"];
                let code = st["ExitCode"].as_i64().unwrap_or(0);
                if !st["Running"].as_bool().unwrap_or(false) && code != 0 {
                    return Err(Error::Upstream {
                        status: 500,
                        message: format!("{name} exited with code {code}"),
                    });
                }
            }
        }
        keep.extend(started.into_iter().map(|(id, _)| id));
        running.insert(sname.clone(), keep);
    }

    for s in plan
        .services
        .iter()
        .filter(|s| s.action == ServiceAction::Remove)
    {
        for (id, name, state, _) in current.remove(&s.name).unwrap_or_default() {
            say(progress, "service", format!("setting aside {name}"));
            set_aside(id, name, state).await?;
        }
    }

    commit(ops, &project.name, plan, revision, progress).await;
    Ok(())
}

/// `(target, volume name)` of each anonymous volume mounted in container
/// `id`: a volume mount that is not one of the project's named volumes.
async fn anonymous_volumes(
    ops: &dyn DockerOps,
    project: &Project,
    id: &str,
) -> Result<Vec<(String, String)>> {
    let named: Vec<&str> = project.volumes.values().map(|v| v.name.as_str()).collect();
    let inspect = ops.inspect_container(id).await?;
    Ok(inspect["Mounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["Type"] == "volume")
        .filter_map(|m| Some((m["Destination"].as_str()?, m["Name"].as_str()?)))
        .filter(|(_, name)| !named.contains(name))
        .map(|(d, n)| (d.to_string(), n.to_string()))
        .collect())
}

/// Points the body's anonymous volume mounts (and image-declared volumes,
/// which have no mount) at the volumes the old container used.
fn carry_volumes(body: &mut Value, vols: &[(String, String)]) {
    let mounts = body["HostConfig"]["Mounts"]
        .as_array_mut()
        .expect("container_body always sets Mounts");
    for (target, name) in vols {
        match mounts.iter_mut().find(|m| m["Target"] == target.as_str()) {
            Some(m) if m["Type"] == "volume" && m["Source"] == "" => m["Source"] = json!(name),
            Some(_) => {}
            None => mounts.push(json!({"Type": "volume", "Source": name, "Target": target})),
        }
    }
}

/// Numbers of the service's live (not set-aside) replicas.
async fn current_numbers(ops: &dyn DockerOps, project: &Project, svc: &str) -> Result<Vec<u32>> {
    let all = ops.list_containers().await?;
    Ok(stack_containers(&all, &project.name)
        .filter(|c| !is_old(c) && service_of(c) == Some(svc))
        .filter_map(|c| c.labels.get(LABEL_NUMBER).and_then(|n| n.parse().ok()))
        .collect())
}

/// Deletes what the deploy set aside and the networks it drops. Past this
/// point the deploy stands; failures are reported, not undone.
async fn commit(
    ops: &dyn DockerOps,
    stack: &str,
    plan: &Plan,
    revision: i64,
    progress: Progress<'_>,
) {
    let all = ops.list_containers().await.unwrap_or_default();
    let ends = [old_suffix(revision, true), old_suffix(revision, false)];
    for c in stack_containers(&all, stack) {
        if ends.iter().any(|e| c.name.ends_with(e.as_str())) {
            if let Err(e) = ops.remove_container(&c.id, true, false).await {
                say(
                    progress,
                    "commit",
                    format!("could not remove {}: {e}", c.name),
                );
            }
        }
    }
    for n in plan
        .networks
        .iter()
        .filter(|n| n.action == ResourceAction::Remove)
    {
        if let Err(e) = ops.remove_network(&n.name).await {
            say(
                progress,
                "commit",
                format!("could not remove network {}: {e}", n.name),
            );
        }
    }
    say(progress, "commit", format!("revision {revision} deployed"));
}

async fn wait_for(
    ops: &dyn DockerOps,
    project: &Project,
    dep: &str,
    cond: DependsCondition,
    ids: &[String],
    timing: &Timing,
    progress: Progress<'_>,
) -> Result<()> {
    let limit = match cond {
        DependsCondition::Started => return Ok(()),
        DependsCondition::Healthy => {
            let hc = project
                .services
                .get(dep)
                .and_then(|s| s.healthcheck.clone())
                .filter(|h| !h.disable)
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "{dep} has no healthcheck, so service_healthy cannot be met"
                    ))
                })?;
            let budget = hc.start_period_secs + i64::from(hc.retries) * hc.interval_secs;
            timing
                .health_cap
                .min(Duration::from_secs(budget.max(0) as u64).max(timing.poll))
        }
        DependsCondition::CompletedSuccessfully => timing.health_cap,
    };
    say(progress, "wait", format!("waiting for {dep}"));
    for id in ids {
        let start = Instant::now();
        loop {
            let st = ops.inspect_container(id).await?["State"].clone();
            let running = st["Running"].as_bool().unwrap_or(false);
            let code = st["ExitCode"].as_i64().unwrap_or(0);
            match cond {
                DependsCondition::Healthy => {
                    match st["Health"]["Status"].as_str() {
                        Some("healthy") => break,
                        Some("unhealthy") => {
                            return Err(Error::Upstream {
                                status: 500,
                                message: format!("{dep} is unhealthy"),
                            })
                        }
                        _ => {}
                    }
                    if !running {
                        return Err(Error::Upstream {
                            status: 500,
                            message: format!(
                                "{dep} exited with code {code} before becoming healthy"
                            ),
                        });
                    }
                }
                DependsCondition::CompletedSuccessfully => {
                    if !running {
                        if code == 0 {
                            break;
                        }
                        return Err(Error::Upstream {
                            status: 500,
                            message: format!("{dep} exited with code {code}"),
                        });
                    }
                }
                DependsCondition::Started => break,
            }
            if start.elapsed() >= limit {
                return Err(Error::Upstream {
                    status: 500,
                    message: format!("{dep} did not become ready within {}s", limit.as_secs()),
                });
            }
            tokio::time::sleep(timing.poll).await;
        }
    }
    Ok(())
}

/// Runs a deploy in the background, once polled, and streams its progress
/// for the task manager. On failure the stack is restored, the revision is marked
/// failed, and the last message carries `error` so the task fails. The
/// stack's deploy lock is released either way.
#[allow(clippy::too_many_arguments)]
pub fn deploy_stream(
    state: AppState,
    ops: Arc<dyn DockerOps>,
    stack: StackRecord,
    revision: RevisionRecord,
    project: Project,
    files: FileValues,
    builds: BTreeMap<String, Prepared>,
    keep: BTreeSet<String>,
    plan: Plan,
    timing: Timing,
) -> BoxStream<Value> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    // The job starts when the task first polls the stream, so a task that
    // is never started never deploys.
    let job = async move {
        let send = {
            let tx = tx.clone();
            move |v: Value| {
                let _ = tx.send(v);
            }
        };
        let outcome = execute(
            ops.as_ref(),
            &project,
            &files,
            &builds,
            &plan,
            revision.number,
            &timing,
            &send,
        )
        .await;
        match outcome {
            Ok(()) => {
                for line in cleanup(ops.as_ref(), &project.name, &keep).await {
                    say(&send, "commit", line);
                }
                if let Err(e) = state
                    .store
                    .finish_revision(stack.id, revision.number, None)
                    .await
                {
                    tracing::warn!(error = %e, stack = %stack.name, "recording a deploy failed");
                }
            }
            Err(e) => {
                say(
                    &send,
                    "restore",
                    format!("deploy failed: {e}; restoring the stack"),
                );
                let (msg, _) = super::restore_and_settle(
                    &state,
                    ops.as_ref(),
                    &stack,
                    revision.number,
                    &e.to_string(),
                )
                .await;
                let _ = tx.send(json!({"error": msg}));
            }
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
