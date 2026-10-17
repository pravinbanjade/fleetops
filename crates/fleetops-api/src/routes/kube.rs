//! `/envs/{env}/k8s/…`: Kubernetes objects in one environment (spec §5).

use crate::openapi::EnvParam;
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
use fleetops_core::access::Grants;
use fleetops_core::env::{EnvKind, Environment};
use fleetops_core::kube::{
    ApplyAction, ApplyMode, ApplyResult, EditableObject, KubeCount, KubeEvent, KubeFilter,
    KubeKind, KubeList, KubeObject, KubeOps, KubeUsage, APPLY_MAX_BYTES,
};
use fleetops_core::perm::{row_of, Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

/// An RFC 1123 DNS subdomain: what Kubernetes allows for object names.
/// Anything else is refused before it can be spliced into an apiserver URL
/// (an encoded `/` or `?` would reach a different object).
fn is_dns_subdomain(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 253
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'.')
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
}

/// An RFC 1123 DNS label: what Kubernetes allows for namespace names.
fn is_dns_label(s: &str) -> bool {
    s.len() <= 63 && !s.contains('.') && is_dns_subdomain(s)
}

pub(crate) fn check_name(name: &str) -> Result<()> {
    if is_dns_subdomain(name) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "'{name}' is not a Kubernetes object name"
        )))
    }
}

/// An object's name in a URL: a DNS subdomain, or one with `:` in it as RBAC
/// objects take (`system:controller:job-controller`). `:` is safe in a path
/// segment; `/`, `?`, `%` and the rest stay refused.
pub(crate) fn check_object_name(name: &str) -> Result<()> {
    let segments_ok = name.split(':').all(is_dns_subdomain);
    if is_dns_subdomain(name) || (name.len() <= 253 && segments_ok) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "'{name}' is not a Kubernetes object name"
        )))
    }
}

/// A namespace, or `-` for cluster-scoped kinds.
pub(crate) fn check_namespace(ns: &str) -> Result<()> {
    if ns == "-" || is_dns_label(ns) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "'{ns}' is not a Kubernetes namespace"
        )))
    }
}

pub(crate) fn wrong_kind(name: &str, kind: EnvKind) -> Error {
    Error::Conflict(format!(
        "wrong environment kind: '{name}' is {}",
        kind.as_str()
    ))
}

/// What a Kubernetes handler works with.
pub(crate) struct KubeCtx {
    pub env: Arc<dyn Environment>,
    pub ops: Arc<dyn KubeOps>,
    pub grants: Grants,
}

/// Resolves the environment, authorises, and returns its Kubernetes ops
/// with the caller's grant.
pub(crate) async fn kube_for(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    need: Need,
) -> Result<KubeCtx> {
    let (env, grants) = crate::authz::env_access(s, cu, name, need).await?;
    if !env.kind().is_kube() {
        return Err(wrong_kind(name, env.kind()));
    }
    let ops = env
        .kube()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok(KubeCtx { env, ops, grants })
}

/// The grant's verdict on one object, audited when it refuses.
async fn check_object(
    s: &AppState,
    cu: &CurrentUser,
    k8: &KubeCtx,
    kind: KubeKind,
    p: &ObjectPath,
    perm: Permission,
) -> Result<()> {
    crate::authz::deny_audited(
        s,
        cu,
        k8.env.id(),
        &perm.to_string(),
        k8.grants.check_object(perm, kind, &p.ns, &p.name),
    )
    .await
}

