//! Changing a swarm (swarm spec §4): nodes, services, secrets, configs and
//! networks. Every change is audited (§8); one that can stop FleetOps or
//! disconnect the swarm first needs the object's name typed (§3).

use super::swarm::{swarm_ctx, EnvIdPath, SwarmCtx};
use super::swarm_protect;
use crate::{
    auth::extract::CurrentUser,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::swarm::{Availability, NodeChange, NodeRole};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::{IntoParams, ToSchema};

impl SwarmCtx {
    /// Writes an audit row for a swarm change, with `extra` merged into
    /// the detail. Audit failures are logged, never surfaced.
    pub async fn audit<T>(
        &self,
        s: &AppState,
        action: &str,
        target_type: &str,
        target_id: &str,
        outcome: &Result<T>,
        extra: Value,
    ) {
        let (result, mut detail) = match outcome {
            Ok(_) => ("success", extra),
            Err(e) => {
                let mut d = extra;
                if !d.is_object() {
                    d = json!({});
                }
                d["error"] = json!(e.to_string());
                ("failure", d)
            }
        };
        if detail.as_object().is_some_and(|o| o.is_empty()) {
            detail = Value::Null;
        }
        let entry = NewAudit {
            ts: now_unix(),
            user_id: Some(self.user.user.id),
            environment_id: Some(self.env.id()),
            action,
            target_type: Some(target_type),
            target_id: Some(target_id),
            result,
            detail: (!detail.is_null()).then_some(detail),
        };
        if let Err(e) = s.store.insert_audit(&entry).await {
            tracing::error!(error = %e, action, "failed to write audit log");
        }
    }
}

/// `{"confirmed": true}` when a protection was confirmed, merged into `d`.
fn with_confirmed(mut d: Value, confirmed: bool) -> Value {
    if confirmed {
        d["confirmed"] = json!(true);
    }
    d
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ConfirmQuery {
    /// The object's name, when the server asked for it (`confirm_required`).
    pub confirm: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct NodeUpdate {
    #[serde(flatten)]
    pub change: NodeChange,
    /// The node's hostname, when the server asked for it.
    pub confirm: Option<String>,
}

#[utoipa::path(patch, path = "/envs/{env}/swarm/nodes/{id}", operation_id = "updateSwarmNode", tag = "swarm",
    summary = "Set a node's availability, role or labels", params(EnvIdPath), request_body = NodeUpdate,
    responses((status = 204, description = "Changed"),
        (status = 409, description = "`confirm_required`: FleetOps runs on this node", body = crate::error::ErrorBody)))]
pub async fn update_node(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NodeUpdate>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmNodes, Verb::Operate),
    )
    .await?;
    let node = c
        .ops
        .node(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such node '{id}'")))?;
    let reason = if body.change.availability == Some(Availability::Drain) {
        swarm_protect::node_guard(&*c.ops, &id, &node.hostname).await?
    } else {
        None
    };
    let confirmed = swarm_protect::require(reason, &node.hostname, body.confirm.as_deref())?;
    let res = c.ops.update_node(&id, &body.change).await;
    let detail = with_confirmed(
        json!({ "hostname": node.hostname, "change": body.change }),
        confirmed,
    );
    c.audit(&s, "swarm.node.update", "swarm-node", &id, &res, detail)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/envs/{env}/swarm/nodes/{id}", operation_id = "removeSwarmNode", tag = "swarm",
    summary = "Remove a worker that is down", params(EnvIdPath, ConfirmQuery),
    responses((status = 204, description = "Removed"),
        (status = 409, description = "The node is up or a manager, or `confirm_required`", body = crate::error::ErrorBody)))]
