//! Who may use which environment (spec §4). Admins only.

use crate::{
    auth::extract::AdminUser,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::access::{normalize_namespaces, EnvRole, Grant};
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
    /// The built-in role, or a custom role's closest built-in.
    pub role: EnvRole,
    /// Set when the grant is a custom role (Enterprise).
    pub role_id: Option<i64>,
    pub role_name: Option<String>,
    /// Null for the whole environment.
    pub namespaces: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct EnvAccessEntry {
    pub user_id: i64,
    pub username: String,
    pub role: EnvRole,
    pub role_id: Option<i64>,
    pub role_name: Option<String>,
    pub namespaces: Option<Vec<String>>,
}

/// A built-in `role`, or a custom role by `role_id` (Enterprise): exactly one.
#[derive(Deserialize, ToSchema)]
pub struct SetAccess {
    #[serde(default)]
    pub role: Option<EnvRole>,
    #[serde(default)]
    pub role_id: Option<i64>,
    /// Kubernetes only; omit for the whole environment.
    #[serde(default)]
    pub namespaces: Option<Vec<String>>,
}

/// What a `SetAccess` asks for.
pub enum Wanted {
    Builtin(EnvRole),
    Custom(i64),
}

impl SetAccess {
    pub fn wanted(&self) -> fleetops_core::Result<Wanted> {
        match (self.role, self.role_id) {
            (Some(r), None) => Ok(Wanted::Builtin(r)),
            (None, Some(id)) => Ok(Wanted::Custom(id)),
            _ => Err(Error::Invalid("give a role or a role_id".into())),
        }
    }
}

/// The custom role a grant names, as (id, name).
pub fn custom_of(g: &Grant) -> (Option<i64>, Option<String>) {
    g.custom
        .as_ref()
        .map_or((None, None), |c| (Some(c.id), Some(c.name.clone())))
}

fn ns_of(g: &Grant) -> Option<Vec<String>> {
    g.namespaces.as_ref().map(|s| s.iter().cloned().collect())
}

fn ns_list(b: &BindingRecord) -> Option<Vec<String>> {
    ns_of(&b.grant)
}

impl From<&BindingRecord> for AccessEntry {
    fn from(b: &BindingRecord) -> Self {
        Self {
            env_id: b.environment_id,
            env_name: b.env_name.clone(),
            env_kind: b.env_kind.clone(),
            role: b.grant.role,
            role_id: None,
            role_name: None,
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
            role_id: None,
            role_name: None,
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
    let mut out: Vec<AccessEntry> = s
        .store
        .user_bindings(id)
        .await?
        .iter()
        .map(AccessEntry::from)
        .collect();
    for (env_id, g) in s.custom_roles.user_rows(id).await? {
        let env = s.store.get_environment(env_id).await?;
        let (role_id, role_name) = custom_of(&g);
        out.push(AccessEntry {
            env_id,
            env_name: env.name,
            env_kind: env.kind,
            role: g.role,
            role_id,
            role_name,
            namespaces: ns_of(&g),
        });
    }
    out.sort_by(|a, b| a.env_name.cmp(&b.env_name));
    Ok(Json(out))
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
    let wanted = body.wanted()?;
    let ns = body.namespaces.map(normalize_namespaces).transpose()?;
    let entry = match wanted {
        Wanted::Builtin(role) => {
            let b = s
                .store
                .set_binding(id, env_id, role, ns.as_ref(), now_unix())
                .await?;
            s.custom_roles.clear(id, env_id).await?;
            AccessEntry::from(&b)
        }
        Wanted::Custom(role_id) => {
            let env = s.store.get_environment(env_id).await?;
            if ns.is_some() && !env.kind.starts_with("kube-") {
                return Err(Error::Invalid(
                    "only a Kubernetes environment takes namespaces".into(),
                )
                .into());
            }
            let g = s
                .custom_roles
                .assign(id, env_id, role_id, ns.as_ref(), now_unix())
                .await?;
            s.store.delete_binding(id, env_id).await?;
            let (role_id, role_name) = custom_of(&g);
            AccessEntry {
                env_id,
                env_name: env.name,
                env_kind: env.kind,
                role: g.role,
                role_id,
                role_name,
                namespaces: ns_of(&g),
            }
        }
    };
    audit(
        &s,
        admin.user.id,
        "access.grant",
        id,
        env_id,
        json!({"env": entry.env_name, "role": entry.role, "role_id": entry.role_id,
            "role_name": entry.role_name, "namespaces": entry.namespaces}),
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
    let direct = s.store.delete_binding(id, env_id).await?;
    let custom = s.custom_roles.clear(id, env_id).await?;
    if direct || custom {
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
    let mut out: Vec<EnvAccessEntry> = s
        .store
        .env_bindings(env.id())
        .await?
        .iter()
        .map(EnvAccessEntry::from)
        .collect();
    for (user_id, g) in s.custom_roles.env_rows(env.id()).await? {
        let user = s.store.get_user(user_id).await?;
        let (role_id, role_name) = custom_of(&g);
        out.push(EnvAccessEntry {
            user_id,
            username: user.username,
            role: g.role,
            role_id,
            role_name,
            namespaces: ns_of(&g),
        });
    }
    out.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(Json(out))
}