/// `verb` on the grid row of `kind`.
fn on(kind: KubeKind, verb: Verb) -> Permission {
    Permission::new(row_of(kind), verb)
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

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KubeListQuery {
    /// Omit for all namespaces.
    pub namespace: Option<String>,
    /// `warnings` (events only): just the Warning events, filtered by the cluster.
    pub filter: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CountsQuery {
    /// Plural kinds, comma-separated, e.g. `services,secrets`.
    pub kinds: String,
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

impl KindPath {
    fn checked(&self) -> Result<KubeKind> {
        kind(&self.kind)
    }
}

impl ObjectPath {
    /// The kind, with the namespace and name checked.
    fn checked(&self) -> Result<KubeKind> {
        let k = kind(&self.kind)?;
        check_namespace(&self.ns)?;
        check_object_name(&self.name)?;
        Ok(k)
    }
}

pub(crate) async fn audit_with(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    action: &str,
    target: &str,
    error: Option<&str>,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action,
        target_type: Some("k8s"),
        target_id: Some(target),
        result: if error.is_none() {
            "success"
        } else {
            "failure"
        },
        detail: error.map(|e| serde_json::json!({ "error": e })),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

async fn audit(
    s: &AppState,
    cu: &CurrentUser,
    env_id: i64,
    action: &str,
    target: &str,
    outcome: &Result<()>,
) {
    let err = outcome.as_ref().err().map(|e| e.to_string());
    audit_with(s, cu, env_id, action, target, err.as_deref()).await;
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}", operation_id = "listKube", tag = "kubernetes",
    summary = "List objects of one kind", params(KindPath, KubeListQuery),
    responses((status = 200, description = "Rows", body = KubeList)))]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<KindPath>,
    Query(q): Query<KubeListQuery>,
) -> ApiResult<Json<KubeList>> {
    let k = p.checked()?;
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    if let Some(ns) = ns {
        check_namespace(ns)?;
    }
    let filter = q
        .filter
        .as_deref()
        .filter(|f| !f.is_empty())
        .map(KubeFilter::parse)
        .transpose()?;
    if let Some(f) = filter {
        f.check(k)?;
    }
    let view = on(k, Verb::View);
    let k8 = kube_for(&s, &cu, &p.env, Need::AnyGrant).await?;
    crate::authz::deny_audited(
        &s,
        &cu,
        k8.env.id(),
        &view.to_string(),
        k8.grants.check_list(view, k, ns),
    )
    .await?;
    let rows = match filter {
        Some(f) => k8.ops.list_filtered(k, ns, f).await?,
        None => k8.ops.list(k, ns).await?,
    };
    Ok(Json(k8.grants.filter_list(k, rows)))
}

/// How many `kind` this grant can see, without listing bodies where the
/// client can avoid it. A namespace-limited grant sums its namespaces.
async fn count_one(k8: &KubeCtx, kind: KubeKind, ns: Option<&str>) -> Result<u64> {
    if kind == KubeKind::Nodes {
        return k8.ops.count(kind, None).await;
    }
    if kind == KubeKind::Namespaces && k8.grants.namespaces_for(on(kind, Verb::View)).is_some() {
        return Ok(k8
            .grants
            .filter_list(kind, k8.ops.list(kind, None).await?)
            .rows
            .len() as u64);
    }
    let Some(set) = k8.grants.namespaces_for(on(kind, Verb::View)) else {
        return k8.ops.count(kind, ns).await;
    };
    match kind {
        KubeKind::Namespaces => Ok(k8
            .grants
            .filter_list(kind, k8.ops.list(kind, None).await?)
            .rows
            .len() as u64),
        _ if ns.is_some() => k8.ops.count(kind, ns).await,
        _ => {
            let counts =
                futures_util::future::try_join_all(set.iter().map(|n| k8.ops.count(kind, Some(n))))
                    .await?;
            Ok(counts.into_iter().sum())
        }
    }
}

#[utoipa::path(get, path = "/envs/{env}/k8s/counts", operation_id = "kubeCounts", tag = "kubernetes",
    summary = "How many objects of each kind, without their bodies", params(EnvParam, CountsQuery),
    responses((status = 200, description = "By plural kind; a kind the cluster refuses has `error` instead of `count`", body = std::collections::BTreeMap<String, KubeCount>)))]
