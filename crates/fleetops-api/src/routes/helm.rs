//! `/envs/{env}/k8s/helm/…`: Helm releases in one Kubernetes environment
//! (spec 3d §6).

use super::kube::{
    access_hint, agent_level, check_name, check_namespace, kube_for, with_hint, KubeCtx,
};
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use fleetops_core::helm::{HelmPlan, HelmRelease, HelmRevision, ReleaseDetail};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ReleasesPath {
    /// Environment name.
    pub env: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ReleasePath {
    pub env: String,
    /// The release's namespace.
    pub ns: String,
    /// Release name.
    pub name: String,
}

impl ReleasePath {
    fn checked(&self) -> Result<()> {
        if self.ns == "-" {
            return Err(Error::Invalid("a release always has a namespace".into()));
        }
        check_namespace(&self.ns)?;
        check_name(&self.name)
    }

    fn target(&self) -> String {
        format!("{}/{}", self.ns, self.name)
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReleasesQuery {
    /// Omit for all namespaces.
    pub namespace: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RevisionQuery {
    /// Omit for the latest.
    pub revision: Option<u32>,
}

#[derive(Deserialize, ToSchema)]
pub struct RollbackBody {
    pub revision: u32,
    /// The latest revision the plan was reviewed against.
    pub expected_current: u32,
    pub dry_run: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct UninstallBody {
    /// The latest revision the plan was reviewed against.
    pub expected_current: u32,
    pub dry_run: bool,
}

#[utoipa::path(get, path = "/envs/{env}/k8s/helm/releases", operation_id = "listHelmReleases", tag = "helm",
    summary = "Helm releases at their latest revision", params(ReleasesPath, ReleasesQuery),
    responses((status = 200, description = "Releases", body = [HelmRelease])))]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasesPath>,
    Query(q): Query<ReleasesQuery>,
) -> ApiResult<Json<Vec<HelmRelease>>> {
    let ns = q.namespace.as_deref().filter(|n| !n.is_empty());
    if let Some(ns) = ns {
        check_namespace(ns)?;
    }
    let k8 = kube_for(&s, &cu, &p.env, Action::KubeRead).await?;
    if let Some(ns) = ns {
        crate::authz::deny_audited(&s, &cu, k8.env.id(), "k8s.read", k8.grant.check_ns(ns)).await?;
    }
    let rows = k8.ops.helm_list(ns).await?;
    Ok(Json(
        rows.into_iter()
            .filter(|r| k8.grant.allows_ns(&r.namespace))
            .collect(),
    ))
}

async fn readable(s: &AppState, cu: &CurrentUser, p: &ReleasePath) -> Result<KubeCtx> {
    p.checked()?;
    let k8 = kube_for(s, cu, &p.env, Action::KubeRead).await?;
    crate::authz::deny_audited(s, cu, k8.env.id(), "k8s.read", k8.grant.check_ns(&p.ns)).await?;
    Ok(k8)
}

#[utoipa::path(get, path = "/envs/{env}/k8s/helm/releases/{ns}/{name}", operation_id = "getHelmRelease", tag = "helm",
    summary = "One revision of a release: chart, status, masked values and manifest, notes", params(ReleasePath, RevisionQuery),
    responses((status = 200, description = "The revision", body = ReleaseDetail)))]
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasePath>,
    Query(q): Query<RevisionQuery>,
) -> ApiResult<Json<ReleaseDetail>> {
    let k8 = readable(&s, &cu, &p).await?;
    Ok(Json(k8.ops.helm_get(&p.ns, &p.name, q.revision).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/helm/releases/{ns}/{name}/history", operation_id = "helmHistory", tag = "helm",
    summary = "Every stored revision, oldest first", params(ReleasePath),
    responses((status = 200, description = "Revisions", body = [HelmRevision])))]
pub async fn history(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasePath>,
) -> ApiResult<Json<Vec<HelmRevision>>> {
    let k8 = readable(&s, &cu, &p).await?;
    Ok(Json(k8.ops.helm_history(&p.ns, &p.name).await?))
}

async fn writable(s: &AppState, cu: &CurrentUser, p: &ReleasePath) -> Result<KubeCtx> {
    p.checked()?;
    let k8 = kube_for(s, cu, &p.env, Action::KubeWrite).await?;
    crate::authz::deny_audited(s, cu, k8.env.id(), "k8s.write", k8.grant.check_ns(&p.ns)).await?;
    Ok(k8)
}

/// Adds the agent's access-level hint to each object the cluster refused.
async fn hinted(s: &AppState, k8: &KubeCtx, mut plan: HelmPlan) -> HelmPlan {
    let level = agent_level(s, &k8.env).await;
    if let Some(hint) = access_hint(level.as_deref()) {
        for e in plan
            .changes
            .iter_mut()
            .filter_map(|c| c.error.as_mut())
            .filter(|e| e.code == "forbidden")
        {
            e.message = with_hint(std::mem::take(&mut e.message), Some(hint));
        }
    }
    plan
}

/// One row per committed rollback or uninstall. Never values.
async fn audit(
    s: &AppState,
    cu: &CurrentUser,
    k8: &KubeCtx,
    action: &str,
    p: &ReleasePath,
    detail: serde_json::Value,
    ok: bool,
) {
    let target = p.target();
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(k8.env.id()),
        action,
        target_type: Some("helm"),
        target_id: Some(&target),
        result: if ok { "success" } else { "failure" },
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

#[utoipa::path(post, path = "/envs/{env}/k8s/helm/releases/{ns}/{name}/rollback", operation_id = "rollbackHelmRelease", tag = "helm",
    summary = "Plan or commit a rollback to an earlier revision", params(ReleasePath), request_body = RollbackBody,
    responses((status = 200, description = "The plan, or what was done", body = HelmPlan)))]
