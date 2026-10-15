use crate::{auth::extract::CurrentUser, AppState};
use axum::{extract::State, Json};
use fleetops_core::edition::EditionInfo;

#[utoipa::path(
    get,
    path = "/edition",
    operation_id = "getEdition",
    tag = "system",
    summary = "This server's edition and the Enterprise features in effect",
    responses((status = 200, description = "Edition", body = EditionInfo))
)]
pub async fn get(State(s): State<AppState>, _user: CurrentUser) -> Json<EditionInfo> {
    Json(EditionInfo::of(s.entitlements.as_ref()))
}
