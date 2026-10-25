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
        (status = 409, description = "`confirm_required`: FleetOps runs on this node")))]
pub async fn update_node(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, id)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NodeUpdate>,
) -> ApiResult<StatusCode> {
    let c = swarm_ctx(&s, cu, &env, Permission::new(Row::SwarmNodes, Verb::Operate)).await?;
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
        (status = 409, description = "The node is up or a manager, or `confirm_required`")))]
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
