//! Planning a stack and starting its deploy (stacks spec §3–§4), shared by
//! the deploy route and git syncs (§8).

use super::build::{self as builds, BuildInputs};
use super::{deploy_stream, recover_stack, stack_containers, StackOps};
use crate::tasks::{NewTask, TaskInfo, TaskStatus};
use crate::AppState;
use fleetops_core::compose::docker::LABEL_PROJECT;
use fleetops_core::compose::files::{resolve, FileValues};
use fleetops_core::compose::plan::{plan, Live, Plan};
use fleetops_core::compose::swarm_plan::{outside_scope, plan_swarm};
use fleetops_core::compose::{parse_mode, parse_variables, ComposeIssue, FileKind, Mode, Project};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{Builds, NewAudit, RevisionFile, RevisionRecord, StackRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use utoipa::ToSchema;

/// Who starts a deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User(i64),
    /// A git sync (auto-deploy); audited as `system (git)`.
    Git,
}

/// How a deploy no user started is shown in the audit log.
pub const GIT_ACTOR: &str = "system (git)";

impl Actor {
    pub fn user_id(self) -> Option<i64> {
        match self {
            Actor::User(id) => Some(id),
            Actor::Git => None,
        }
    }

    /// Task owner; only admins see tasks owned by `0`.
    fn owner(self) -> i64 {
        self.user_id().unwrap_or(0)
    }
}

/// What a plan or deploy is made from.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum DeploySource {
    /// The stack's saved draft.
    Draft,
    /// An earlier revision (a rollback).
    Revision { revision: i64 },
}

