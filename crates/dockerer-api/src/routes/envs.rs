use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    extract::{ApiJson, ApiPath},
    routes::audit,
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use dockerer_core::env::{validate_env_name, validate_env_url, EnvKind, EnvStatus};
use dockerer_core::now_unix;
use dockerer_store::EnvironmentRecord;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateEnv {
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
}

#[derive(Deserialize)]
pub struct UpdateEnv {
    pub name: Option<String>,
    pub url: Option<String>,
}

pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
) -> ApiResult<Json<Vec<EnvironmentRecord>>> {
    authorize(&cu.user, None, Action::EnvRead)?;
    Ok(Json(s.store.list_environments().await?))
}

pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiJson(body): ApiJson<CreateEnv>,
) -> ApiResult<(StatusCode, Json<EnvironmentRecord>)> {
    authorize(&cu.user, None, Action::EnvManage)?;
    validate_env_name(&body.name)?;
    validate_env_url(&body.url)?;
    let rec = s
        .store
        .create_environment(&body.name, body.kind.as_str(), &body.url, now_unix())
        .await?;
    s.envs.insert(&rec)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.create",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok((StatusCode::CREATED, Json(rec)))
}

async fn find(s: &AppState, name: &str) -> ApiResult<EnvironmentRecord> {
    let env = s.envs.get(name)?;
    Ok(s.store.get_environment(env.id()).await?)
}

pub async fn update(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<UpdateEnv>,
) -> ApiResult<Json<EnvironmentRecord>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let current = find(&s, &name).await?;
    let new_name = body.name.unwrap_or(current.name.clone());
    let new_url = body.url.unwrap_or(current.url.clone());
    validate_env_name(&new_name)?;
    validate_env_url(&new_url)?;
    let rec = s
        .store
        .update_environment(current.id, &new_name, &new_url, now_unix())
        .await?;
    s.envs.remove(&current.name);
    s.envs.insert(&rec)?;
    audit(
        &s,
        Some(cu.user.id),
        "env.update",
        "environment",
        &rec.name,
        "success",
    )
    .await;
    Ok(Json(rec))
}

pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<StatusCode> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let current = find(&s, &name).await?;
    s.store.delete_environment(current.id).await?;
    s.envs.remove(&current.name);
    audit(
        &s,
        Some(cu.user.id),
        "env.delete",
        "environment",
        &current.name,
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn status(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<EnvStatus>> {
    let env = s.envs.get(&name)?;
    authorize(&cu.user, Some(env.id()), Action::EnvRead)?;
    Ok(Json(env.status().await))
}
