//! `/ee/teams` (teams spec, "API"). Admins only. Without a license,
//! changes that add access are refused; removals always work.

use super::{
    db::{self, TeamBinding, TeamRecord},
    validate_description, validate_name,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_api::{
    auth::extract::AdminUser,
    authz::require_feature,
    extract::{ApiJson, ApiPath},
    AccessEntry, ApiResult, AppState, SetAccess, Wanted,
};
use fleetops_core::access::{normalize_scope, CustomRef, EnvRole, Grant};
use fleetops_core::{edition::Feature, now_unix, Role, User};
use fleetops_store::NewAudit;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct TeamSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub members: i64,
    pub environments: i64,
}

#[derive(Serialize, ToSchema)]
pub struct TeamDetail {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub members: Vec<User>,
    /// Members that single sign-on group sync added; they leave the team
    /// through the identity provider, not by hand.
    pub synced: Vec<SyncedMember>,
    /// The team's environment access, by environment name.
    pub access: Vec<AccessEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct SyncedMember {
    pub user_id: i64,
    /// The SSO connection whose group mapping added the member.
    pub connection: String,
}

#[derive(Deserialize, ToSchema)]
pub struct NewTeam {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Where part of a user's access comes from.
#[derive(Serialize, ToSchema)]
pub struct AccessSource {
    /// Null for a direct grant.
    pub team_id: Option<i64>,
    pub team_name: Option<String>,
    pub role: EnvRole,
    /// Set when the grant is a custom role.
    pub role_id: Option<i64>,
    pub role_name: Option<String>,
    pub namespaces: Option<Vec<String>>,
}

/// A user's access to one environment, and the grants it is made of (each
/// checked on its own).
#[derive(Serialize, ToSchema)]
pub struct EffectiveAccess {
    pub env_id: i64,
    pub env_name: String,
    pub env_kind: String,
    /// The strongest built-in role among the sources.
    pub role: EnvRole,
    /// Every namespace a source covers; null when one covers the whole environment.
    pub namespaces: Option<Vec<String>>,
    /// Direct grant first, then teams by name.
    pub sources: Vec<AccessSource>,
}

/// A team's grant on one environment, and who it reaches.
#[derive(Serialize, ToSchema)]
pub struct EnvTeamAccess {
    pub team_id: i64,
    pub team_name: String,
    pub role: EnvRole,
    pub role_id: Option<i64>,
    pub role_name: Option<String>,
    /// Null for the whole environment.
    pub namespaces: Option<Vec<String>>,
    /// Usernames, sorted.
    pub members: Vec<String>,
}

fn ns_vec(g: &Grant) -> Option<Vec<String>> {
    g.namespaces.as_ref().map(|s| s.iter().cloned().collect())
}

pub(crate) fn entry(b: &TeamBinding) -> AccessEntry {
    AccessEntry {
        env_id: b.environment_id,
        env_name: b.env_name.clone(),
        env_kind: b.env_kind.clone(),
        role: b.grant.role,
        role_id: b.grant.custom.as_ref().map(|c| c.id),
        role_name: b.grant.custom.as_ref().map(|c| c.name.clone()),
        namespaces: b
            .grant
            .namespaces
            .as_ref()
            .map(|s| s.iter().cloned().collect()),
    }
}

/// Whether `new` gives nothing `old` did not: no permission the old one
/// lacked, and namespaces within the old ones. Allowed without a license.
fn no_stronger(new: &Grant, old: Option<&Grant>) -> bool {
    let Some(old) = old else { return false };
    new.perms.is_subset(&old.perms)
        && match (&new.namespaces, &old.namespaces) {
            (_, None) => true,
            (Some(n), Some(o)) => n.is_subset(o),
            (None, Some(_)) => false,
        }
}

async fn audit(
    s: &AppState,
    admin: i64,
    action: &str,
    team: &TeamRecord,
    env_id: Option<i64>,
    mut detail: serde_json::Value,
) {
    detail["team"] = json!(team.name);
    let id = team.id.to_string();
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(admin),
        environment_id: env_id,
        action,
        target_type: Some("team"),
        target_id: Some(&id),
        result: "success",
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}

/// Ends the live subscriptions of everyone in the team: their access changed.
async fn signal_members(s: &AppState, team_id: i64) -> ApiResult<()> {
    for u in db::members(s.store.pool(), team_id).await? {
        s.hubs.access_changed(u.id);
    }
    Ok(())
}

async fn detail(s: &AppState, team: TeamRecord) -> ApiResult<Json<TeamDetail>> {
    let p = s.store.pool();
    Ok(Json(TeamDetail {
        members: db::members(p, team.id).await?,
        synced: db::synced_members(p, team.id)
            .await?
            .into_iter()
            .map(|(user_id, connection)| SyncedMember {
                user_id,
                connection,
            })
            .collect(),
        access: db::bindings(p, team.id).await?.iter().map(entry).collect(),
        id: team.id,
        name: team.name,
        description: team.description,
    }))
}

#[utoipa::path(get, path = "/ee/teams", operation_id = "listTeams", tag = "enterprise",
    summary = "List teams",
    responses((status = 200, description = "Teams, by name", body = [TeamSummary])))]
