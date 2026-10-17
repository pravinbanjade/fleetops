//! `/envs/{env}/k8s/discovery` and `/envs/{env}/k8s/r/…`: kinds FleetOps
//! has no curated page for, found by discovery (spec 3g §2).

use crate::routes::kube::{
    audit_with, check_name, check_namespace, check_object_name, check_size, kube_for, mode,
    KubeCtx, ReplaceBody,
};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::kube::{
    ApiKind, ApplyResult, Discovery, EditableObject, KindRef, KubeEvent, KubeList, KubeObject,
    KubeOps,
};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{Error, Result};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::IntoParams;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ResourcePath {
    /// Environment name.
    pub env: String,
    /// API group; `core` for the core group.
    pub group: String,
    pub version: String,
    /// Plural lowercase resource, e.g. `certificates`.
    pub resource: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct GenericObjectPath {
    pub env: String,
    pub group: String,
    pub version: String,
    pub resource: String,
    /// Namespace; `-` for cluster-scoped kinds.
    pub ns: String,
    pub name: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DiscoveryQuery {
    /// `1` skips the 60-second cache.
    pub refresh: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GenericListQuery {
    /// Omit for all namespaces.
    pub namespace: Option<String>,
}

/// The resource a URL names, checked before it can reach an apiserver URL.
pub(crate) fn kind_ref(group: &str, version: &str, resource: &str) -> Result<KindRef> {
    let ok_version = !version.is_empty()
        && version.len() <= 63
        && version
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if (group != "core" && check_name(group).is_err())
        || !ok_version
        || check_name(resource).is_err()
    {
        return Err(Error::Invalid(format!(
            "'{group}/{version}/{resource}' is not a Kubernetes resource"
        )));
    }
    Ok(KindRef {
        group: KindRef::group_from_url(group),
        version: version.into(),
        resource: resource.into(),
    })
}

/// The discovered kind `r`, refreshing once on a miss: a CRD installed a
/// moment ago is found without waiting for the cache.
pub(crate) async fn resolve(ops: &Arc<dyn KubeOps>, r: &KindRef) -> Result<ApiKind> {
    let find = |d: Discovery| d.kinds.into_iter().find(|k| &k.api == r);
    if let Some(k) = find(ops.discover(false).await?) {
        return Ok(k);
    }
    find(ops.discover(true).await?)
        .ok_or_else(|| Error::NotFound(format!("the cluster does not serve {}", r.path())))
}

fn target(k: &ApiKind, ns: &str, name: &str) -> String {
    format!("{}/{ns}/{name}", k.api.path())
}

impl GenericObjectPath {
    fn checked(&self) -> Result<KindRef> {
        check_namespace(&self.ns)?;
        check_object_name(&self.name)?;
        kind_ref(&self.group, &self.version, &self.resource)
    }
}

/// The kind and the grant's verdict on one object, audited when it refuses.
async fn object(
    s: &AppState,
    cu: &CurrentUser,
    p: &GenericObjectPath,
    verb: Verb,
) -> Result<(KubeCtx, ApiKind)> {
    let r = p.checked()?;
    let perm = Permission::new(Row::K8sOther, verb);
    let k8 = kube_for(s, cu, &p.env, Need::Perm(perm)).await?;
    let k = resolve(&k8.ops, &r).await?;
    crate::authz::deny_audited(
        s,
        cu,
        k8.env.id(),
        &perm.to_string(),
        k8.grants.check_object_in(perm, k.namespaced, &p.ns),
    )
    .await?;
    Ok((k8, k))
}

fn verb(k: &ApiKind, v: &str, what: &str) -> Result<()> {
    if k.can(v) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("{} cannot be {what}", k.kind)))
    }
}

#[utoipa::path(get, path = "/envs/{env}/k8s/discovery", operation_id = "kubeDiscovery", tag = "kubernetes",
    summary = "The kinds the cluster serves that have no curated page", params(crate::openapi::EnvParam, DiscoveryQuery),
    responses((status = 200, description = "Generic kinds, and API groups that could not be read", body = Discovery)))]
