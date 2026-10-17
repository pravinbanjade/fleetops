//! `/ee/roles`: custom roles, admins only (custom roles spec).

use super::db::{self, RoleRecord};
use super::validate_name;
use crate::teams::validate_description;
use axum::{extract::State, http::StatusCode, Json};
use fleetops_api::{
    auth::extract::AdminUser,
    authz::require_feature,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use fleetops_core::edition::Feature;
use fleetops_core::now_unix;
use fleetops_core::perm::{PermSet, Row, Verb};
use fleetops_store::NewAudit;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct RoleSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// `row.verb`, sorted.
    pub permissions: Vec<String>,
    /// How many users and teams hold it, on any environment.
    pub users: i64,
    pub teams: i64,
}

#[derive(Serialize, ToSchema)]
pub struct RoleHolder {
    pub id: i64,
    /// Username or team name.
    pub name: String,
    /// The environment it is held on.
    pub env: String,
}

#[derive(Serialize, ToSchema)]
pub struct RoleHolders {
    pub users: Vec<RoleHolder>,
    pub teams: Vec<RoleHolder>,
}

#[derive(Serialize, ToSchema)]
pub struct RoleDetail {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub permissions: Vec<String>,
    pub holders: RoleHolders,
}

#[derive(Deserialize, ToSchema)]
pub struct NewRole {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub permissions: Vec<String>,
}

/// Any of the fields; `description: null` clears it.
#[derive(Deserialize, ToSchema)]
pub struct RoleChange {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
}

/// A field that was sent, even as `null`, is `Some`.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Serialize, ToSchema)]
pub struct CatalogRow {
    pub key: &'static str,
    pub label: &'static str,
    /// `docker` or `kubernetes`.
    pub group: &'static str,
    /// The verbs this row has a cell for, in column order.
    pub cells: Vec<&'static str>,
}

#[derive(Serialize, ToSchema)]
pub struct CatalogVerb {
    pub key: &'static str,
    pub label: &'static str,
}

/// The role grid, so the editor isn't written down twice.
#[derive(Serialize, ToSchema)]
pub struct Catalog {
    pub rows: Vec<CatalogRow>,
    pub verbs: Vec<CatalogVerb>,
}

async fn audit(
    s: &AppState,
    admin: i64,
    action: &str,
    role: &RoleRecord,
    mut detail: serde_json::Value,
) {
    detail["role"] = json!(role.name);
    let id = role.id.to_string();
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(admin),
        environment_id: None,
        action,
        target_type: Some("role"),
        target_id: Some(&id),
        result: "success",
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}

async fn detail(s: &AppState, r: RoleRecord) -> ApiResult<Json<RoleDetail>> {
    let (users, teams) = db::holder_rows(s.store.pool(), r.id).await?;
    let holder = |(id, name, env): (i64, String, String)| RoleHolder { id, name, env };
    Ok(Json(RoleDetail {
        holders: RoleHolders {
            users: users.into_iter().map(holder).collect(),
            teams: teams.into_iter().map(holder).collect(),
        },
        permissions: r.perms.to_strs(),
        id: r.id,
        name: r.name,
        description: r.description,
    }))
}

#[utoipa::path(get, path = "/ee/roles", operation_id = "listRoles", tag = "enterprise",
    summary = "List custom roles",
    responses((status = 200, description = "Roles, by name", body = [RoleSummary])))]