pub async fn list(State(s): State<AppState>, _a: AdminUser) -> ApiResult<Json<Vec<TeamSummary>>> {
    Ok(Json(
        db::list(s.store.pool())
            .await?
            .into_iter()
            .map(|(t, members, environments)| TeamSummary {
                id: t.id,
                name: t.name,
                description: t.description,
                members,
                environments,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/ee/teams", operation_id = "createTeam", tag = "enterprise",
    summary = "Create a team (needs the teams feature)", request_body = NewTeam,
    responses((status = 201, description = "The team", body = TeamDetail)))]
pub async fn create(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiJson(body): ApiJson<NewTeam>,
) -> ApiResult<(StatusCode, Json<TeamDetail>)> {
    require_feature(&s, Feature::Teams)?;
    let name = validate_name(&body.name)?;
    let description = validate_description(body.description.as_deref())?;
    let team = db::create(s.store.pool(), &name, description.as_deref(), now_unix()).await?;
    audit(&s, admin.user.id, "team.create", &team, None, json!({})).await;
    Ok((StatusCode::CREATED, detail(&s, team).await?))
}

#[utoipa::path(get, path = "/ee/teams/{id}", operation_id = "getTeam", tag = "enterprise",
    summary = "A team with its members and access", params(("id" = i64, Path, description = "Team id")),
    responses((status = 200, description = "The team", body = TeamDetail)))]
pub async fn get(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<TeamDetail>> {
    let team = db::get(s.store.pool(), id).await?;
    detail(&s, team).await
}

#[utoipa::path(patch, path = "/ee/teams/{id}", operation_id = "updateTeam", tag = "enterprise",
    summary = "Rename a team or change its description (needs the teams feature)",
    params(("id" = i64, Path, description = "Team id")), request_body = NewTeam,
    responses((status = 200, description = "The team", body = TeamDetail)))]
pub async fn update(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
    ApiJson(body): ApiJson<NewTeam>,
) -> ApiResult<Json<TeamDetail>> {
    require_feature(&s, Feature::Teams)?;
    let name = validate_name(&body.name)?;
    let description = validate_description(body.description.as_deref())?;
    let old = db::get(s.store.pool(), id).await?;
    let team = db::update(
        s.store.pool(),
        id,
        &name,
        description.as_deref(),
        now_unix(),
    )
    .await?;
    audit(&s, admin.user.id, "team.update", &team, None,
        json!({"from": {"name": old.name, "description": old.description}, "description": team.description}),
    )
    .await;
    detail(&s, team).await
}

#[utoipa::path(delete, path = "/ee/teams/{id}", operation_id = "deleteTeam", tag = "enterprise",
    summary = "Delete a team; its members lose the access it gave",
    params(("id" = i64, Path, description = "Team id")),
    responses((status = 204, description = "Deleted")))]
