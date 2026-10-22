//! Stacks: Compose projects FleetOps authors and deploys (stacks spec §5–§6).

use super::compose::ComposeOutcome;
use super::{ctx, EnvCtx};
use crate::openapi::EnvParam;
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath, ApiQuery},
    stacks::start::{make_plan, start_deploy, Actor},
    stacks::{is_old, recover_stack, stack_containers, try_lock},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::compose::docker::LABEL_PROJECT;
use fleetops_core::compose::{parse, validate_stack_name, ComposeIssue};
use fleetops_core::docker::{mask_env, ContainerAction};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{Draft, NewAudit, RevisionRecord, StackRecord};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

pub use crate::stacks::start::{DeploySource, DeployStarted, PlanResponse};

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
    /// A git stack has a fetched commit that is not deployed.
    pub update_available: bool,
    /// A git stack's auto-sync is held after a rollback.
    pub paused: bool,
    /// A git stack's last sync error.
    pub sync_error: Option<String>,
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

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployBody {
    pub from: DeploySource,
    /// The `token` of the plan that was reviewed.
    pub plan_token: String,
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

pub(super) async fn audit<T>(
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
    let git = s.store.stack_git(stack.id).await?;
    Ok(StackSummary {
        update_available: git.as_ref().is_some_and(|g| g.pending_commit.is_some()),
        paused: git.as_ref().is_some_and(|g| g.paused),
        sync_error: git.and_then(|g| g.last_sync_error),
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
    let stored = s.store.draft(stack.id).await?;
    if stack.source == "git"
        && stored.as_ref().map(|d| d.compose.as_str()) != Some(body.compose.as_str())
    {
        return Err(Error::Invalid(
            "a git stack's Compose file comes from its repository; change it there".into(),
        )
        .into());
    }
    let stored = stored.map(|d| d.variables).unwrap_or_default();
    unmask_variables(&mut body.variables, &stored)?;
    // A git stack's file is never written back: a sync may have replaced
    // it since the check above.
    let res = if stack.source == "git" {
        s.store
            .save_draft_variables(stack.id, &body.variables, Some(c.user.user.id), now_unix())
            .await
    } else {
        s.store
            .save_draft(
                stack.id,
                &body.compose,
                &body.variables,
                Some(c.user.user.id),
                now_unix(),
            )
            .await
    };
    audit(&s, &c, "stack.save", &name, &res, None).await;
    res?;
    Ok(StatusCode::NO_CONTENT)
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
    Ok(Json(make_plan(&s, c.ops.as_ref(), &stack, from).await?.0))
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
    let started = start_deploy(
        &s,
        c.ops.clone(),
        c.env.clone(),
        &stack,
        body.from,
        Some(&body.plan_token),
        Actor::User(c.user.user.id),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
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
        Ok(()) => {
            let res = s.store.delete_stack(stack.id).await;
            if res.is_ok() {
                crate::stacks::git::remove_cache(&s, stack.id);
            }
            res
        }
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
