use crate::{auth::extract::CurrentUser, extract::ApiPath, tasks::TaskInfo, ApiResult, AppState};
use axum::{extract::State, Json};
use fleetops_core::{Error, Role};

/// A task's current state. Other users' tasks look missing to non-admins.
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<TaskInfo>> {
    let task = s
        .tasks
        .get(&id)
        .filter(|t| t.owner == cu.user.id || cu.user.role == Role::Admin)
        .ok_or_else(|| Error::NotFound(format!("task '{id}'")))?;
    Ok(Json(task.info()))
}