pub async fn discovery(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    Query(q): Query<DiscoveryQuery>,
) -> ApiResult<Json<Discovery>> {
    let k8 = kube_for(&s, &cu, &env, Need::AnyGrant).await?;
    let mut d = k8.ops.discover(q.refresh.as_deref() == Some("1")).await?;
    let view = Permission::new(Row::K8sOther, Verb::View);
    if k8.grants.namespaces_for(view).is_some() {
        d.kinds.retain(|k| k.namespaced);
    }
    Ok(Json(d))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}", operation_id = "listGenericKube", tag = "kubernetes",
    summary = "List objects of a discovered kind", params(ResourcePath, GenericListQuery),
    responses((status = 200, description = "Rows", body = KubeList)))]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ResourcePath>,
    Query(q): Query<GenericListQuery>,
) -> ApiResult<Json<KubeList>> {
    let r = kind_ref(&p.group, &p.version, &p.resource)?;
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    if let Some(ns) = ns {
        check_namespace(ns)?;
    }
    let view = Permission::new(Row::K8sOther, Verb::View);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(view)).await?;
    let k = resolve(&k8.ops, &r).await?;
    let ns = ns.filter(|_| k.namespaced);
    crate::authz::deny_audited(
        &s,
        &cu,
        k8.env.id(),
        &view.to_string(),
        k8.grants.check_list_in(view, k.namespaced, ns),
    )
    .await?;
    let rows = k8.ops.generic_list(&k, ns).await?;
    Ok(Json(k8.grants.filter_list_in(k.namespaced, rows)))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}/{ns}/{name}", operation_id = "getGenericKube", tag = "kubernetes",
    summary = "One object of a discovered kind", params(GenericObjectPath),
    responses((status = 200, description = "The object", body = KubeObject)))]
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<GenericObjectPath>,
) -> ApiResult<Json<KubeObject>> {
    let (k8, k) = object(&s, &cu, &p, Verb::View).await?;
    Ok(Json(k8.ops.generic_get(&k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}/{ns}/{name}/events", operation_id = "genericKubeEvents", tag = "kubernetes",
    summary = "A discovered object's events, newest first", params(GenericObjectPath),
    responses((status = 200, description = "Events", body = [KubeEvent])))]
pub async fn events(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<GenericObjectPath>,
) -> ApiResult<Json<Vec<KubeEvent>>> {
    let (k8, k) = object(&s, &cu, &p, Verb::View).await?;
    Ok(Json(k8.ops.generic_events(&k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}/{ns}/{name}/edit", operation_id = "editableGenericKube", tag = "kubernetes",
    summary = "A discovered object as YAML for editing", params(GenericObjectPath),
    responses((status = 200, description = "Cleaned YAML", body = EditableObject)))]
pub async fn editable(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<GenericObjectPath>,
) -> ApiResult<Json<EditableObject>> {
    let (k8, k) = object(&s, &cu, &p, Verb::View).await?;
    Ok(Json(k8.ops.generic_editable(&k, &p.ns, &p.name).await?))
}

#[utoipa::path(put, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}/{ns}/{name}", operation_id = "replaceGenericKube", tag = "kubernetes",
    summary = "Replace a discovered object with edited YAML; 409 when it changed since it was opened",
    params(GenericObjectPath), request_body = ReplaceBody,
    responses((status = 200, description = "What changed (or would, for a dry run)", body = ApplyResult)))]
pub async fn replace(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<GenericObjectPath>,
    ApiJson(body): ApiJson<ReplaceBody>,
) -> ApiResult<Json<ApplyResult>> {
    check_size(&body.yaml)?;
    // An empty version would make the PUT unconditional: a silent overwrite.
    if body.resource_version.trim().is_empty() {
        return Err(Error::Invalid(
            "resource_version is required: open the object to edit it".into(),
        )
        .into());
    }
    let (k8, k) = object(&s, &cu, &p, Verb::Operate).await?;
    verb(&k, "update", "edited")?;
    let r = k8
        .ops
        .generic_replace(
            &k,
            &p.ns,
            &p.name,
            &body.yaml,
            &body.resource_version,
            mode(body.dry_run, false),
        )
        .await;
    if !body.dry_run {
        let err = r.as_ref().err().map(|e| e.to_string());
        audit_with(
            &s,
            &cu,
            k8.env.id(),
            "k8s.edit",
            &target(&k, &p.ns, &p.name),
            err.as_deref(),
        )
        .await;
    }
    Ok(Json(r?))
}

#[utoipa::path(delete, path = "/envs/{env}/k8s/r/{group}/{version}/{resource}/{ns}/{name}", operation_id = "deleteGenericKube", tag = "kubernetes",
    summary = "Delete a discovered object (background propagation)", params(GenericObjectPath),
    responses((status = 204, description = "Deleted")))]
pub async fn delete(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<GenericObjectPath>,
) -> ApiResult<StatusCode> {
    let (k8, k) = object(&s, &cu, &p, Verb::Delete).await?;
    verb(&k, "delete", "deleted")?;
    let r = k8.ops.generic_delete(&k, &p.ns, &p.name).await;
    let err = r.as_ref().err().map(|e| e.to_string());
    audit_with(
        &s,
        &cu,
        k8.env.id(),
        "k8s.delete",
        &target(&k, &p.ns, &p.name),
        err.as_deref(),
    )
    .await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}
