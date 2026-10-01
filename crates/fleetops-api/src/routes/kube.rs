//! `/envs/{env}/k8s/…`: Kubernetes objects in one environment (spec §5).

use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::env::{EnvKind, Environment};
use fleetops_core::kube::{KubeEvent, KubeKind, KubeList, KubeObject, KubeOps};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

pub(crate) fn wrong_kind(name: &str, kind: EnvKind) -> Error {
    Error::Conflict(format!(
        "wrong environment kind: '{name}' is {}",
        kind.as_str()
    ))
}

/// Resolves the environment, authorises, and returns its Kubernetes ops.
pub(crate) fn kube_for(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    action: Action,
) -> Result<(Arc<dyn Environment>, Arc<dyn KubeOps>)> {
    let env = s.envs.get(name)?;
    authorize(&cu.user, Some(env.id()), action)?;
    if !env.kind().is_kube() {
        return Err(wrong_kind(name, env.kind()));
    }
    let ops = env
        .kube()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok((env, ops))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct KindPath {
    /// Environment name.
    pub env: String,
    /// Plural lowercase kind, e.g. `deployments`.
    pub kind: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ObjectPath {
    pub env: String,
    pub kind: String,
    /// Namespace; `-` for cluster-scoped kinds.
    pub ns: String,
    pub name: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Omit for all namespaces.
    pub namespace: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ScaleBody {
    pub replicas: u32,
}

fn kind(s: &str) -> Result<KubeKind> {
    s.parse()
}

async fn audit(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    action: &str,
    target: &str,
    outcome: &Result<()>,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action,
        target_type: Some("k8s"),
        target_id: Some(target),
        result: if outcome.is_ok() {
            "success"
        } else {
            "failure"
        },
        detail: outcome
            .as_ref()
            .err()
            .map(|e| serde_json::json!({ "error": e.to_string() })),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}", operation_id = "listKube", tag = "kubernetes",
    summary = "List objects of one kind", params(KindPath, ListQuery),
    responses((status = 200, description = "Rows", body = KubeList)))]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<KindPath>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<KubeList>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(
        ops.list(k, q.namespace.as_deref().filter(|n| !n.is_empty()))
            .await?,
    ))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "getKube", tag = "kubernetes",
    summary = "One object: summary, masked YAML, conditions, owners", params(ObjectPath),
    responses((status = 200, description = "The object", body = KubeObject)))]
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<Json<KubeObject>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.get(k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/events", operation_id = "kubeEvents", tag = "kubernetes",
    summary = "An object's events, newest first", params(ObjectPath),
    responses((status = 200, description = "Events", body = [KubeEvent])))]
pub async fn events(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<Json<Vec<KubeEvent>>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.events(k, &p.ns, &p.name).await?))
}

fn target(k: KubeKind, p: &ObjectPath) -> String {
    format!("{}/{}/{}", k.as_str(), p.ns, p.name)
}

#[utoipa::path(delete, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "deleteKube", tag = "kubernetes",
    summary = "Delete an object (background propagation)", params(ObjectPath),
    responses((status = 204, description = "Deleted")))]
pub async fn delete(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.delete(k, &p.ns, &p.name).await;
    audit(&s, &cu, env.id(), "k8s.delete", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/scale", operation_id = "scaleKube", tag = "kubernetes",
    summary = "Set a Deployment's or StatefulSet's replicas", params(ObjectPath), request_body = ScaleBody,
    responses((status = 204, description = "Scaled")))]
pub async fn scale(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
    ApiJson(body): ApiJson<ScaleBody>,
) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    if !k.scalable() {
        return Err(Error::Invalid(format!("{} cannot be scaled", k.as_str())).into());
    }
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.scale(k, &p.ns, &p.name, body.replicas).await;
    audit(&s, &cu, env.id(), "k8s.scale", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/restart", operation_id = "restartKube", tag = "kubernetes",
    summary = "Rollout restart a Deployment, StatefulSet or DaemonSet", params(ObjectPath),
    responses((status = 204, description = "Restarted")))]
pub async fn restart(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    if !k.restartable() {
        return Err(Error::Invalid(format!("{} cannot be restarted", k.as_str())).into());
    }
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.restart(k, &p.ns, &p.name).await;
    audit(&s, &cu, env.id(), "k8s.restart", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}