pub async fn list(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<Vec<RoleSummary>>> {
    Ok(Json(
        db::list(s.store.pool())
            .await?
            .into_iter()
            .map(|(r, users, teams)| RoleSummary {
                permissions: r.perms.to_strs(),
                id: r.id,
                name: r.name,
                description: r.description,
                users,
                teams,
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/ee/roles/catalog", operation_id = "roleCatalog", tag = "enterprise",
    summary = "The permission grid a role is built from",
    responses((status = 200, description = "Rows and verbs", body = Catalog)))]
pub async fn catalog(_a: AdminUser) -> Json<Catalog> {
    Json(Catalog {
        rows: Row::ALL
            .into_iter()
            .map(|r| CatalogRow {
                key: r.as_str(),
                label: r.label(),
                group: r.group(),
                cells: r.verbs().iter().map(|v| v.as_str()).collect(),
            })
            .collect(),
        verbs: Verb::ALL
            .into_iter()
            .map(|v| CatalogVerb {
                key: v.as_str(),
                label: v.label(),
            })
            .collect(),
    })
}

#[utoipa::path(post, path = "/ee/roles", operation_id = "createRole", tag = "enterprise",
    summary = "Create a custom role (needs the rbac feature)", request_body = NewRole,
    responses((status = 201, description = "The role", body = RoleDetail)))]
pub async fn create(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiJson(body): ApiJson<NewRole>,
) -> ApiResult<(StatusCode, Json<RoleDetail>)> {
    require_feature(&s, Feature::Rbac)?;
    let name = validate_name(&body.name)?;
    let description = validate_description(body.description.as_deref())?;
    let perms = PermSet::from_strs(&body.permissions)?;
    let r = db::create(
        s.store.pool(),
        &name,
        description.as_deref(),
        perms,
        now_unix(),
    )
    .await?;
    audit(
        &s,
        admin.user.id,
        "role.create",
        &r,
        json!({"permissions": perms.to_strs()}),
    )
    .await;
    Ok((StatusCode::CREATED, detail(&s, r).await?))
}

#[utoipa::path(get, path = "/ee/roles/{id}", operation_id = "getRole", tag = "enterprise",
    summary = "A custom role with who holds it", params(("id" = i64, Path, description = "Role id")),
    responses((status = 200, description = "The role", body = RoleDetail)))]
pub async fn get(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<RoleDetail>> {
    let r = db::get(s.store.pool(), id).await?;
    detail(&s, r).await
}

#[utoipa::path(patch, path = "/ee/roles/{id}", operation_id = "updateRole", tag = "enterprise",
    summary = "Rename a role or change its permissions (adding any needs the rbac feature)",
    params(("id" = i64, Path, description = "Role id")), request_body = RoleChange,
    responses((status = 200, description = "The role", body = RoleDetail)))]
pub async fn update(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<RoleChange>,
) -> ApiResult<Json<RoleDetail>> {
    let p = s.store.pool();
    let old = db::get(p, id).await?;
    let name = body.name.as_deref().map(validate_name).transpose()?;
    let description = body
        .description
        .as_ref()
        .map(|d| validate_description(d.as_deref()))
        .transpose()?;
    let perms = body
        .permissions
        .as_deref()
        .map(PermSet::from_strs)
        .transpose()?;
    // Without a license an admin may only take permissions away.
    let renames = name.as_ref().is_some_and(|n| *n != old.name) || description.is_some();
    let adds = perms.is_some_and(|p| !p.is_subset(&old.perms));
    if renames || adds {
        require_feature(&s, Feature::Rbac)?;
    }
    let holders = db::holders(p, id).await?;
    let r = db::update(
        p,
        id,
        name.as_deref(),
        description.as_ref().map(|d| d.as_deref()),
        perms,
        now_unix(),
    )
    .await?;
    let diff = |a: &PermSet, b: &PermSet| -> Vec<String> {
        a.iter()
            .filter(|x| !b.contains(*x))
            .map(|x| x.to_string())
            .collect()
    };
    let (added, removed) = (diff(&r.perms, &old.perms), diff(&old.perms, &r.perms));
    audit(
        &s,
        admin.user.id,
        "role.update",
        &r,
        json!({"added": added, "removed": removed}),
    )
    .await;
    if r.perms != old.perms {
        for u in holders {
            s.hubs.access_changed(u);
        }
    }
    detail(&s, r).await
}

#[utoipa::path(delete, path = "/ee/roles/{id}", operation_id = "deleteRole", tag = "enterprise",
    summary = "Delete a role and every grant of it", params(("id" = i64, Path, description = "Role id")),
    responses((status = 204, description = "Deleted")))]
pub async fn delete(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    let p = s.store.pool();
    let r = db::get(p, id).await?;
    // Read before the grants go, so every former holder is told.
    let holders = db::holders(p, id).await?;
    let removed = db::delete(p, id).await?;
    audit(
        &s,
        admin.user.id,
        "role.delete",
        &r,
        json!({"grants_removed": removed}),
    )
    .await;
    for u in holders {
        s.hubs.access_changed(u);
    }
    Ok(StatusCode::NO_CONTENT)
}
