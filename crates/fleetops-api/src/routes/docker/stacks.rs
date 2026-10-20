//! Stacks: Compose projects FleetOps authors and deploys (stacks spec §5–§6).

use super::compose::ComposeOutcome;
use super::{ctx, EnvCtx};
use crate::openapi::EnvParam;
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath, ApiQuery},
    stacks::{deploy_stream, is_old, recover_stack, stack_containers, try_lock},
    tasks::{NewTask, TaskInfo, TaskStatus},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::compose::docker::LABEL_PROJECT;
use fleetops_core::compose::plan::{plan, Live, Plan};
use fleetops_core::compose::{parse, parse_variables, validate_stack_name, ComposeIssue, Project};
use fleetops_core::docker::{mask_env, ContainerAction};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{Draft, NewAudit, RevisionRecord, StackRecord};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use utoipa::{IntoParams, ToSchema};

/// Largest Compose file a stack takes.
pub const COMPOSE_MAX: usize = 1024 * 1024;
/// Largest variable set a stack takes, in bytes.
pub const VARIABLES_MAX: usize = 64 * 1024;

const REVEAL: Permission = Permission::new(Row::Stacks, Verb::Reveal);

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StackSummary {
    pub name: String,
    /// inline | git
    pub source: String,
    pub deployed_revision: Option<i64>,
    /// The newest revision's status, if any.
    pub last_status: Option<String>,
    pub updated_at: i64,
    /// The saved draft differs from the deployed revision.
    pub draft_differs: bool,
}