pub async fn delete(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<StatusCode> {
    let p = s.store.pool();
    let team = db::get(p, id).await?;
    // Read before the delete cascades them away.
    let members = db::members(p, id).await?;
    db::delete(p, id).await?;
    audit(
        &s,
        admin.user.id,
        "team.delete",
        &team,
        None,
        json!({"members": members.iter().map(|u| &u.username).collect::<Vec<_>>()}),
    )
    .await;
    for u in &members {
        s.hubs.access_changed(u.id);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/ee/teams/{id}/members/{user_id}", operation_id = "addTeamMember",
    tag = "enterprise", summary = "Add a user to a team (needs the teams feature)",
    params(("id" = i64, Path, description = "Team id"), ("user_id" = i64, Path, description = "User id")),
    responses((status = 204, description = "A member (already, or now)")))]
pub async fn add_member(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, user_id)): ApiPath<(i64, i64)>,
) -> ApiResult<StatusCode> {
    require_feature(&s, Feature::Teams)?;
    let team = db::get(s.store.pool(), id).await?;
    let user = s.store.get_user(user_id).await?;
    if db::add_member(s.store.pool(), id, user_id, now_unix()).await? {
        audit(
            &s,
            admin.user.id,
            "team.member.add",
            &team,
            None,
            json!({"user_id": user.id, "username": user.username}),
        )
        .await;
        s.hubs.access_changed(user_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/ee/teams/{id}/members/{user_id}", operation_id = "removeTeamMember",
    tag = "enterprise", summary = "Remove a user from a team",
    params(("id" = i64, Path, description = "Team id"), ("user_id" = i64, Path, description = "User id")),
    responses((status = 204, description = "Not a member (any more)")))]
pub async fn remove_member(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, user_id)): ApiPath<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let team = db::get(s.store.pool(), id).await?;
    let user = s.store.get_user(user_id).await?;
    if db::synced_members(s.store.pool(), id)
        .await?
        .iter()
        .any(|(u, _)| *u == user_id)
    {
        return Err(fleetops_core::Error::Conflict(
            "managed by single sign-on: remove the group mapping or the user's group instead"
                .into(),
        )
        .into());
    }
    if db::remove_member(s.store.pool(), id, user_id).await? {
        audit(
            &s,
            admin.user.id,
            "team.member.remove",
            &team,
            None,
            json!({"user_id": user.id, "username": user.username}),
        )
        .await;
        s.hubs.access_changed(user_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(put, path = "/ee/teams/{id}/access/{env_id}", operation_id = "setTeamAccess",
    tag = "enterprise",
    summary = "Grant or change a team's access to an environment (raising it needs the teams feature)",
    params(("id" = i64, Path, description = "Team id"), ("env_id" = i64, Path, description = "Environment id")),
    request_body = SetAccess, responses((status = 200, description = "The grant", body = AccessEntry)))]
pub async fn set_access(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, env_id)): ApiPath<(i64, i64)>,
    ApiJson(body): ApiJson<SetAccess>,
) -> ApiResult<Json<AccessEntry>> {
    let p = s.store.pool();
    let team = db::get(p, id).await?;
    let env = s.store.get_environment(env_id).await?;
    let wanted = body.wanted()?;
    let ns = body
        .namespaces
        .map(|v| normalize_scope(&env.kind, v))
        .transpose()?;
    let new = match wanted {
        Wanted::Builtin(role) => Grant::builtin(role, ns),
        Wanted::Custom(role_id) => {
            let r = crate::roles::db::get(p, role_id).await?;
            Grant::custom(
                CustomRef {
                    id: r.id,
                    name: r.name,
                },
                r.perms,
                ns,
            )
        }
    };
    let old = db::binding(p, id, env_id).await?;
    if !no_stronger(&new, old.as_ref().map(|b| &b.grant)) {
        require_feature(&s, Feature::Teams)?;
        if new.custom.is_some() {
            require_feature(&s, Feature::Rbac)?;
        }
    }
    let b = match &new.custom {
        None => {
            db::set_binding(p, id, env_id, new.role, new.namespaces.as_ref(), now_unix()).await?
        }
        Some(c) => {
            db::set_custom_binding(p, id, env_id, c.id, new.namespaces.as_ref(), now_unix()).await?
        }
    };
    let e = entry(&b);
    audit(
        &s,
        admin.user.id,
        "team.access.grant",
        &team,
        Some(env_id),
        json!({"env": b.env_name, "role": e.role, "role_id": e.role_id,
            "role_name": e.role_name, "namespaces": e.namespaces}),
    )
    .await;
    signal_members(&s, id).await?;
    Ok(Json(e))
}

