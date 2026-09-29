use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    compose::{group_projects, project_members, ComposeProject},
    extract::ApiPath,
    ApiResult, AppState,
};
use axum::{extract::State, Json};
use fleetops_core::docker::ContainerAction;
use fleetops_core::Error;
use serde_json::{json, Value};

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<ComposeProject>>> {
    let c = ctx(&s, cu, &env, Action::DockerRead).await?;
    Ok(Json(group_projects(&c.ops.list_containers().await?)))
}

/// start | stop | restart | down (down removes the project's containers).
/// Every member is attempted; per-container failures are reported, not fatal.
pub async fn action(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, project, action)): ApiPath<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let op = match action.as_str() {
        "start" => Some(ContainerAction::Start),
        "stop" => Some(ContainerAction::Stop),
        "restart" => Some(ContainerAction::Restart),
        "down" => None,
        other => return Err(Error::NotFound(format!("unknown compose action '{other}'")).into()),
    };
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
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

fn affected(n: usize, errors: Vec<String>) -> Json<Value> {
    Json(json!({ "affected": n, "errors": errors }))
}