pub async fn remove_node(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ConfirmQuery>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, Permission::new(Row::SwarmNodes, Verb::Delete)).await?;
    let node = c
        .ops
        .node(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such node '{id}'")))?;
    let host = &node.hostname;
    if node.role == NodeRole::Manager {
        return Err(Error::Conflict(format!("demote node '{host}' before removing it")).into());
    }
    if node.state == "ready" {
        return Err(Error::Conflict(format!(
            "node '{host}' is up; run 'docker swarm leave' on it, or wait until it is down, before removing it"
        ))
        .into());
    }
    let reason = swarm_protect::node_guard(&*c.ops, &id, host).await?;
    let confirmed = swarm_protect::require(reason, host, q.confirm.as_deref())?;
    let res = c.ops.remove_node(&id).await;
    let detail = with_confirmed(json!({ "hostname": host }), confirmed);
    c.audit(&s, "swarm.node.remove", "swarm-node", &id, &res, detail)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
pub struct NewService {
    /// Docker's ServiceSpec.
    #[schema(value_type = Object)]
    pub spec: Value,
}

#[derive(serde::Serialize, ToSchema)]
pub struct Created {
    pub id: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ServiceEdit {
    /// Docker's ServiceSpec, whole.
    #[schema(value_type = Object)]
    pub spec: Value,
    /// The `version` the spec was read at.
    pub version: u64,
    /// The service's name, when the server asked for it.
    pub confirm: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct Scale {
    pub replicas: u64,
    pub confirm: Option<String>,
}

#[derive(Deserialize, Default, ToSchema)]
pub struct Confirm {
    /// The object's name, when the server asked for it.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// The service, and whether changing it needs `confirm` (passes or fails).
async fn guarded_service(
    c: &SwarmCtx,
    id: &str,
    confirm: Option<&str>,
) -> Result<(fleetops_core::swarm::ServiceDetail, bool)> {
    let svc = c
        .ops
        .service(id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such service '{id}'")))?;
    let reason = swarm_protect::service_guard(&svc.summary);
    let confirmed = swarm_protect::require(reason, &svc.summary.name, confirm)?;
    Ok((svc, confirmed))
}

fn operate() -> Permission {
    Permission::new(Row::SwarmServices, Verb::Operate)
}

#[utoipa::path(post, path = "/envs/{env}/swarm/services", operation_id = "createSwarmService", tag = "swarm",
    summary = "Create a service from a ServiceSpec", params(crate::openapi::EnvParam), request_body = NewService,
    responses((status = 201, description = "Created", body = Created)))]
pub async fn create_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<NewService>,
) -> ApiResult<(StatusCode, axum::Json<Created>)> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmServices, Verb::Create),
    )
    .await?;
    let name = body.spec["Name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Invalid("a service spec needs a Name".into()))?
        .to_string();
    let res = c.ops.create_service(&body.spec).await;
    c.audit(
        &s,
        "swarm.service.create",
        "swarm-service",
        &name,
        &res,
        json!({}),
    )
    .await;
    Ok((StatusCode::CREATED, axum::Json(Created { id: res? })))
}

#[utoipa::path(put, path = "/envs/{env}/swarm/services/{id}", operation_id = "editSwarmService", tag = "swarm",
    summary = "Replace a service's spec", params(EnvIdPath), request_body = ServiceEdit,
    responses((status = 204, description = "Updated"),
        (status = 409, description = "Changed since `version`, or `confirm_required`", body = crate::error::ErrorBody)))]
pub async fn edit_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(mut body): ApiJson<ServiceEdit>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, operate()).await?;
    let (svc, confirmed) = guarded_service(&c, &id, body.confirm.as_deref()).await?;
    // A spec read without Reveal has masked env values: keep the real ones.
    let reveal = Permission::new(Row::Containers, Verb::Reveal);
    if !c.grants.allows(reveal, None) {
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let path = "/TaskTemplate/ContainerSpec/Env";
        let live = strings(svc.spec.pointer(path));
        if let Some(vars) = body.spec.pointer_mut(path) {
            let mut env = strings(Some(vars));
            fleetops_core::docker::unmask_env(&mut env, &live);
            *vars = json!(env);
        }
    }
    let change = fleetops_core::swarm::ServiceChange::Spec {
        spec: body.spec,
        version: body.version,
    };
    let res = c.ops.update_service(&id, change).await;
    let detail = with_confirmed(json!({ "name": svc.summary.name }), confirmed);
    c.audit(
        &s,
        "swarm.service.update",
        "swarm-service",
        &id,
        &res,
        detail,
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/swarm/services/{id}/scale", operation_id = "scaleSwarmService", tag = "swarm",
    summary = "Set a replicated service's replicas", params(EnvIdPath), request_body = Scale,
    responses((status = 204, description = "Scaled"), (status = 409, description = "`confirm_required`", body = crate::error::ErrorBody)))]