#[derive(Serialize, ToSchema)]
pub struct PlanResponse {
    /// Compose problems; when any, there is no plan.
    pub issues: Vec<ComposeIssue>,
    pub warnings: Vec<ComposeIssue>,
    pub plan: Option<Plan>,
    /// The deployed revision's Compose file.
    pub compose_before: Option<String>,
    pub compose_after: String,
    /// Names of variables added, removed or changed (never values).
    pub variables_changed: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DeployStarted {
    pub task_id: String,
    pub revision: i64,
}

pub(crate) fn issues_message(issues: &[ComposeIssue]) -> String {
    issues
        .iter()
        .map(|i| {
            if i.path.is_empty() {
                i.message.clone()
            } else {
                format!("{}: {}", i.path, i.message)
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// What a plan or deploy of `from` is made of.
pub(crate) struct Source {
    compose: String,
    variables: Vec<String>,
    rollback_of: Option<i64>,
    /// The git commit the file came from, on a git stack.
    commit: Option<String>,
    /// Values for `file:` secrets and configs.
    values: FileValues,
}

fn file_kind(kind: &str) -> FileKind {
    if kind == "secret" {
        FileKind::Secret
    } else {
        FileKind::Config
    }
}

/// The values a deploy of the draft uses for `file:` sources: secrets from
/// the Secrets tab; configs from the repository on a git stack, and from
/// the Secrets tab otherwise.
pub(crate) async fn draft_values(s: &AppState, stack: &StackRecord) -> Result<FileValues> {
    let config_source = if stack.source == "git" {
        "repo"
    } else {
        "fleetops"
    };
    Ok(s.store
        .stack_files(stack.id)
        .await?
        .into_iter()
        .filter(|f| match f.kind.as_str() {
            "secret" => f.source == "fleetops",
            _ => f.source == config_source,
        })
        .map(|f| ((file_kind(&f.kind), f.name), f.value))
        .collect())
}

/// What revision `number` deployed, as values.
pub(crate) async fn revision_values(
    s: &AppState,
    stack: &StackRecord,
    number: i64,
) -> Result<FileValues> {
    Ok(s.store
        .revision_files(stack.id, number)
        .await?
        .into_iter()
        .map(|f| ((file_kind(&f.kind), f.name), f.value))
        .collect())
}

async fn source(s: &AppState, stack: &StackRecord, from: DeploySource) -> Result<Source> {
    let git = stack.source == "git";
    match from {
        DeploySource::Draft => {
            // The file and its commit are read together, so a sync landing
            // meanwhile cannot label one with the other's commit.
            let (d, commit) = s
                .store
                .git_draft(stack.id)
                .await?
                .ok_or_else(|| Error::NotFound(format!("draft of stack '{}'", stack.name)))?;
            Ok(Source {
                compose: d.compose,
                variables: d.variables,
                rollback_of: None,
                commit: commit.filter(|_| git),
                values: draft_values(s, stack).await?,
            })
        }
        DeploySource::Revision { revision } => {
            let r = s.store.revision(stack.id, revision).await?;
            let v = s.store.revision_variables(stack.id, revision).await?;
            Ok(Source {
                compose: r.compose,
                variables: v,
                rollback_of: Some(revision),
                commit: r.git_commit.filter(|_| git),
                values: revision_values(s, stack, revision).await?,
            })
        }
    }
}

/// The deployed revision: its record, variables and (if it still parses)
/// project, with its files resolved from the revision's snapshot.
pub(crate) async fn deployed(
    s: &AppState,
    stack: &StackRecord,
    mode: Mode,
) -> Result<Option<(RevisionRecord, Vec<String>, Option<Project>)>> {
    let Some(n) = stack.deployed_revision else {
        return Ok(None);
    };
    let r = s.store.revision(stack.id, n).await?;
    let v = s.store.revision_variables(stack.id, n).await?;
    let mut p = parse_mode(&r.compose, &v, &stack.name, mode)
        .ok()
        .map(|p| p.project);
    if let Some(p) = p.as_mut() {
        let values = revision_values(s, stack, n).await?;
        // A file the snapshot lacks keeps an empty digest, which reads as
        // changed: the safe side.
        let _ = resolve(p, &v, &values, &s.store.file_digest_key());
        if mode == Mode::Compose {
            let inputs = builds::revision_inputs(s, stack, n).await?;
            let _ = fleetops_core::compose::build::apply(
                p,
                &inputs.trees,
                inputs.git,
                &s.store.build_key(),
            );
        }
    }
    Ok(Some((r, v, p)))
}

async fn live(ops: &dyn DockerOps, stack: &str) -> Result<Live> {
    let in_stack = |labels: &std::collections::BTreeMap<String, String>| {
        labels.get(LABEL_PROJECT).map(String::as_str) == Some(stack)
    };
    let containers = ops.list_containers().await?;
    Ok(Live {
        containers: stack_containers(&containers, stack).cloned().collect(),
        networks: ops
            .list_networks()
            .await?
            .into_iter()
            .filter(|n| in_stack(&n.labels))
            .collect(),
        volumes: ops
            .list_volumes()
            .await?
            .into_iter()
            .filter(|v| in_stack(&v.labels))
            .collect(),
        images: ops
            .list_images()
            .await?
            .into_iter()
            .flat_map(|i| i.repo_tags)
            .collect(),
    })
}

fn variables_changed(before: &[String], after: &[String]) -> Vec<String> {
    let a = parse_variables(before).unwrap_or_default();
    let b = parse_variables(after).unwrap_or_default();
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

/// A plan ready to deploy: the project, the plan, the content of its files
/// and what its builds come from.
pub(crate) struct Ready {
    pub project: Project,
    pub plan: Plan,
    pub files: FileValues,
    pub inputs: BuildInputs,
}

/// The plan of `from` for `stack`, with what was planned and what it was
/// made from. Issues mean the file does not parse, a secret or config has
/// no value, or a build cannot be made.
pub(crate) type Planned = (PlanResponse, Option<Ready>, Source);

/// `scope`: on a swarm, the stacks the deployer may use (swarm spec §6);
/// `None` is every stack.
pub(crate) async fn make_plan(
    s: &AppState,
    ops: &StackOps,
    stack: &StackRecord,
    from: DeploySource,
    scope: Option<&BTreeSet<String>>,
) -> Result<Planned> {
    let mode = ops.mode();
    match ops {
        StackOps::Docker(d) => recover_stack(s, d.as_ref(), stack).await?,
        StackOps::Swarm(_) => super::swarm::settle_interrupted(s, stack).await?,
    }
    let src = source(s, stack, from).await?;
    let dep = deployed(s, stack, mode).await?;
    let compose_before = dep.as_ref().map(|(r, _, _)| r.compose.clone());
    let changed = variables_changed(
        dep.as_ref().map(|(_, v, _)| v.as_slice()).unwrap_or(&[]),
        &src.variables,
    );
    let mut parsed =
        parse_mode(&src.compose, &src.variables, &stack.name, mode).and_then(|mut parsed| {
            let files = resolve(
                &mut parsed.project,
                &src.variables,
                &src.values,
                &s.store.file_digest_key(),
            )?;
            Ok((parsed, files, BuildInputs::default()))
        });
    // A Swarm stack never builds (its parser refuses `build:`).
    if let (Ok((parsed_ok, _, inputs)), StackOps::Docker(_)) = (&mut parsed, ops) {
        let (found, issues) = match from {
            DeploySource::Draft => {
                builds::draft_inputs(s, stack, src.commit.as_deref(), &parsed_ok.project).await?
            }
            DeploySource::Revision { revision } => {
                (builds::revision_inputs(s, stack, revision).await?, vec![])
            }
        };
        *inputs = found;
        let applied = if issues.is_empty() {
            fleetops_core::compose::build::apply(
                &mut parsed_ok.project,
                &inputs.trees,
                inputs.git,
                &s.store.build_key(),
            )
        } else {
            Err(issues)
        };
        if let Err(issues) = applied {
            parsed = Err(issues);
        }
    }
    match parsed {
        Err(issues) => Ok((
            PlanResponse {
                issues,
                warnings: vec![],
                plan: None,
                compose_before,
                compose_after: src.compose.clone(),
                variables_changed: changed,
            },
            None,
            src,
        )),
        Ok((parsed, files, inputs)) => {
            let prev = dep
                .as_ref()
                .and_then(|(r, _, p)| p.as_ref().map(|p| (p, r.number)));
            let (p, issues) = match ops {
                StackOps::Docker(d) => {
                    let live = live(d.as_ref(), &stack.name).await?;
                    let p = plan(&parsed.project, prev, &live);
                    let issues =
                        builds::check_rebuilds(s, stack, &parsed.project, &p, &inputs).await?;
                    (p, issues)
                }
                StackOps::Swarm(sw) => {
                    let live = super::swarm::live(sw.as_ref(), &stack.name).await?;
                    let mut p = plan_swarm(&parsed.project, prev, &live);
                    super::swarm::protect(&mut p, &live, &stack.name);
                    if let Some(scope) = scope {
                        p.blocking
                            .extend(outside_scope(&parsed.project, &live, scope));
                    }
                    (p, vec![])
                }
            };
            if !issues.is_empty() {
                return Ok((
                    PlanResponse {
                        issues,
                        warnings: parsed.warnings,
                        plan: None,
                        compose_before,
                        compose_after: src.compose.clone(),
                        variables_changed: changed,
                    },
                    None,
                    src,
                ));
            }
            Ok((
                PlanResponse {
                    issues: vec![],
                    warnings: parsed.warnings,
                    plan: Some(p.clone()),
                    compose_before,
                    compose_after: src.compose.clone(),
                    variables_changed: changed,
                },
                Some(Ready {
                    project: parsed.project,
                    plan: p,
                    files,
                    inputs,
                }),
                src,
            ))
        }
    }
}

/// Plans `from` again and deploys it in the background. `plan_token` is the
/// reviewed plan's token; only auto-deploys pass `None`.
#[allow(clippy::too_many_arguments)]
pub async fn start_deploy(
    s: &AppState,
    ops: StackOps,
    env: Arc<dyn Environment>,
    stack: &StackRecord,
    from: DeploySource,
    plan_token: Option<&str>,
    actor: Actor,
    scope: Option<&BTreeSet<String>>,
) -> Result<DeployStarted> {
    let (resp, planned, src) = make_plan(s, &ops, stack, from, scope).await?;
    let Source {
        variables,
        rollback_of,
        commit,
        ..
    } = src;
    let Some(Ready {
        project,
        plan: p,
        files,
        inputs,
    }) = planned
    else {
        return Err(Error::Invalid(issues_message(&resp.issues)));
    };
    if plan_token.is_some_and(|t| t != p.token) {
        return Err(Error::Conflict(
            "the stack changed since this plan was made; review again".into(),
        ));
    }
    if !p.blocking.is_empty() {
        return Err(Error::Invalid(p.blocking.join("; ")));
    }
    // A Swarm stack builds nothing: Swarm pulls each image on its nodes.
    let (built, keep, prepared) = match &ops {
        StackOps::Docker(_) => {
            let built: Builds = builds::builds_of(&project, &inputs);
            // Images of the deploy before this one stay, for a quick rollback.
            let mut keep: BTreeSet<String> = built.values().map(|b| b.image.clone()).collect();
            if let Some(n) = stack.deployed_revision {
                keep.extend(
                    s.store
                        .revision_builds(stack.id, n)
                        .await?
                        .into_values()
                        .map(|b| b.image),
                );
            }
            let prepared = builds::prepare(s, stack, &project, &p, &inputs).await?;
            (built, keep, prepared)
        }
        StackOps::Swarm(_) => Default::default(),
    };
    if !s.stack_locks.lock().unwrap().insert(stack.id) {
        return Err(Error::Conflict(
            "a deploy of this stack is already running".into(),
        ));
    }
    let action = if rollback_of.is_some() {
        "stack.rollback"
    } else {
        "stack.deploy"
    };
    let created = s
        .store
        .create_revision(
            stack.id,
            &resp.compose_after,
            &variables,
            if rollback_of.is_some() {
                "rollback"
            } else {
                "deploy"
            },
            rollback_of,
            commit.as_deref(),
            actor.user_id(),
            now_unix(),
            &files
                .iter()
                .map(|((kind, name), value)| RevisionFile {
                    kind: kind.as_str().into(),
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect::<Vec<_>>(),
            &built,
        )
        .await;
    let rev = match created {
        Ok(r) => r,
        Err(e) => {
            s.stack_locks.lock().unwrap().remove(&stack.id);
            return Err(e);
        }
    };
    // A rolled-back git stack stops following its branch until resumed.
    let paused = stack.source == "git" && rollback_of.is_some();
    if paused {
        if let Err(e) = s.store.set_git_paused(stack.id, true).await {
            tracing::warn!(error = %e, stack = %stack.name, "pausing git sync failed");
        }
    }
    let number = rev.number;
    let stream = match &ops {
        StackOps::Docker(d) => deploy_stream(
            s.clone(),
            d.clone(),
            stack.clone(),
            rev,
            project,
            files,
            prepared,
            keep,
            p,
            s.config.stack_timing,
        ),
        StackOps::Swarm(sw) => super::swarm::deploy_stream(
            s.clone(),
            env.clone(),
            sw.clone(),
            stack.clone(),
            rev,
            project,
            files,
            p,
            s.config.stack_timing,
        ),
    };
    let store = s.store.clone();
    let (env_id, stack_id, stack_name, is_git) = (
        env.id(),
        stack.id,
        stack.name.clone(),
        stack.source == "git",
    );
    let spawned = s.tasks.spawn(
        NewTask {
            kind: "stack.deploy",
            env: env.name().to_string(),
            target: stack.name.clone(),
            owner: actor.owner(),
        },
        stream,
        move |info: TaskInfo| async move {
            let ok = matches!(info.status, TaskStatus::Succeeded);
            let mut detail = if ok {
                json!({ "task_id": info.id, "revision": number, "rollback_of": rollback_of })
            } else {
                json!({ "task_id": info.id, "revision": number, "error": info.error })
            };
            if let Some(c) = &commit {
                detail["commit"] = json!(c);
            }
            if paused {
                detail["paused"] = json!(true);
            }
            if actor == Actor::Git {
                detail["by"] = json!(GIT_ACTOR);
            }
            if is_git {
                settle_git(&store, stack_id, commit.as_deref(), ok, actor, &info).await;
            }
            let entry = NewAudit {
                ts: now_unix(),
                user_id: actor.user_id(),
                environment_id: Some(env_id),
                action,
                target_type: Some("stack"),
                target_id: Some(&stack_name),
                result: if ok { "success" } else { "failure" },
                detail: Some(detail),
            };
            if let Err(e) = store.insert_audit(&entry).await {
                tracing::error!(error = %e, "failed to write audit log");
            }
        },
    );
    match spawned {
        Ok(t) => Ok(DeployStarted {
            task_id: t.info().id,
            revision: number,
        }),
        Err(e) => {
            // The stream was never polled, so nothing was deployed.
            let _ = s
                .store
                .finish_revision(stack.id, number, Some(&e.to_string()))
                .await;
            s.stack_locks.lock().unwrap().remove(&stack.id);
            Err(e)
        }
    }
}

/// After a git stack's deploy: a deployed pending commit is no longer
/// pending, and a failed auto-deploy shows on the stack.
async fn settle_git(
    store: &fleetops_store::Store,
    stack_id: i64,
    commit: Option<&str>,
    ok: bool,
    actor: Actor,
    info: &TaskInfo,
) {
    let res = if ok {
        match store.stack_git(stack_id).await {
            Ok(Some(g)) if g.pending_commit.is_some() && g.pending_commit.as_deref() == commit => {
                store.set_git_pending(stack_id, None).await
            }
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    } else if actor == Actor::Git {
        let short = commit.map(|c| &c[..c.len().min(7)]).unwrap_or("?");
        let err = info.error.as_deref().unwrap_or("unknown error");
        store
            .record_git_error(
                stack_id,
                &format!("auto-deploy of {short} failed: {err}"),
                now_unix(),
            )
            .await
    } else {
        Ok(())
    };
    if let Err(e) = res {
        tracing::warn!(error = %e, stack_id, "recording a git deploy failed");
    }
}