pub async fn rollback(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasePath>,
    ApiJson(body): ApiJson<RollbackBody>,
) -> ApiResult<Json<HelmPlan>> {
    let k8 = writable(&s, &cu, &p).await?;
    let r = k8
        .ops
        .helm_rollback(
            &p.ns,
            &p.name,
            body.revision,
            body.expected_current,
            body.dry_run,
            k8.grant.namespaces.as_ref(),
        )
        .await;
    let r = crate::authz::deny_audited(&s, &cu, k8.env.id(), "k8s.write", r).await;
    if !body.dry_run {
        let from = body.expected_current;
        let (ok, detail) = match &r {
            Ok(plan) => (
                plan.status.as_deref() == Some("deployed"),
                json!({"from": from, "to": body.revision, "revision": plan.revision, "status": plan.status}),
            ),
            Err(e) => (
                false,
                json!({"from": from, "to": body.revision, "error": e.to_string()}),
            ),
        };
        audit(&s, &cu, &k8, "helm.rollback", &p, detail, ok).await;
    }
    Ok(Json(hinted(&s, &k8, r?).await))
}

#[utoipa::path(post, path = "/envs/{env}/k8s/helm/releases/{ns}/{name}/uninstall", operation_id = "uninstallHelmRelease", tag = "helm",
    summary = "Plan or commit uninstalling a release", params(ReleasePath), request_body = UninstallBody,
    responses((status = 200, description = "The plan, or what was done", body = HelmPlan)))]
pub async fn uninstall(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasePath>,
    ApiJson(body): ApiJson<UninstallBody>,
) -> ApiResult<Json<HelmPlan>> {
    let k8 = writable(&s, &cu, &p).await?;
    let r = k8
        .ops
        .helm_uninstall(
            &p.ns,
            &p.name,
            body.expected_current,
            body.dry_run,
            k8.grant.namespaces.as_ref(),
        )
        .await;
    let r = crate::authz::deny_audited(&s, &cu, k8.env.id(), "k8s.write", r).await;
    if !body.dry_run {
        let rev = body.expected_current;
        let (ok, detail) = match &r {
            Ok(plan) => (
                plan.status.as_deref() == Some("uninstalled"),
                json!({"revision": rev, "status": plan.status}),
            ),
            Err(e) => (false, json!({"revision": rev, "error": e.to_string()})),
        };
        audit(&s, &cu, &k8, "helm.uninstall", &p, detail, ok).await;
    }
    Ok(Json(hinted(&s, &k8, r?).await))
}