pub async fn scale_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<Scale>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, operate()).await?;
    let (svc, confirmed) = guarded_service(&c, &id, body.confirm.as_deref()).await?;
    let res = c
        .ops
        .update_service(
            &id,
            fleetops_core::swarm::ServiceChange::Scale(body.replicas),
        )
        .await;
    let detail = with_confirmed(
        json!({ "name": svc.summary.name, "replicas": body.replicas }),
        confirmed,
    );
    c.audit(
        &s,
        "swarm.service.scale",
        "swarm-service",
        &id,
        &res,
        detail,
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

/// Redeploy or rollback: no parameters but the confirmation.
async fn simple_change(
    s: AppState,
    cu: CurrentUser,
    env: String,
    id: String,
    confirm: Option<String>,
    change: fleetops_core::swarm::ServiceChange,
    action: &str,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, operate()).await?;
    let (svc, confirmed) = guarded_service(&c, &id, confirm.as_deref()).await?;
    let res = c.ops.update_service(&id, change).await;
    let detail = with_confirmed(json!({ "name": svc.summary.name }), confirmed);
    c.audit(&s, action, "swarm-service", &id, &res, detail)
        .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/swarm/services/{id}/redeploy", operation_id = "redeploySwarmService", tag = "swarm",
    summary = "Restart every task with the same spec (force update)", params(EnvIdPath), request_body = Confirm,
    responses((status = 204, description = "Redeploying"), (status = 409, description = "`confirm_required`", body = crate::error::ErrorBody)))]
pub async fn redeploy_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<Confirm>,
) -> ApiResult<StatusCode> {
    use fleetops_core::swarm::ServiceChange::Redeploy;
    simple_change(
        s,
        cu,
        env,
        id,
        body.confirm,
        Redeploy,
        "swarm.service.redeploy",
    )
    .await
}

#[utoipa::path(post, path = "/envs/{env}/swarm/services/{id}/rollback", operation_id = "rollbackSwarmService", tag = "swarm",
    summary = "Go back to the service's previous spec", params(EnvIdPath), request_body = Confirm,
    responses((status = 204, description = "Rolling back"), (status = 409, description = "`confirm_required`", body = crate::error::ErrorBody)))]
pub async fn rollback_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<Confirm>,
) -> ApiResult<StatusCode> {
    use fleetops_core::swarm::ServiceChange::Rollback;
    simple_change(
        s,
        cu,
        env,
        id,
        body.confirm,
        Rollback,
        "swarm.service.rollback",
    )
    .await
}

#[utoipa::path(delete, path = "/envs/{env}/swarm/services/{id}", operation_id = "removeSwarmService", tag = "swarm",
    summary = "Remove a service", params(EnvIdPath, ConfirmQuery),
    responses((status = 204, description = "Removed"), (status = 409, description = "`confirm_required`", body = crate::error::ErrorBody)))]
pub async fn remove_service(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    Query(q): Query<ConfirmQuery>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmServices, Verb::Delete),
    )
    .await?;
    let (svc, confirmed) = guarded_service(&c, &id, q.confirm.as_deref()).await?;
    let res = c.ops.remove_service(&id).await;
    let detail = with_confirmed(json!({ "name": svc.summary.name }), confirmed);
    c.audit(
        &s,
        "swarm.service.remove",
        "swarm-service",
        &id,
        &res,
        detail,
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

/// Docker's limit on a secret or config.
const MAX_VALUE: usize = 500 * 1024;

#[derive(Deserialize, ToSchema)]
pub struct NewSwarmObject {
    pub name: String,
    /// Write-only for secrets: neither Docker nor FleetOps shows it again.
    pub value: String,
}

fn checked_value<'a>(o: &'a NewSwarmObject, what: &str) -> Result<&'a [u8]> {
    if o.name.trim().is_empty() {
        return Err(Error::Invalid(format!("a {what} needs a name")));
    }
    if o.value.len() > MAX_VALUE {
        return Err(Error::Invalid(format!("a {what} value is at most 500 KiB")));
    }
    Ok(o.value.as_bytes())
}

#[utoipa::path(post, path = "/envs/{env}/swarm/secrets", operation_id = "createSwarmSecret", tag = "swarm",
    summary = "Create a secret (its value is write-only)", params(crate::openapi::EnvParam), request_body = NewSwarmObject,
    responses((status = 201, description = "Created", body = Created)))]
pub async fn create_secret(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<NewSwarmObject>,
) -> ApiResult<(StatusCode, axum::Json<Created>)> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmSecrets, Verb::Create),
    )
    .await?;
    let data = checked_value(&body, "secret")?;
    let res = c.ops.create_secret(&body.name, data).await;
    c.audit(
        &s,
        "swarm.secret.create",
        "swarm-secret",
        &body.name,
        &res,
        json!({}),
    )
    .await;
    Ok((StatusCode::CREATED, axum::Json(Created { id: res? })))
}

