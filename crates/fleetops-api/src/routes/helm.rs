//! `/envs/{env}/k8s/helm/…`: Helm releases in one Kubernetes environment
//! (spec 3d §6).

use super::kube::{
    access_hint, agent_level, check_name, check_namespace, kube_for, with_hint, KubeCtx,
};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use fleetops_core::helm::{
    ChartRef, ChartShow, Deploy, HelmMode, HelmPlan, HelmRelease, HelmRevision, ReleaseDetail,
    HELM_OFF,
};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};

const HELM_VIEW: Permission = Permission::new(Row::Helm, Verb::View);
use fleetops_kube::helm::{
    render::{ChartRenderer, RenderInput},
    values,
};
use fleetops_store::NewAudit;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
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
    let k8 = kube_for(&s, &cu, &p.env, Need::Perm(HELM_VIEW)).await?;
    if let Some(ns) = ns {
        crate::authz::deny_audited(
            &s,
            &cu,
            k8.env.id(),
            "helm.view",
            k8.grants.check_ns(HELM_VIEW, ns),
        )
        .await?;
    }
    let rows = k8.ops.helm_list(ns).await?;
    Ok(Json(
        rows.into_iter()
            .filter(|r| k8.grants.allows(HELM_VIEW, Some(&r.namespace)))
            .collect(),
    ))
}

async fn readable(s: &AppState, cu: &CurrentUser, p: &ReleasePath) -> Result<KubeCtx> {
    p.checked()?;
    let k8 = kube_for(s, cu, &p.env, Need::Perm(HELM_VIEW)).await?;
    crate::authz::deny_audited(
        s,
        cu,
        k8.env.id(),
        "helm.view",
        k8.grants.check_ns(HELM_VIEW, &p.ns),
    )
    .await?;
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
    let mut d = k8.ops.helm_get(&p.ns, &p.name, q.revision).await?;
    if !k8
        .grants
        .allows(Permission::new(Row::Helm, Verb::Reveal), Some(&p.ns))
    {
        d.values.clear();
        d.notes.clear();
        d.hidden = true;
    }
    Ok(Json(d))
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

async fn writable(s: &AppState, cu: &CurrentUser, p: &ReleasePath, verb: Verb) -> Result<KubeCtx> {
    p.checked()?;
    let perm = Permission::new(Row::Helm, verb);
    let k8 = kube_for(s, cu, &p.env, Need::Perm(perm)).await?;
    crate::authz::deny_audited(
        s,
        cu,
        k8.env.id(),
        &perm.to_string(),
        k8.grants.check_ns(perm, &p.ns),
    )
    .await?;
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
    user_id: i64,
    env_id: i64,
    action: &str,
    target: &str,
    detail: serde_json::Value,
    ok: bool,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(user_id),
        environment_id: Some(env_id),
        action,
        target_type: Some("helm"),
        target_id: Some(target),
        result: if ok { "success" } else { "failure" },
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

/// Runs a rollback or uninstall, and its audit row, on a task of its own:
/// a caller that disconnects mid-commit must not leave the release
/// half-written and locked `pending-rollback`.
async fn detached<F>(f: F) -> Result<HelmPlan>
where
    F: std::future::Future<Output = Result<HelmPlan>> + Send + 'static,
{
    tokio::spawn(f)
        .await
        .map_err(|e| Error::Internal(format!("the Helm operation stopped: {e}")))?
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
    let k8 = writable(&s, &cu, &p, Verb::Operate).await?;
    let (ops, allowed, env_id) = (
        k8.ops.clone(),
        k8.grants
            .namespaces_for(Permission::new(Row::Helm, Verb::Operate)),
        k8.env.id(),
    );
    let (st, user_id, ns, name, target) = (
        s.clone(),
        cu.user.id,
        p.ns.clone(),
        p.name.clone(),
        p.target(),
    );
    let r = detached(async move {
        let r = ops
            .helm_rollback(&ns, &name, body.revision, body.expected_current, body.dry_run, allowed.as_ref())
            .await;
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
            audit(&st, user_id, env_id, "helm.rollback", &target, detail, ok).await;
        }
        r
    })
    .await;
    let r = crate::authz::deny_audited(&s, &cu, env_id, "helm.operate", r).await;
    let plan = hide_values(&k8, &p.ns, r?);
    Ok(Json(hinted(&s, &k8, plan).await))
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
    let k8 = writable(&s, &cu, &p, Verb::Delete).await?;
    let (ops, allowed, env_id) = (
        k8.ops.clone(),
        k8.grants
            .namespaces_for(Permission::new(Row::Helm, Verb::Delete)),
        k8.env.id(),
    );
    let (st, user_id, ns, name, target) = (
        s.clone(),
        cu.user.id,
        p.ns.clone(),
        p.name.clone(),
        p.target(),
    );
    let r = detached(async move {
        let r = ops
            .helm_uninstall(
                &ns,
                &name,
                body.expected_current,
                body.dry_run,
                allowed.as_ref(),
            )
            .await;
        if !body.dry_run {
            let rev = body.expected_current;
            let (ok, detail) = match &r {
                Ok(plan) => (
                    plan.status.as_deref() == Some("uninstalled"),
                    json!({"revision": rev, "status": plan.status}),
                ),
                Err(e) => (false, json!({"revision": rev, "error": e.to_string()})),
            };
            audit(&st, user_id, env_id, "helm.uninstall", &target, detail, ok).await;
        }
        r
    })
    .await;
    let r = crate::authz::deny_audited(&s, &cu, env_id, "helm.delete", r).await;
    Ok(Json(hinted(&s, &k8, r?).await))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct NamespacePath {
    pub env: String,
    /// The release's namespace.
    pub ns: String,
}

#[derive(Serialize, ToSchema)]
pub struct HelmCapabilities {
    /// Whether install and upgrade are available (the server has --helm-binary).
    pub install: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ShowBody {
    pub chart: ChartRef,
}

#[derive(Deserialize, ToSchema)]
pub struct InstallBody {
    pub name: String,
    pub chart: ChartRef,
    /// YAML; empty for none.
    #[serde(default)]
    pub values: String,
    pub dry_run: bool,
    /// The plan's `current`; 0 for a new release.
    #[serde(default)]
    pub expected_current: u32,
}

#[derive(Deserialize, ToSchema)]
pub struct UpgradeBody {
    pub chart: ChartRef,
    /// YAML; masked values sent back unchanged keep their stored value.
    #[serde(default)]
    pub values: String,
    pub dry_run: bool,
    #[serde(default)]
    pub expected_current: u32,
}

fn renderer(s: &AppState) -> Result<Arc<dyn ChartRenderer>> {
    s.config
        .helm
        .clone()
        .ok_or_else(|| Error::Invalid(HELM_OFF.into()))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/helm/capabilities", operation_id = "helmCapabilities", tag = "helm",
    summary = "Whether charts can be installed and upgraded here", params(ReleasesPath),
    responses((status = 200, description = "Capabilities", body = HelmCapabilities)))]
pub async fn capabilities(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasesPath>,
) -> ApiResult<Json<HelmCapabilities>> {
    kube_for(&s, &cu, &p.env, Need::AnyGrant).await?;
    Ok(Json(HelmCapabilities {
        install: s.config.helm.is_some(),
    }))
}

#[utoipa::path(post, path = "/envs/{env}/k8s/helm/charts/show", operation_id = "showHelmChart", tag = "helm",
    summary = "A chart's metadata and default values", params(ReleasesPath), request_body = ShowBody,
    responses((status = 200, description = "The chart", body = ChartShow)))]