#[utoipa::path(delete, path = "/ee/teams/{id}/access/{env_id}", operation_id = "revokeTeamAccess",
    tag = "enterprise", summary = "Revoke a team's access to an environment",
    params(("id" = i64, Path, description = "Team id"), ("env_id" = i64, Path, description = "Environment id")),
    responses((status = 204, description = "Revoked (or there was nothing to revoke)")))]
pub async fn revoke_access(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    ApiPath((id, env_id)): ApiPath<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let p = s.store.pool();
    let team = db::get(p, id).await?;
    let env = s.store.get_environment(env_id).await?;
    if db::delete_binding(p, id, env_id).await? {
        audit(
            &s,
            admin.user.id,
            "team.access.revoke",
            &team,
            Some(env_id),
            json!({"env": env.name}),
        )
        .await;
        signal_members(&s, id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/ee/users/{id}/access", operation_id = "listUserEffectiveAccess",
    tag = "enterprise", summary = "A user's access from direct grants and teams, with its sources",
    params(("id" = i64, Path, description = "User id")),
    responses((status = 200, description = "By environment name; empty for admins, who have full access", body = [EffectiveAccess])))]
pub async fn user_access(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(id): ApiPath<i64>,
) -> ApiResult<Json<Vec<EffectiveAccess>>> {
    if s.store.get_user(id).await?.role == Role::Admin {
        return Ok(Json(vec![]));
    }
    // (env id, name, kind, grant, source), direct grants first.
    let mut parts: Vec<(i64, String, String, Grant, AccessSource)> = Vec::new();
    let source = |team: Option<&TeamRecord>, g: &Grant| AccessSource {
        team_id: team.map(|t| t.id),
        team_name: team.map(|t| t.name.clone()),
        role: g.role,
        role_id: g.custom.as_ref().map(|c| c.id),
        role_name: g.custom.as_ref().map(|c| c.name.clone()),
        namespaces: ns_vec(g),
    };
    for b in s.store.user_bindings(id).await? {
        let src = source(None, &b.grant);
        parts.push((b.environment_id, b.env_name, b.env_kind, b.grant, src));
    }
    for (env_id, g) in crate::roles::db::user_direct(s.store.pool(), id).await? {
        let env = s.store.get_environment(env_id).await?;
        let src = source(None, &g);
        parts.push((env_id, env.name, env.kind, g, src));
    }
    for (team, b) in db::user_team_bindings(s.store.pool(), id).await? {
        let src = source(Some(&team), &b.grant);
        parts.push((b.environment_id, b.env_name, b.env_kind, b.grant, src));
    }
    let mut out: Vec<EffectiveAccess> = Vec::new();
    let mut grants: Vec<Vec<Grant>> = Vec::new();
    for (env_id, env_name, env_kind, grant, src) in parts {
        match out.iter().position(|e| e.env_id == env_id) {
            Some(i) => {
                out[i].sources.push(src);
                grants[i].push(grant);
            }
            None => {
                out.push(EffectiveAccess {
                    env_id,
                    env_name,
                    env_kind,
                    role: grant.role,
                    namespaces: None,
                    sources: vec![src],
                });
                grants.push(vec![grant]);
            }
        }
    }
    for (e, g) in out.iter_mut().zip(grants) {
        e.role = g.iter().map(|x| x.role).max().unwrap_or(EnvRole::Viewer);
        e.namespaces = if g.iter().any(|x| x.namespaces.is_none()) {
            None
        } else {
            let all: std::collections::BTreeSet<&String> = g
                .iter()
                .flat_map(|x| x.namespaces.iter().flatten())
                .collect();
            Some(all.into_iter().cloned().collect())
        };
    }
    out.sort_by(|a, b| a.env_name.cmp(&b.env_name));
    Ok(Json(out))
}

#[utoipa::path(get, path = "/ee/envs/{env}/teams", operation_id = "listEnvTeamAccess",
    tag = "enterprise", summary = "Teams with access to an environment, and their members",
    params(("env" = String, Path, description = "Environment name")),
    responses((status = 200, description = "By team name", body = [EnvTeamAccess])))]