#[derive(Serialize, ToSchema)]
pub struct StackDetail {
    pub stack: StackRecord,
    /// Variable values are masked without `stacks.reveal`.
    pub draft: Draft,
    pub deployed: Option<RevisionRecord>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateStackBody {
    pub name: String,
    pub compose: String,
    /// `KEY=value` lines.
    #[serde(default)]
    pub variables: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct DraftBody {
    pub compose: String,
    #[serde(default)]
    pub variables: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ValidateBody {
    pub name: String,
    pub compose: String,
    #[serde(default)]
    pub variables: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ValidateResponse {
    pub issues: Vec<ComposeIssue>,
    pub warnings: Vec<ComposeIssue>,
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

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployBody {
    pub from: DeploySource,
    /// The `token` of the plan that was reviewed.
    pub plan_token: String,
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

#[derive(Serialize, ToSchema)]
pub struct RevisionDetail {
    pub revision: RevisionRecord,
    /// Masked without `stacks.reveal`.
    pub variables: Vec<String>,
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RemoveQuery {
    /// Also remove the stack's named volumes.
    #[serde(default)]
    pub volumes: bool,
}

#[derive(ToSchema)]
#[schema(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum StackActionName {
    Start,
    Stop,
    Restart,
}

fn check_size(compose: &str, variables: &[String]) -> Result<()> {
    if compose.len() > COMPOSE_MAX {
        return Err(Error::Invalid(
            "the Compose file is larger than 1 MiB".into(),
        ));
    }
    if variables.iter().map(|v| v.len() + 1).sum::<usize>() > VARIABLES_MAX {
        return Err(Error::Invalid(
            "the variables are larger than 64 KiB".into(),
        ));
    }
    Ok(())
}

/// Puts back the stored value of each `KEY=<mask>` line. A masked value
/// with no stored value (a renamed key, say) is an error, never dropped.
fn unmask_variables(lines: &mut [String], stored: &[String]) -> Result<()> {
    let masked = format!("={}", fleetops_core::kube::MASK);
    for line in lines.iter_mut() {
        let Some(key) = line.strip_suffix(&masked) else {
            continue;
        };
        let prefix = format!("{key}=");
        match stored.iter().find(|s| s.starts_with(&prefix)) {
            Some(real) => *line = real.clone(),
            None => {
                return Err(Error::Invalid(format!(
                    "the value of {key} is hidden and not stored; enter it again"
                )))
            }
        }
    }
    Ok(())
}

fn issues_message(issues: &[ComposeIssue]) -> String {
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

async fn audit<T>(
    s: &AppState,
    c: &EnvCtx,
    action: &str,
    stack: &str,
    outcome: &Result<T>,
    detail: Option<Value>,
) {
    let (result, detail) = match outcome {
        Ok(_) => ("success", detail),
        Err(e) => {
            let mut d = detail.unwrap_or_else(|| json!({}));
            d["error"] = json!(e.to_string());
            ("failure", Some(d))
        }
    };
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(c.user.user.id),
        environment_id: Some(c.env.id()),
        action,
        target_type: Some("stack"),
        target_id: Some(stack),
        result,
        detail,
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}

/// The compose and variables `from` names, and the revision it rolls back to.
async fn source(
    s: &AppState,
    stack: &StackRecord,
    from: DeploySource,
) -> Result<(String, Vec<String>, Option<i64>)> {
    match from {
        DeploySource::Draft => {
            let d = s
                .store
                .draft(stack.id)
                .await?
                .ok_or_else(|| Error::NotFound(format!("draft of stack '{}'", stack.name)))?;
            Ok((d.compose, d.variables, None))
        }
        DeploySource::Revision { revision } => {
            let r = s.store.revision(stack.id, revision).await?;
            let v = s.store.revision_variables(stack.id, revision).await?;
            Ok((r.compose, v, Some(revision)))
        }
    }
}

/// The deployed revision: its record, variables and (if it still parses)
/// project.
async fn deployed(
    s: &AppState,
    stack: &StackRecord,
) -> Result<Option<(RevisionRecord, Vec<String>, Option<Project>)>> {
    let Some(n) = stack.deployed_revision else {
        return Ok(None);
    };
    let r = s.store.revision(stack.id, n).await?;
    let v = s.store.revision_variables(stack.id, n).await?;
    let p = parse(&r.compose, &v, &stack.name).ok().map(|p| p.project);
    Ok(Some((r, v, p)))
}

async fn live(c: &EnvCtx, stack: &str) -> Result<Live> {
    let in_stack = |labels: &std::collections::BTreeMap<String, String>| {
        labels.get(LABEL_PROJECT).map(String::as_str) == Some(stack)
    };
    let containers = c.ops.list_containers().await?;
    Ok(Live {
        containers: stack_containers(&containers, stack).cloned().collect(),
        networks: c
            .ops
            .list_networks()
            .await?
            .into_iter()
            .filter(|n| in_stack(&n.labels))
            .collect(),
        volumes: c
            .ops
            .list_volumes()
            .await?
            .into_iter()
            .filter(|v| in_stack(&v.labels))
            .collect(),
        images: c
            .ops
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

async fn summary(s: &AppState, stack: StackRecord) -> Result<StackSummary> {
    let revs = s.store.list_revisions(stack.id).await?;
    let draft = s.store.draft(stack.id).await?;
    let latest = revs.first();
    let deployed = match stack.deployed_revision {
        Some(n) => revs.iter().find(|r| r.number == n),
        None => None,
    };
    let draft_differs = match (&draft, deployed) {
        (Some(d), Some(r)) => {
            d.compose != r.compose
                || d.variables != s.store.revision_variables(stack.id, r.number).await?
        }
        (Some(_), None) => true,
        _ => false,
    };
    let updated_at = [
        Some(stack.created_at),
        draft.as_ref().map(|d| d.updated_at),
        latest.map(|r| r.created_at),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);
    Ok(StackSummary {
        name: stack.name.clone(),
        source: stack.source.clone(),
        deployed_revision: stack.deployed_revision,
        last_status: latest.map(|r| r.status.clone()),
        updated_at,
        draft_differs,
    })
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks",
    operation_id = "listStacks",
    tag = "stacks",
    summary = "Stacks on the environment",
    params(EnvParam),
    responses((status = 200, description = "Stacks", body = Vec<StackSummary>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<StackSummary>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let mut out = Vec::new();
    for st in s.store.list_stacks(c.env.id()).await? {
        out.push(summary(&s, st).await?);
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks",
    operation_id = "createStack",
    tag = "stacks",
    summary = "Create a stack with its first draft",
    params(EnvParam),
    request_body = CreateStackBody,
    responses((status = 201, description = "Created", body = StackSummary))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<CreateStackBody>,
) -> ApiResult<(StatusCode, Json<StackSummary>)> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    validate_stack_name(&body.name).map_err(Error::Invalid)?;
    check_size(&body.compose, &body.variables)?;
    let res = s
        .store
        .create_stack(
            c.env.id(),
            &body.name,
            &body.compose,
            &body.variables,
            Some(c.user.user.id),
            now_unix(),
        )
        .await;
    audit(&s, &c, "stack.create", &body.name, &res, None).await;
    Ok((StatusCode::CREATED, Json(summary(&s, res?).await?)))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/validate",
    operation_id = "validateStack",
    tag = "stacks",
    summary = "Check a Compose file without saving it",
    params(EnvParam),
    request_body = ValidateBody,
    responses((status = 200, description = "Problems and warnings", body = ValidateResponse))
)]
pub async fn validate(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<ValidateBody>,
) -> ApiResult<Json<ValidateResponse>> {
    ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    check_size(&body.compose, &body.variables)?;
    Ok(Json(
        match parse(&body.compose, &body.variables, &body.name) {
            Ok(p) => ValidateResponse {
                issues: vec![],
                warnings: p.warnings,
            },
            Err(issues) => ValidateResponse {
                issues,
                warnings: vec![],
            },
        },
    ))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks/{stack}",
    operation_id = "getStack",
    tag = "stacks",
    summary = "A stack, its draft and its deployed revision",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "The stack", body = StackDetail))
)]
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<StackDetail>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    let mut draft = s.store.draft(stack.id).await?.unwrap_or(Draft {
        compose: String::new(),
        variables: vec![],
        updated_at: stack.created_at,
    });
    if !c.grants.allows(REVEAL, None) {
        mask_env(&mut draft.variables);
    }
    let deployed = match stack.deployed_revision {
        Some(n) => Some(s.store.revision(stack.id, n).await?),
        None => None,
    };
    Ok(Json(StackDetail {
        stack,
        draft,
        deployed,
    }))
}

#[utoipa::path(
    put,
    path = "/envs/{env}/stacks/{stack}/draft",
    operation_id = "saveStackDraft",
    tag = "stacks",
    summary = "Save the stack's draft",
    description = "Masked variable values are replaced by the stored ones.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    request_body = DraftBody,
    responses((status = 204, description = "Saved"))
)]
pub async fn save_draft(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiJson(mut body): ApiJson<DraftBody>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    check_size(&body.compose, &body.variables)?;
    let stored = s
        .store
        .draft(stack.id)
        .await?
        .map(|d| d.variables)
        .unwrap_or_default();
    unmask_variables(&mut body.variables, &stored)?;
    let res = s
        .store
        .save_draft(
            stack.id,
            &body.compose,
            &body.variables,
            Some(c.user.user.id),
            now_unix(),
        )
        .await;
    audit(&s, &c, "stack.save", &name, &res, None).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

/// Plans `from` for `stack`. `Err` issues mean the file does not parse.
async fn make_plan(
    s: &AppState,
    c: &EnvCtx,
    stack: &StackRecord,
    from: DeploySource,
) -> Result<(
    PlanResponse,
    Option<(Project, Plan)>,
    Vec<String>,
    Option<i64>,
)> {
    recover_stack(s, c.ops.as_ref(), stack).await?;
    let (compose, variables, rollback_of) = source(s, stack, from).await?;
    let dep = deployed(s, stack).await?;
    let compose_before = dep.as_ref().map(|(r, _, _)| r.compose.clone());
    let changed = variables_changed(
        dep.as_ref().map(|(_, v, _)| v.as_slice()).unwrap_or(&[]),
        &variables,
    );
    match parse(&compose, &variables, &stack.name) {
        Err(issues) => Ok((
            PlanResponse {
                issues,
                warnings: vec![],
                plan: None,
                compose_before,
                compose_after: compose.clone(),
                variables_changed: changed,
            },
            None,
            variables,
            rollback_of,
        )),
        Ok(parsed) => {
            let live = live(c, &stack.name).await?;
            let prev = dep
                .as_ref()
                .and_then(|(r, _, p)| p.as_ref().map(|p| (p, r.number)));
            let p = plan(&parsed.project, prev, &live);
            Ok((
                PlanResponse {
                    issues: vec![],
                    warnings: parsed.warnings,
                    plan: Some(p.clone()),
                    compose_before,
                    compose_after: compose.clone(),
                    variables_changed: changed,
                },
                Some((parsed.project, p)),
                variables,
                rollback_of,
            ))
        }
    }
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/plan",
    operation_id = "planStack",
    tag = "stacks",
    summary = "What deploying the draft or an earlier revision would do",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    request_body = DeploySource,
    responses((status = 200, description = "The plan, or the Compose problems", body = PlanResponse))
)]
pub async fn plan_stack(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiJson(from): ApiJson<DeploySource>,
) -> ApiResult<Json<PlanResponse>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    Ok(Json(make_plan(&s, &c, &stack, from).await?.0))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/deploy",
    operation_id = "deployStack",
    tag = "stacks",
    summary = "Deploy the reviewed plan in the background",
    description = "Fails with 409 when the stack or environment changed since the plan was made.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    request_body = DeployBody,
    responses((status = 202, description = "Started; follow it on the websocket `task` topic", body = DeployStarted))
)]
pub async fn deploy(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<DeployBody>,
) -> ApiResult<(StatusCode, Json<DeployStarted>)> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Operate)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    let (resp, planned, variables, rollback_of) = make_plan(&s, &c, &stack, body.from).await?;
    let Some((project, p)) = planned else {
        return Err(Error::Invalid(issues_message(&resp.issues)).into());
    };
    if p.token != body.plan_token {
        return Err(Error::Conflict(
            "the stack changed since this plan was made; review again".into(),
        )
        .into());
    }
    if !p.blocking.is_empty() {
        return Err(Error::Invalid(p.blocking.join("; ")).into());
    }
    if !s.stack_locks.lock().unwrap().insert(stack.id) {
        return Err(Error::Conflict("a deploy of this stack is already running".into()).into());
    }
    let action = if rollback_of.is_some() {
        "stack.rollback"
    } else {
        "stack.deploy"
    };
    let rev = match s
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
            None,
            Some(c.user.user.id),
            now_unix(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            s.stack_locks.lock().unwrap().remove(&stack.id);
            return Err(e.into());
        }
    };
    let number = rev.number;
    let stream = deploy_stream(
        s.clone(),
        c.ops.clone(),
        stack.clone(),
        rev,
        project,
        p,
        s.config.stack_timing,
    );
    let store = s.store.clone();
    let (user_id, env_id, stack_name) = (c.user.user.id, c.env.id(), stack.name.clone());
    let spawned = s.tasks.spawn(
        NewTask {
            kind: "stack.deploy",
            env: c.env.name().to_string(),
            target: stack.name.clone(),
            owner: user_id,
        },
        stream,
        move |info: TaskInfo| async move {
            let (result, detail) = match info.status {
                TaskStatus::Succeeded => (
                    "success",
                    json!({ "task_id": info.id, "revision": number, "rollback_of": rollback_of }),
                ),
                _ => (
                    "failure",
                    json!({ "task_id": info.id, "revision": number, "error": info.error }),
                ),
            };
            let entry = NewAudit {
                ts: now_unix(),
                user_id: Some(user_id),
                environment_id: Some(env_id),
                action,
                target_type: Some("stack"),
                target_id: Some(&stack_name),
                result,
                detail: Some(detail),
            };
            if let Err(e) = store.insert_audit(&entry).await {
                tracing::error!(error = %e, "failed to write audit log");
            }
        },
    );
    let task = match spawned {
        Ok(t) => t,
        Err(e) => {
            // The stream was never polled, so nothing was deployed.
            let _ = s
                .store
                .finish_revision(stack.id, number, Some(&e.to_string()))
                .await;
            s.stack_locks.lock().unwrap().remove(&stack.id);
            return Err(e.into());
        }
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(DeployStarted {
            task_id: task.info().id,
            revision: number,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks/{stack}/revisions",
    operation_id = "listStackRevisions",
    tag = "stacks",
    summary = "The stack's revisions, newest first",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "Revisions", body = Vec<RevisionRecord>))
)]
pub async fn revisions(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<Vec<RevisionRecord>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    Ok(Json(s.store.list_revisions(stack.id).await?))
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks/{stack}/revisions/{n}",
    operation_id = "getStackRevision",
    tag = "stacks",
    summary = "One revision with its variables",
    params(EnvParam, ("stack" = String, Path, description = "Stack name"), ("n" = i64, Path, description = "Revision number")),
    responses((status = 200, description = "The revision", body = RevisionDetail))
)]
pub async fn revision(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name, n)): ApiPath<(String, String, i64)>,
) -> ApiResult<Json<RevisionDetail>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    let revision = s.store.revision(stack.id, n).await?;
    let mut variables = s.store.revision_variables(stack.id, n).await?;
    if !c.grants.allows(REVEAL, None) {
        mask_env(&mut variables);
    }
    Ok(Json(RevisionDetail {
        revision,
        variables,
    }))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/{action}",
    operation_id = "stackAction",
    tag = "stacks",
    summary = "Start, stop or restart the stack's containers",
    params(EnvParam, ("stack" = String, Path, description = "Stack name"), ("action" = StackActionName, Path)),
    responses((status = 200, description = "Every container was attempted; failures are listed", body = ComposeOutcome))
)]
pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name, action)): ApiPath<(String, String, String)>,
) -> ApiResult<Json<ComposeOutcome>> {
    let op = match action.as_str() {
        "start" => ContainerAction::Start,
        "stop" => ContainerAction::Stop,
        "restart" => ContainerAction::Restart,
        other => return Err(Error::NotFound(format!("unknown stack action '{other}'")).into()),
    };
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Operate)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    recover_stack(&s, c.ops.as_ref(), &stack).await?;
    let _lock = try_lock(&s, stack.id)
        .ok_or_else(|| Error::Conflict("a deploy of this stack is running".into()))?;
    let all = c.ops.list_containers().await?;
    let members: Vec<_> = stack_containers(&all, &name)
        .filter(|c| !is_old(c))
        .collect();
    let mut errors = Vec::new();
    for m in &members {
        if let Err(e) = c.ops.container_action(&m.id, op).await {
            errors.push(format!("{}: {e}", m.name));
        }
    }
    let outcome = if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Upstream {
            status: 500,
            message: errors.join("; "),
        })
    };
    audit(&s, &c, &format!("stack.{action}"), &name, &outcome, None).await;
    Ok(Json(ComposeOutcome {
        affected: members.len() - errors.len(),
        errors,
    }))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/stacks/{stack}",
    operation_id = "removeStack",
    tag = "stacks",
    summary = "Remove the stack's containers and networks, and optionally its volumes",
    params(EnvParam, ("stack" = String, Path, description = "Stack name"), RemoveQuery),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiQuery(q): ApiQuery<RemoveQuery>,
) -> ApiResult<StatusCode> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Delete)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    let _lock = try_lock(&s, stack.id)
        .ok_or_else(|| Error::Conflict("a deploy of this stack is running".into()))?;
    let outcome = remove_objects(&c, &name, q.volumes).await;
    let outcome = match outcome {
        Ok(()) => s.store.delete_stack(stack.id).await,
        Err(e) => Err(e),
    };
    audit(
        &s,
        &c,
        "stack.remove",
        &name,
        &outcome,
        Some(json!({ "volumes": q.volumes })),
    )
    .await;
    outcome?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_objects(c: &EnvCtx, stack: &str, volumes: bool) -> Result<()> {
    let mut errors = Vec::new();
    let all = c.ops.list_containers().await?;
    for m in stack_containers(&all, stack) {
        if let Err(e) = c.ops.remove_container(&m.id, true, false).await {
            errors.push(format!("{}: {e}", m.name));
        }
    }
    let ours = |labels: &std::collections::BTreeMap<String, String>| {
        labels.get(LABEL_PROJECT).map(String::as_str) == Some(stack)
    };
    for n in c.ops.list_networks().await? {
        if ours(&n.labels) {
            if let Err(e) = c.ops.remove_network(&n.id).await {
                errors.push(format!("network {}: {e}", n.name));
            }
        }
    }
    if volumes {
        for v in c.ops.list_volumes().await? {
            if ours(&v.labels) {
                if let Err(e) = c.ops.remove_volume(&v.name, false).await {
                    errors.push(format!("volume {}: {e}", v.name));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Upstream {
            status: 500,
            message: errors.join("; "),
        })
    }
}