pub async fn show(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasesPath>,
    ApiJson(body): ApiJson<ShowBody>,
) -> ApiResult<Json<ChartShow>> {
    let r = renderer(&s)?;
    body.chart.validate(r.insecure_repos())?;
    kube_for(&s, &cu, &p.env, Need::of(Row::Helm, Verb::Create)).await?;
    Ok(Json(r.show(&body.chart).await?))
}

/// Renders with the cluster's version and API groups. helm reads the
/// values as typed, plus an overlay of what was restored from `stored`, so
/// it parses them by its own rules; the release stores helm's result.
#[allow(clippy::too_many_arguments)]
async fn render(
    r: &dyn ChartRenderer,
    k8: &KubeCtx,
    ns: &str,
    name: &str,
    chart: &ChartRef,
    values_yaml: &str,
    stored: &serde_json::Value,
    upgrade: bool,
) -> Result<fleetops_core::helm::Rendered> {
    let parsed = values::parse(values_yaml)?;
    let (full, overlay) = values::restore(&parsed, stored)?;
    let input = RenderInput {
        name: name.into(),
        namespace: ns.into(),
        chart: chart.clone(),
        values: full,
        values_yaml: values_yaml.into(),
        overlay,
        upgrade,
        kube_version: k8.ops.version().await?.git_version,
        api_versions: k8.ops.api_versions().await?,
    };
    r.render(&input).await
}