pub async fn counts(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env_name): ApiPath<String>,
    Query(q): Query<CountsQuery>,
) -> ApiResult<Json<std::collections::BTreeMap<String, KubeCount>>> {
    let mut kinds: Vec<KubeKind> = q
        .kinds
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(kind)
        .collect::<Result<_>>()?;
    kinds.sort();
    kinds.dedup();
    if kinds.is_empty() {
        return Err(Error::Invalid("name at least one kind to count".into()).into());
    }
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    if let Some(ns) = ns {
        check_namespace(ns)?;
    }
    let k8 = kube_for(&s, &cu, &env_name, Need::AnyGrant).await?;
    // A kind the role can't view counts as refused; a namespace or scope
    // outside the grants refuses the request, as before.
    let open = |k: KubeKind| matches!(k, KubeKind::Nodes | KubeKind::Namespaces);
    let viewable = |k: KubeKind| open(k) || k8.grants.holds(on(k, Verb::View));
    for &k in kinds.iter().filter(|&&k| viewable(k)) {
        let view = on(k, Verb::View);
        crate::authz::deny_audited(
            &s,
            &cu,
            k8.env.id(),
            &view.to_string(),
            k8.grants.check_list(view, k, ns),
        )
        .await?;
    }
    let k8 = &k8;
    let results = futures_util::future::join_all(kinds.iter().map(|&k| {
        let ok = viewable(k);
        async move {
            if ok {
                count_one(k8, k, ns).await
            } else {
                Err(fleetops_core::access::perm_denied(on(k, Verb::View)))
            }
        }
    }))
    .await;
    Ok(Json(
        kinds
            .into_iter()
            .zip(results)
            .map(|(k, r)| {
                let c = match r {
                    Ok(n) => KubeCount {
                        count: Some(n),
                        error: None,
                    },
                    Err(e) => KubeCount {
                        count: None,
                        error: Some(e.code().into()),
                    },
                };
                (k.as_str().to_string(), c)
            })
            .collect(),
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
    let k = p.checked()?;
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(on(k, Verb::View))).await?;
    check_object(&s, &cu, &k8, k, &p, on(k, Verb::View)).await?;
    let KubeCtx { ops, .. } = k8;
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
    let k = p.checked()?;
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(on(k, Verb::View))).await?;
    check_object(&s, &cu, &k8, k, &p, on(k, Verb::View)).await?;
    let KubeCtx { ops, .. } = k8;
    Ok(Json(ops.events(k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/pods", operation_id = "workloadPods", tag = "kubernetes",
    summary = "The pods a workload's selector matches", params(ObjectPath),
    responses((status = 200, description = "Rows", body = KubeList)))]
pub async fn workload_pods(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<Json<KubeList>> {
    let k = p.checked()?;
    if !k.has_pod_logs() {
        return Err(Error::Invalid(format!("{} have no pods of their own", k.as_str())).into());
    }
    let view = Permission::new(Row::Workloads, Verb::View);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(view)).await?;
    check_object(&s, &cu, &k8, k, &p, view).await?;
    let KubeCtx { ops, .. } = k8;
    Ok(Json(ops.workload_pods(k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/metrics/{kind}", operation_id = "kubeUsage", tag = "kubernetes",
    summary = "CPU and memory now, for nodes or pods, from metrics-server", params(KindPath, ListQuery),
    responses((status = 200, description = "Usage; `available` is false when the cluster has no metrics API", body = KubeUsage)))]
pub async fn usage(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<KindPath>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<KubeUsage>> {
    let k = p.checked()?;
    if !matches!(k, KubeKind::Nodes | KubeKind::Pods) {
        return Err(Error::Invalid(format!("there are no metrics for {}", k.as_str())).into());
    }
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    if let Some(ns) = ns {
        check_namespace(ns)?;
    }
    let view = on(k, Verb::View);
    let k8 = kube_for(&s, &cu, &p.env, Need::AnyGrant).await?;
    crate::authz::deny_audited(
        &s,
        &cu,
        k8.env.id(),
        &view.to_string(),
        k8.grants.check_list(view, k, ns),
    )
    .await?;
    let mut u = k8.ops.usage(k, ns).await?;
    u.rows.retain(|r| {
        r.namespace
            .as_deref()
            .is_none_or(|n| k8.grants.allows(view, Some(n)))
    });
    Ok(Json(u))
}

/// Events are the cluster's own record: FleetOps reads them, never edits.
fn writable(k: KubeKind) -> Result<()> {
    if k.read_only() {
        Err(Error::Invalid(format!("{} are read-only", k.as_str())))
    } else {
        Ok(())
    }
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
    let k = p.checked()?;
    writable(k)?;
    let perm = on(k, Verb::Delete);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
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
    let k = p.checked()?;
    if !k.scalable() {
        return Err(Error::Invalid(format!("{} cannot be scaled", k.as_str())).into());
    }
    let perm = Permission::new(Row::Workloads, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
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
    let k = p.checked()?;
    if !k.restartable() {
        return Err(Error::Invalid(format!("{} cannot be restarted", k.as_str())).into());
    }
    let perm = Permission::new(Row::Workloads, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
    let r = ops.restart(k, &p.ns, &p.name).await;
    audit(&s, &cu, env.id(), "k8s.restart", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

/// Suspends or resumes; the handlers differ only in the direction.
async fn set_suspended(
    s: AppState,
    cu: CurrentUser,
    p: ObjectPath,
    on: bool,
) -> ApiResult<StatusCode> {
    let k = p.checked()?;
    if !k.suspendable() {
        return Err(Error::Invalid(format!("{} cannot be suspended", k.as_str())).into());
    }
    let perm = Permission::new(Row::Workloads, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
    let r = ops.suspend(k, &p.ns, &p.name, on).await;
    let action = if on { "k8s.suspend" } else { "k8s.resume" };
    audit(&s, &cu, env.id(), action, &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/suspend", operation_id = "suspendKube", tag = "kubernetes",
    summary = "Suspend a Deployment or StatefulSet (scale to 0, keeping its replica count) or a CronJob (spec.suspend)",
    params(ObjectPath),
    responses((status = 204, description = "Suspended")))]
pub async fn suspend(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    set_suspended(s, cu, p, true).await
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/resume", operation_id = "resumeKube", tag = "kubernetes",
    summary = "Resume what suspend stopped: the kept replica count, or a CronJob's schedule",
    params(ObjectPath),
    responses((status = 204, description = "Resumed")))]
pub async fn resume(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    set_suspended(s, cu, p, false).await
}

async fn set_cordoned(
    s: AppState,
    cu: CurrentUser,
    p: ObjectPath,
    on: bool,
) -> ApiResult<StatusCode> {
    let k = p.checked()?;
    if k != KubeKind::Nodes {
        return Err(Error::Invalid("only nodes can be cordoned".into()).into());
    }
    let perm = Permission::new(Row::K8sOther, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
    let r = ops.cordon(&p.name, on).await;
    let action = if on { "k8s.cordon" } else { "k8s.uncordon" };
    audit(&s, &cu, env.id(), action, &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/cordon", operation_id = "cordonKube", tag = "kubernetes",
    summary = "Cordon a node: nothing new is scheduled on it; what runs there keeps running", params(ObjectPath),
    responses((status = 204, description = "Cordoned")))]
pub async fn cordon(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    set_cordoned(s, cu, p, true).await
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/uncordon", operation_id = "uncordonKube", tag = "kubernetes",
    summary = "Uncordon a node: it takes new pods again", params(ObjectPath),
    responses((status = 204, description = "Uncordoned")))]
pub async fn uncordon(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<StatusCode> {
    set_cordoned(s, cu, p, false).await
}

#[derive(Serialize, ToSchema)]
pub struct TriggerResult {
    /// The Job that was started.
    pub job: String,
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/trigger", operation_id = "triggerKube", tag = "kubernetes",
    summary = "Run a CronJob now: a Job from its template, as kubectl create job --from=cronjob does", params(ObjectPath),
    responses((status = 200, description = "The Job started", body = TriggerResult)))]
pub async fn trigger(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<Json<TriggerResult>> {
    let k = p.checked()?;
    if k != KubeKind::CronJobs {
        return Err(Error::Invalid("only cronjobs can be run now".into()).into());
    }
    let perm = Permission::new(Row::Workloads, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
    let r = ops.trigger(&p.ns, &p.name).await;
    let err = r.as_ref().err().map(|e| e.to_string());
    audit_with(
        &s,
        &cu,
        env.id(),
        "k8s.trigger",
        &target(k, &p),
        err.as_deref(),
    )
    .await;
    Ok(Json(TriggerResult { job: r? }))
}

#[derive(Deserialize, ToSchema)]
pub struct ReplaceBody {
    pub yaml: String,
    /// The `resource_version` the editor opened.
    pub resource_version: String,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ApplyBody {
    /// One or more YAML documents.
    pub yaml: String,
    /// For namespaced documents that name none.
    pub namespace: String,
    #[serde(default)]
    pub dry_run: bool,
    /// Take ownership of fields another manager owns.
    #[serde(default)]
    pub force: bool,
}

pub(crate) fn check_size(yaml: &str) -> Result<()> {
    if yaml.len() > APPLY_MAX_BYTES {
        return Err(Error::Invalid("the YAML is larger than 1 MiB".into()));
    }
    Ok(())
}

pub(crate) fn mode(dry_run: bool, force: bool) -> ApplyMode {
    if dry_run {
        ApplyMode::DryRun
    } else {
        ApplyMode::Commit { force }
    }
}

/// What an agent's access level would need to allow a forbidden write.
pub(crate) fn access_hint(level: Option<&str>) -> Option<&'static str> {
    match level {
        Some("read-only") => Some("this agent is installed read-only; re-install it at the standard level to change the common kinds, or cluster-admin for any kind"),
        Some("standard") => Some("this agent's standard level covers the common kinds; re-install it at cluster-admin to apply other kinds"),
        _ => None,
    }
}

pub(crate) async fn agent_level(s: &AppState, env: &Arc<dyn Environment>) -> Option<String> {
    if env.kind() != EnvKind::KubeAgent {
        return None;
    }
    s.store
        .get_agent(env.id())
        .await
        .ok()
        .flatten()
        .and_then(|a| a.access)
}

pub(crate) fn with_hint(message: String, hint: Option<&str>) -> String {
    match hint {
        Some(h) => format!("{message} ({h})"),
        None => message,
    }
}

/// The audit target for a document: the plural kind when FleetOps knows it.
fn doc_target(r: &ApplyResult) -> String {
    let kind = KubeKind::from_kind_name(&r.kind)
        .map(|k| k.as_str().to_string())
        .unwrap_or_else(|| r.kind.to_lowercase());
    format!(
        "{kind}/{}/{}",
        r.namespace.as_deref().unwrap_or("-"),
        r.name
    )
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/edit", operation_id = "editableKube", tag = "kubernetes",
    summary = "An object as YAML for editing, with its resource version", params(ObjectPath),
    responses((status = 200, description = "Cleaned, masked YAML", body = EditableObject)))]
pub async fn editable(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
) -> ApiResult<Json<EditableObject>> {
    let k = p.checked()?;
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(on(k, Verb::View))).await?;
    check_object(&s, &cu, &k8, k, &p, on(k, Verb::View)).await?;
    let KubeCtx { ops, .. } = k8;
    Ok(Json(ops.get_editable(k, &p.ns, &p.name).await?))
}

#[utoipa::path(put, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "replaceKube", tag = "kubernetes",
    summary = "Replace an object with edited YAML; 409 when it changed since it was opened",
    params(ObjectPath), request_body = ReplaceBody,
    responses((status = 200, description = "What changed (or would, for a dry run)", body = ApplyResult)))]
pub async fn replace(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
    ApiJson(body): ApiJson<ReplaceBody>,
) -> ApiResult<Json<ApplyResult>> {
    let k = p.checked()?;
    writable(k)?;
    check_size(&body.yaml)?;
    // An empty version would make the PUT unconditional: a silent overwrite.
    if body.resource_version.trim().is_empty() {
        return Err(Error::Invalid(
            "resource_version is required: open the object to edit it".into(),
        )
        .into());
    }
    let perm = on(k, Verb::Operate);
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(perm)).await?;
    check_object(&s, &cu, &k8, k, &p, perm).await?;
    let KubeCtx { env, ops, .. } = k8;
    let r = ops
        .replace(
            k,
            &p.ns,
            &p.name,
            &body.yaml,
            &body.resource_version,
            mode(body.dry_run, false),
        )
        .await;
    let r = match r {
        Err(Error::Forbidden(m)) => {
            let level = agent_level(&s, &env).await;
            Err(Error::Forbidden(with_hint(
                m,
                access_hint(level.as_deref()),
            )))
        }
        other => other,
    };
    if !body.dry_run {
        let err = r.as_ref().err().map(|e| e.to_string());
        audit_with(
            &s,
            &cu,
            env.id(),
            "k8s.edit",
            &target(k, &p),
            err.as_deref(),
        )
        .await;
    }
    Ok(Json(r?))
}

#[utoipa::path(post, path = "/envs/{env}/k8s/apply", operation_id = "applyKube", tag = "kubernetes",
    summary = "Server-side apply one or more YAML documents", params(EnvParam), request_body = ApplyBody,
    responses((status = 200, description = "One result per document, in order", body = [ApplyResult])))]
pub async fn apply(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env_name): ApiPath<String>,
    ApiJson(body): ApiJson<ApplyBody>,
) -> ApiResult<Json<Vec<ApplyResult>>> {
    check_size(&body.yaml)?;
    if body.namespace == "-" {
        return Err(Error::Invalid("pick a namespace for documents that name none".into()).into());
    }
    check_namespace(&body.namespace)?;
    let perm = Permission::new(Row::K8sOther, Verb::Operate);
    let KubeCtx { env, ops, grants } = kube_for(&s, &cu, &env_name, Need::Perm(perm)).await?;
    crate::authz::deny_audited(
        &s,
        &cu,
        env.id(),
        &perm.to_string(),
        grants.check_ns(perm, &body.namespace),
    )
    .await?;
    let mut results = ops
        .apply(
            &body.yaml,
            &body.namespace,
            mode(body.dry_run, body.force),
            grants.namespaces_for(perm).as_ref(),
        )
        .await?;
    let level = agent_level(&s, &env).await;
    if let Some(hint) = access_hint(level.as_deref()) {
        for e in results
            .iter_mut()
            .filter_map(|r| r.error.as_mut())
            .filter(|e| e.code == "forbidden")
        {
            e.message = with_hint(std::mem::take(&mut e.message), Some(hint));
        }
    }
    if !body.dry_run {
        for r in &results {
            let action = match (r.action, &r.error) {
                (Some(ApplyAction::Unchanged), _) => continue,
                (Some(ApplyAction::Created), _) => "k8s.create",
                _ => "k8s.apply",
            };
            let err = r.error.as_ref().map(|e| e.message.as_str());
            audit_with(&s, &cu, env.id(), action, &doc_target(r), err).await;
        }
    }
    Ok(Json(results))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/templates", operation_id = "kubeTemplates", tag = "kubernetes",
    summary = "Starter YAML for creating each kind", params(EnvParam),
    responses((status = 200, description = "YAML by plural kind; NAMESPACE is a placeholder", body = std::collections::BTreeMap<String, String>)))]
pub async fn templates(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env_name): ApiPath<String>,
) -> ApiResult<Json<std::collections::BTreeMap<String, String>>> {
    kube_for(&s, &cu, &env_name, Need::AnyGrant).await?;
    Ok(Json(
        KubeKind::ALL
            .into_iter()
            .filter_map(|k| {
                fleetops_kube::templates::get(k).map(|t| (k.as_str().to_string(), t.to_string()))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod hint_tests {
    use super::access_hint;

    #[test]
    fn the_hint_names_the_level_that_would_allow_it() {
        assert!(access_hint(Some("read-only")).unwrap().contains("standard"));
        assert!(access_hint(Some("standard"))
            .unwrap()
            .contains("cluster-admin"));
        assert!(access_hint(Some("cluster-admin")).is_none());
        assert!(access_hint(None).is_none());
    }
}