#[utoipa::path(delete, path = "/envs/{env}/swarm/secrets/{id}", operation_id = "removeSwarmSecret", tag = "swarm",
    summary = "Remove a secret no service uses", params(EnvIdPath),
    responses((status = 204, description = "Removed")))]
pub async fn remove_secret(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmSecrets, Verb::Delete),
    )
    .await?;
    let res = c.ops.remove_secret(&id).await;
    c.audit(
        &s,
        "swarm.secret.remove",
        "swarm-secret",
        &id,
        &res,
        json!({}),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/swarm/configs", operation_id = "createSwarmConfig", tag = "swarm",
    summary = "Create a config", params(crate::openapi::EnvParam), request_body = NewSwarmObject,
    responses((status = 201, description = "Created", body = Created)))]
pub async fn create_config(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<NewSwarmObject>,
) -> ApiResult<(StatusCode, axum::Json<Created>)> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmConfigs, Verb::Create),
    )
    .await?;
    let data = checked_value(&body, "config")?;
    let res = c.ops.create_config(&body.name, data).await;
    c.audit(
        &s,
        "swarm.config.create",
        "swarm-config",
        &body.name,
        &res,
        json!({}),
    )
    .await;
    Ok((StatusCode::CREATED, axum::Json(Created { id: res? })))
}

#[derive(serde::Serialize, ToSchema)]
pub struct ConfigData {
    /// Null when the content is not UTF-8.
    pub text: Option<String>,
    pub size: u64,
}

#[utoipa::path(get, path = "/envs/{env}/swarm/configs/{id}/data", operation_id = "getSwarmConfigData", tag = "swarm",
    summary = "A config's content (needs Reveal)", params(EnvIdPath),
    responses((status = 200, description = "The content", body = ConfigData)))]
pub async fn config_data(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<axum::Json<ConfigData>> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmConfigs, Verb::Reveal),
    )
    .await?;
    let data = c
        .ops
        .config_data(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no such config '{id}'")))?;
    c.audit(
        &s,
        "swarm.config.reveal",
        "swarm-config",
        &id,
        &Ok(()),
        json!({}),
    )
    .await;
    Ok(axum::Json(ConfigData {
        size: data.len() as u64,
        text: String::from_utf8(data).ok(),
    }))
}

#[utoipa::path(delete, path = "/envs/{env}/swarm/configs/{id}", operation_id = "removeSwarmConfig", tag = "swarm",
    summary = "Remove a config no service uses", params(EnvIdPath),
    responses((status = 204, description = "Removed")))]
pub async fn remove_config(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(
        &s,
        cu,
        &env,
        Permission::new(Row::SwarmConfigs, Verb::Delete),
    )
    .await?;
    let res = c.ops.remove_config(&id).await;
    c.audit(
        &s,
        "swarm.config.remove",
        "swarm-config",
        &id,
        &res,
        json!({}),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/swarm/networks", operation_id = "createSwarmNetwork", tag = "swarm",
    summary = "Create an overlay network", params(crate::openapi::EnvParam),
    request_body = fleetops_core::swarm::NewSwarmNetwork,
    responses((status = 201, description = "Created", body = Created)))]
pub async fn create_network(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<fleetops_core::swarm::NewSwarmNetwork>,
) -> ApiResult<(StatusCode, axum::Json<Created>)> {
    let c = swarm_ctx(&s, cu, &env, Permission::new(Row::Networks, Verb::Create)).await?;
    if body.name.trim().is_empty() {
        return Err(Error::Invalid("a network needs a name".into()).into());
    }
    let res = c.ops.create_network(&body).await;
    c.audit(
        &s,
        "swarm.network.create",
        "swarm-network",
        &body.name,
        &res,
        json!({}),
    )
    .await;
    Ok((StatusCode::CREATED, axum::Json(Created { id: res? })))
}

#[utoipa::path(delete, path = "/envs/{env}/swarm/networks/{id}", operation_id = "removeSwarmNetwork", tag = "swarm",
    summary = "Remove a swarm-scoped network", params(EnvIdPath),
    responses((status = 204, description = "Removed")))]
pub async fn remove_network(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, Permission::new(Row::Networks, Verb::Delete)).await?;
    // Only the swarm's own networks: a node's local ones go through its view.
    if !c
        .ops
        .networks()
        .await?
        .iter()
        .any(|n| n.id == id || n.name == id)
    {
        return Err(Error::NotFound(format!("no such swarm network '{id}'")).into());
    }
    let res = c.ops.remove_network(&id).await;
    c.audit(
        &s,
        "swarm.network.remove",
        "swarm-network",
        &id,
        &res,
        json!({}),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}