/// Plans or commits on a task of its own (as rollback does), auditing a commit.
#[allow(clippy::too_many_arguments)]
async fn deploy(
    s: &AppState,
    cu: &CurrentUser,
    k8: &KubeCtx,
    target: String,
    d: Deploy,
    dry_run: bool,
    expected_current: u32,
    action: &'static str,
) -> ApiResult<Json<HelmPlan>> {
    let ns = d.namespace.clone();
    let perm = if action == "helm.install" {
        Permission::new(Row::Helm, Verb::Create)
    } else {
        Permission::new(Row::Helm, Verb::Operate)
    };
    let (ops, allowed, env_id) = (k8.ops.clone(), k8.grants.namespaces_for(perm), k8.env.id());
    let (st, user_id) = (s.clone(), cu.user.id);
    let mode = if dry_run {
        HelmMode::Plan
    } else {
        HelmMode::Commit { expected_current }
    };
    let r = detached(async move {
        let r = ops.helm_deploy(&d, mode, allowed.as_ref()).await;
        if !dry_run {
            let (ok, detail) = match &r {
                Ok(plan) => (
                    plan.status.as_deref() == Some("deployed"),
                    json!({"chart": d.source.label(), "version": d.rendered.meta.version, "revision": plan.revision, "status": plan.status}),
                ),
                Err(e) => (false, json!({"chart": d.source.label(), "version": d.rendered.meta.version, "error": e.to_string()})),
            };
            audit(&st, user_id, env_id, action, &target, detail, ok).await;
        }
        r
    })
    .await;
    let r = crate::authz::deny_audited(s, cu, env_id, &perm.to_string(), r).await;
    let plan = hide_values(k8, &ns, r?);
    Ok(Json(hinted(s, k8, plan).await))
}

/// A plan as a role without Reveal on Helm releases sees it: no values or notes.
fn hide_values(k8: &KubeCtx, ns: &str, mut plan: HelmPlan) -> HelmPlan {
    if !k8
        .grants
        .allows(Permission::new(Row::Helm, Verb::Reveal), Some(ns))
    {
        plan.values_before = None;
        plan.values_after = None;
        plan.notes.clear();
    }
    plan
}

#[utoipa::path(post, path = "/envs/{env}/k8s/helm/releases/{ns}", operation_id = "installHelmChart", tag = "helm",
    summary = "Plan or commit installing a chart", params(NamespacePath), request_body = InstallBody,
    responses((status = 200, description = "The plan, or what was done", body = HelmPlan)))]
pub async fn install(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<NamespacePath>,
    ApiJson(body): ApiJson<InstallBody>,
) -> ApiResult<Json<HelmPlan>> {
    let r = renderer(&s)?;
    body.chart.validate(r.insecure_repos())?;
    let rp = ReleasePath {
        env: p.env,
        ns: p.ns,
        name: body.name.clone(),
    };
    let k8 = writable(&s, &cu, &rp, Verb::Create).await?;
    let none = serde_json::Value::Null;
    let rendered = render(
        r.as_ref(),
        &k8,
        &rp.ns,
        &rp.name,
        &body.chart,
        &body.values,
        &none,
        false,
    )
    .await?;
    let d = Deploy {
        namespace: rp.ns.clone(),
        name: rp.name.clone(),
        config: rendered.config.clone(),
        rendered,
        source: body.chart,
        upgrade: false,
    };
    deploy(
        &s,
        &cu,
        &k8,
        rp.target(),
        d,
        body.dry_run,
        body.expected_current,
        "helm.install",
    )
    .await
}

#[utoipa::path(post, path = "/envs/{env}/k8s/helm/releases/{ns}/{name}/upgrade", operation_id = "upgradeHelmRelease", tag = "helm",
    summary = "Plan or commit upgrading a release", params(ReleasePath), request_body = UpgradeBody,
    responses((status = 200, description = "The plan, or what was done", body = HelmPlan)))]
pub async fn upgrade(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ReleasePath>,
    ApiJson(body): ApiJson<UpgradeBody>,
) -> ApiResult<Json<HelmPlan>> {
    let r = renderer(&s)?;
    body.chart.validate(r.insecure_repos())?;
    let k8 = writable(&s, &cu, &p, Verb::Operate).await?;
    values::parse(&body.values)?;
    let stored = k8.ops.helm_config(&p.ns, &p.name).await?;
    let rendered = render(
        r.as_ref(),
        &k8,
        &p.ns,
        &p.name,
        &body.chart,
        &body.values,
        &stored,
        true,
    )
    .await?;
    let d = Deploy {
        namespace: p.ns.clone(),
        name: p.name.clone(),
        config: rendered.config.clone(),
        rendered,
        source: body.chart,
        upgrade: true,
    };
    deploy(
        &s,
        &cu,
        &k8,
        p.target(),
        d,
        body.dry_run,
        body.expected_current,
        "helm.upgrade",
    )
    .await
}
