//! Who may use which environment (spec §4). Admins only.

use crate::{
    auth::extract::AdminUser,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::access::{normalize_namespaces, EnvRole};
use fleetops_core::{now_unix, Error, Role};
use fleetops_store::{BindingRecord, NewAudit};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct AccessEntry {
    pub env_id: i64,
    pub env_name: String,
    pub env_kind: String,
    pub role: EnvRole,
    /// Null for the whole environment.
    pub namespaces: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct EnvAccessEntry {
    pub user_id: i64,
    pub username: String,
    pub role: EnvRole,
    pub namespaces: Option<Vec<String>>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetAccess {
    pub role: EnvRole,
    /// Kubernetes only; omit for the whole environment.
    #[serde(default)]
    pub namespaces: Option<Vec<String>>,
}

fn ns_list(b: &BindingRecord) -> Option<Vec<String>> {
    b.grant
        .namespaces
        .as_ref()
        .map(|s| s.iter().cloned().collect())
}

impl From<&BindingRecord> for AccessEntry {
    fn from(b: &BindingRecord) -> Self {
        Self {
            env_id: b.environment_id,
            env_name: b.env_name.clone(),
            env_kind: b.env_kind.clone(),
            role: b.grant.role,
            namespaces: ns_list(b),
        }
    }
}

impl From<&BindingRecord> for EnvAccessEntry {
    fn from(b: &BindingRecord) -> Self {
        Self {
            user_id: b.user_id,
            username: b.username.clone(),
            role: b.grant.role,
            namespaces: ns_list(b),
        }
    }
}

async fn audit(
    s: &AppState,
    admin: i64,
    action: &str,
    user_id: i64,
    env_id: i64,
    detail: serde_json::Value,
) {
    let id = user_id.to_string();
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(admin),
        environment_id: Some(env_id),
        action,
        target_type: Some("user"),
        target_id: Some(&id),
        result: "success",
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}

#[utoipa::path(get, path = "/users/{id}/access", operation_id = "listUserAccess", tag = "users",
    summary = "A user's environment grants", params(("id" = i64, Path, description = "User id")),
    responses((status = 200, description = "Grants, by environment", body = [AccessEntry])))]
pub async fn list_user(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Vec<AccessEntry>>> {
    s.store.get_user(id).await?;
    Ok(Json(
        s.store
            .user_bindings(id)
            .await?
            .iter()
            .map(AccessEntry::from)
            .collect(),
    ))
}

#[utoipa::path(put, path = "/users/{id}/access/{env_id}", operation_id = "setUserAccess", tag = "users",
    summary = "Grant or change a user's access to an environment",
    params(("id" = i64, Path, description = "User id"), ("env_id" = i64, Path, description = "Environment id")),
    request_body = SetAccess, responses((status = 200, description = "The grant", body = AccessEntry)))]
pub async fn set(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, env_id)): ApiPath<(i64, i64)>,
    ApiJson(body): ApiJson<SetAccess>,
) -> ApiResult<Json<AccessEntry>> {
    if s.store.get_user(id).await?.role == Role::Admin {
        return Err(Error::Invalid("admins already have full access".into()).into());
    }
    let ns = body.namespaces.map(normalize_namespaces).transpose()?;
    let b = s
        .store
        .set_binding(id, env_id, body.role, ns.as_ref(), now_unix())
        .await?;
    let entry = AccessEntry::from(&b);
    audit(
        &s,
        admin.user.id,
        "access.grant",
        id,
        env_id,
        json!({"env": b.env_name, "role": body.role, "namespaces": entry.namespaces}),
    )
    .await;
    s.hubs.access_changed(id);
    Ok(Json(entry))
}

#[utoipa::path(delete, path = "/users/{id}/access/{env_id}", operation_id = "revokeUserAccess", tag = "users",
    summary = "Revoke a user's access to an environment",
    params(("id" = i64, Path, description = "User id"), ("env_id" = i64, Path, description = "Environment id")),
    responses((status = 204, description = "Revoked (or there was nothing to revoke)")))]
pub async fn revoke(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, env_id)): ApiPath<(i64, i64)>,
) -> ApiResult<StatusCode> {
    s.store.get_user(id).await?;
    let env = s.store.get_environment(env_id).await?;
    if s.store.delete_binding(id, env_id).await? {
        audit(
            &s,
            admin.user.id,
            "access.revoke",
            id,
            env_id,
            json!({"env": env.name}),
        )
        .await;
        s.hubs.access_changed(id);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/envs/{env}/access", operation_id = "listEnvAccess", tag = "environments",
    summary = "Who has access to an environment", params(crate::openapi::EnvParam),
    responses((status = 200, description = "Grants, by user", body = [EnvAccessEntry])))]
pub async fn list_env(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<EnvAccessEntry>>> {
    let env = s.envs.get(&env)?;
    Ok(Json(
        s.store
            .env_bindings(env.id())
            .await?
            .iter()
            .map(EnvAccessEntry::from)
            .collect(),
    ))
}
