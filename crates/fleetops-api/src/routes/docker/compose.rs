use super::ctx;
use crate::openapi::{ComposeActionName, EnvParam};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    compose::{group_projects, project_members, ComposeProject},
    extract::ApiPath,
    ApiResult, AppState,
};
use axum::{extract::State, Json};
use fleetops_core::docker::ContainerAction;
use fleetops_core::perm::{Row, Verb};
use fleetops_core::Error;
use serde::Serialize;
use utoipa::ToSchema;

#[utoipa::path(
    get,
    path = "/envs/{env}/compose-projects",
    operation_id = "listComposeProjects",
    tag = "compose",
    summary = "Compose projects on the environment",
    params(EnvParam),
    responses((status = 200, description = "Projects", body = Vec<ComposeProject>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ComposeProject>>> {
    let c = ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    Ok(Json(group_projects(&c.ops.list_containers().await?)))
}

/// start | stop | restart | down (down removes the project's containers).
/// Every member is attempted; per-container failures are reported, not fatal.
#[utoipa::path(
    post,
    path = "/envs/{env}/compose-projects/{project}/{action}",
    operation_id = "composeAction",
    tag = "compose",
    summary = "Start, stop, restart or remove (`down`) a project's containers",
    params(EnvParam, ("project" = String, Path, description = "Project name"), ("action" = ComposeActionName, Path)),
    responses((status = 200, description = "Every member was attempted; failures are listed", body = ComposeOutcome))
)]
pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, project, action)): ApiPath<(String, String, String)>,
) -> ApiResult<Json<ComposeOutcome>> {
    let op = match action.as_str() {
        "start" => Some(ContainerAction::Start),
        "stop" => Some(ContainerAction::Stop),
        "restart" => Some(ContainerAction::Restart),
        "down" => None,
        other => return Err(Error::NotFound(format!("unknown compose action '{other}'")).into()),
    };
    let need = match op {
        Some(_) => Need::of(Row::Stacks, Verb::Operate),
        None => Need::of(Row::Stacks, Verb::Delete),
    };
    let c = ctx(&s, cu, &env, need).await?;
    let all = c.ops.list_containers().await?;
    let members = project_members(&all, &project);
    if members.is_empty() {
        return Err(Error::NotFound(format!("compose project '{project}'")).into());
    }
    let mut errors = Vec::new();
    for m in &members {
        let res = match op {
            Some(a) => c.ops.container_action(&m.id, a).await,
            None => c.ops.remove_container(&m.id, true, false).await,
        };
        if let Err(e) = res {
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
    c.audit(
        &s,
        &format!("compose.{action}"),
        "compose-project",
        &project,
        &outcome,
    )
    .await;
    Ok(affected(members.len() - errors.len(), errors))
}

#[derive(Serialize, ToSchema)]
pub struct ComposeOutcome {
    /// Containers the action succeeded on.
    pub affected: usize,
    /// `name: error` for each container it failed on.
    pub errors: Vec<String>,
}

fn affected(affected: usize, errors: Vec<String>) -> Json<ComposeOutcome> {
    Json(ComposeOutcome { affected, errors })
}