pub async fn env_teams(
    State(s): State<AppState>,
    _a: AdminUser,
    ApiPath(env): ApiPath<String>,
) -> ApiResult<Json<Vec<EnvTeamAccess>>> {
    let env = s
        .store
        .list_environments()
        .await?
        .into_iter()
        .find(|e| e.name == env)
        .ok_or_else(|| fleetops_core::Error::NotFound(format!("environment '{env}'")))?;
    let p = s.store.pool();
    let mut out = Vec::new();
    for (team, b) in db::env_teams(p, env.id).await? {
        out.push(EnvTeamAccess {
            members: db::members(p, team.id)
                .await?
                .into_iter()
                .map(|u| u.username)
                .collect(),
            team_id: team.id,
            team_name: team.name,
            role: b.grant.role,
            role_id: b.grant.custom.as_ref().map(|c| c.id),
            role_name: b.grant.custom.as_ref().map(|c| c.name.clone()),
            namespaces: ns_vec(&b.grant),
        });
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::access::EnvRole::{Operator, Viewer};

    fn g(role: fleetops_core::access::EnvRole, ns: Option<&[&str]>) -> Grant {
        Grant::builtin(role, ns.map(|n| n.iter().map(|s| s.to_string()).collect()))
    }

    #[test]
    fn no_stronger_cases() {
        let ab: &[&str] = &["a", "b"];
        let a: &[&str] = &["a"];
        assert!(
            no_stronger(&g(Viewer, Some(a)), Some(&g(Viewer, Some(a)))),
            "equal"
        );
        assert!(
            no_stronger(&g(Viewer, None), Some(&g(Operator, None))),
            "lower role"
        );
        assert!(
            !no_stronger(&g(Operator, None), Some(&g(Viewer, None))),
            "higher role"
        );
        assert!(
            no_stronger(&g(Viewer, Some(a)), Some(&g(Viewer, None))),
            "list under whole"
        );
        assert!(
            !no_stronger(&g(Viewer, None), Some(&g(Viewer, Some(a)))),
            "whole over list"
        );
        assert!(
            !no_stronger(&g(Viewer, Some(ab)), Some(&g(Viewer, Some(a)))),
            "wider list"
        );
        assert!(!no_stronger(&g(Viewer, Some(a)), None), "no old grant");
    }

    #[test]
    fn no_stronger_compares_permissions() {
        use fleetops_core::access::EnvRole::{Operator, Viewer};
        let custom = |perms: &[&str]| {
            Grant::custom(
                CustomRef {
                    id: 1,
                    name: "r".into(),
                },
                perms.iter().map(|p| p.parse().unwrap()).collect(),
                None,
            )
        };
        let view = custom(&["pods.view"]);
        assert!(
            no_stronger(&view, Some(&g(Viewer, None))),
            "a subset of Viewer"
        );
        let reveal = custom(&["secrets.reveal"]);
        assert!(
            !no_stronger(&reveal, Some(&g(Operator, None))),
            "Operator lacks secrets.reveal"
        );
        assert!(
            !no_stronger(&g(Viewer, None), Some(&custom(&["pods.view"]))),
            "Viewer is more than pods.view"
        );
    }
}
