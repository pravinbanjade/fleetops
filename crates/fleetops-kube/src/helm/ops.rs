//! Helm releases against the cluster (spec §2.3, §3, §4). Every call goes
//! through the same kube-rs client as the rest of `KubeOps`, so it works
//! over a kubeconfig and through an agent tunnel alike.

use super::storage::{self, Release};
use super::{mask, plan};
use crate::client::{cleaned, outcome, timed, KubeClient};
use fleetops_core::helm::{
    is_pending, HelmChange, HelmObjectChange, HelmPlan, HelmRelease, HelmRevision, ReleaseDetail,
    CHANGED, HOOKS_NOTE, UNREADABLE,
};
use fleetops_core::kube::{ApplyError, FIELD_MANAGER};
use fleetops_core::{Error, Result};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams};
use kube::discovery::ApiResource;
use std::collections::BTreeSet;

/// Longest description written to a failed revision.
const DESCRIPTION_MAX: usize = 1000;

use super::hooks::{self, HookState};
use fleetops_core::helm::{ChartRef, Deploy, HelmMode, RenderedHook};
use std::time::Duration;

const HOOK_TIMEOUT: Duration = Duration::from_secs(300);
const HOOK_POLL: Duration = Duration::from_secs(2);
const HOOK_DELETE_WAIT: Duration = Duration::from_secs(60);
pub const CHART_SOURCE: &str = "fleetops.io/chart-source";

/// The revision an install continues from: 0 for a new name, an
/// uninstalled release's last version, or a refusal.
fn install_base(name: &str, latest: Option<&Release>) -> Result<u32> {
    match latest {
        None => Ok(0),
        Some(r) if r.info.status == "uninstalled" => Ok(r.version),
        Some(_) => Err(Error::Conflict(format!(
            "release '{name}' exists; upgrade it"
        ))),
    }
}

fn unreadable_err(e: String) -> Error {
    Error::Upstream {
        status: 422,
        message: format!("the release record cannot be read: {e}"),
    }
}

fn release_selector(name: &str) -> String {
    format!("{},name={name}", storage::OWNER_SELECTOR)
}

/// "Kind name: message" for each object that failed.
fn failures(changes: &[HelmObjectChange]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|c| {
            c.error
                .as_ref()
                .map(|e| format!("{} {}: {}", c.kind, c.name, e.message))
        })
        .collect()
}

fn truncate(mut s: String) -> String {
    if s.len() > DESCRIPTION_MAX {
        let mut cut = DESCRIPTION_MAX;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

/// One list row: an unreadable record is a row saying so; one removed
/// since the metadata list (an uninstall racing the poll) is no row.
fn list_row(ns: &str, name: &str, v: u32, r: Result<Release>) -> Result<Option<HelmRelease>> {
    match r {
        Ok(r) => Ok(Some(storage::summary(&r))),
        Err(Error::Upstream {
            status: 422,
            message,
        }) => Ok(Some(storage::unreadable(name, ns, v, message))),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `live` carries this release's ownership metadata.
fn owned_by(live: &serde_json::Value, release: &str, release_ns: &str) -> bool {
    let ann = |k: &str| {
        live.pointer("/metadata/annotations")
            .and_then(|a| a.get(k))
            .and_then(serde_json::Value::as_str)
    };
    ann("meta.helm.sh/release-name") == Some(release)
        && ann("meta.helm.sh/release-namespace") == Some(release_ns)
}

fn new_plan(current: u32) -> HelmPlan {
    HelmPlan {
        current,
        note: HOOKS_NOTE.into(),
        ..Default::default()
    }
}

impl KubeClient {
    fn release_secrets(&self, ns: Option<&str>) -> Api<Secret> {
        match ns {
            Some(ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        }
    }

    /// (namespace, name, version) of every release Secret matching `selector`.
    async fn release_metas(
        &self,
        ns: Option<&str>,
        selector: &str,
    ) -> Result<Vec<(String, String, u32)>> {
        let lp = ListParams::default().labels(selector);
        let list = timed(self.release_secrets(ns).list_metadata(&lp)).await?;
        Ok(list
            .items
            .into_iter()
            .filter_map(|m| {
                let l = m.metadata.labels?;
                Some((
                    m.metadata.namespace?,
                    l.get("name")?.clone(),
                    l.get("version")?.parse().ok()?,
                ))
            })
            .collect())
    }

    async fn read_release(&self, ns: &str, name: &str, version: u32) -> Result<Release> {
        let s = timed(
            self.release_secrets(Some(ns))
                .get_opt(&storage::secret_name(name, version)),
        )
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!("revision {version} of release '{name}' not found"))
        })?;
        let raw = s
            .data
            .as_ref()
            .and_then(|d| d.get("release"))
            .map(|b| b.0.clone())
            .unwrap_or_default();
        storage::decode(&raw).map_err(unreadable_err)
    }

    async fn latest(&self, ns: &str, name: &str) -> Result<Release> {
        let v = self
            .release_metas(Some(ns), &release_selector(name))
            .await?
            .into_iter()
            .map(|(_, _, v)| v)
            .max()
            .ok_or_else(|| Error::NotFound(format!("release '{name}' not found")))?;
        self.read_release(ns, name, v).await
    }

    /// Creates (`create`) or replaces the Secret holding `r`. A create that
    /// finds the Secret already there lost a race with another operation.
    async fn write_release(
        &self,
        r: &Release,
        create: bool,
        source: Option<&ChartRef>,
    ) -> Result<()> {
        let api = self.release_secrets(Some(&r.namespace));
        let name = storage::secret_name(&r.name, r.version);
        let mut s = if create {
            Secret {
                type_: Some(storage::SECRET_TYPE.into()),
                ..Secret::default()
            }
        } else {
            timed(api.get(&name)).await?
        };
        s.metadata.name = Some(name.clone());
        s.metadata.namespace = Some(r.namespace.clone());
        s.metadata.labels = Some(storage::labels(r));
        if let Some(src) = source {
            s.metadata
                .annotations
                .get_or_insert_with(Default::default)
                .insert(
                    CHART_SOURCE.into(),
                    serde_json::to_string(src).expect("a chart reference serialises"),
                );
        }
        s.data = Some([("release".to_string(), ByteString(storage::encode(r)))].into());
        let pp = PostParams {
            dry_run: false,
            field_manager: Some(FIELD_MANAGER.into()),
        };
        if create {
            timed(api.create(&pp, &s)).await.map_err(|e| match e {
                Error::Conflict(_) => Error::Conflict(CHANGED.into()),
                e => e,
            })?;
        } else {
            timed(api.replace(&name, &pp, &s)).await?;
        }
        Ok(())
    }

    /// The resource of every step, after namespace defaulting, refusing
    /// the whole action when `allowed` does not cover an object.
    async fn resolve_steps(
        &self,
        steps: &mut [plan::Step],
        release_ns: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<Vec<Result<ApiResource>>> {
        let mut out = Vec::with_capacity(steps.len());
        for s in steps.iter_mut() {
            match self.resolve(&s.doc.api_version, &s.doc.kind).await {
                Ok((ar, namespaced)) => {
                    crate::apply::set_namespace(&mut s.doc, namespaced, release_ns);
                    fleetops_core::access::check_doc(
                        allowed,
                        namespaced,
                        s.doc.namespace.as_deref(),
                    )?;
                    out.push(Ok(ar));
                }
                // A kind the cluster does not serve: its scope is unknown,
                // so a limited grant cannot be sure it covers it.
                Err(_) if allowed.is_some() => {
                    return Err(fleetops_core::access::cluster_scoped_denied());
                }
                Err(e) => out.push(Err(e)),
            }
        }
        Ok(out)
    }

    /// Runs one step (or its dry run): the masked before and after.
    async fn step(
        &self,
        step: &mut plan::Step,
        ar: Result<ApiResource>,
        release: &str,
        release_ns: &str,
        dry_run: bool,
    ) -> Result<(Option<String>, Option<String>)> {
        let ar = ar?;
        let api: Api<DynamicObject> = match &step.doc.namespace {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        };
        let live = timed(api.get_opt(&step.doc.name))
            .await?
            .map(|o| serde_json::to_value(o).unwrap_or_default());
        match (step.change, live) {
            (HelmChange::Keep, live) => Ok((
                live.map(|v| crate::mask::to_yaml(&cleaned(v)))
                    .transpose()?,
                None,
            )),
            (HelmChange::Delete, None) => Ok((None, None)),
            (HelmChange::Delete, Some(live)) => {
                let dp = DeleteParams {
                    dry_run,
                    ..DeleteParams::default()
                };
                timed(api.delete(&step.doc.name, &dp)).await?;
                Ok((Some(crate::mask::to_yaml(&cleaned(live))?), None))
            }
            (HelmChange::Patch, Some(live)) => {
                let pp = PatchParams {
                    dry_run,
                    field_manager: Some(FIELD_MANAGER.into()),
                    ..PatchParams::default()
                };
                let patch = step.patch.clone().unwrap_or_default();
                let out = timed(api.patch(&step.doc.name, &pp, &Patch::Merge(&patch))).await?;
                let (_, before, after) =
                    outcome(Some(live), serde_json::to_value(out).unwrap_or_default())?;
                Ok((before, Some(after)))
            }
            // Ours already (kept by an upgrade, or left by a partly failed
            // operation): Helm takes it back, so do we.
            (HelmChange::Create, Some(live)) if owned_by(&live, release, release_ns) => {
                plan::adopt(&mut step.doc.value, release, release_ns);
                let pp = PatchParams {
                    dry_run,
                    field_manager: Some(FIELD_MANAGER.into()),
                    ..PatchParams::default()
                };
                let out =
                    timed(api.patch(&step.doc.name, &pp, &Patch::Merge(&step.doc.value))).await?;
                let (_, before, after) =
                    outcome(Some(live), serde_json::to_value(out).unwrap_or_default())?;
                Ok((before, Some(after)))
            }
            (HelmChange::Create, Some(_)) => Err(Error::Conflict(format!(
                "{} '{}' already exists outside this release",
                step.doc.kind, step.doc.name
            ))),
            // Created, or recreated after being deleted by hand.
            (HelmChange::Patch | HelmChange::Create, None) => {
                plan::adopt(&mut step.doc.value, release, release_ns);
                let obj: DynamicObject = serde_json::from_value(step.doc.value.clone())
                    .map_err(|e| Error::Invalid(format!("not a Kubernetes object: {e}")))?;
                let pp = PostParams {
                    dry_run,
                    field_manager: Some(FIELD_MANAGER.into()),
                };
                let out = timed(api.create(&pp, &obj)).await?;
                let (_, _, after) = outcome(None, serde_json::to_value(out).unwrap_or_default())?;
                Ok((None, Some(after)))
            }
        }
    }

    async fn run_steps(
        &self,
        steps: &mut [plan::Step],
        resources: Vec<Result<ApiResource>>,
        release: &str,
        release_ns: &str,
        dry_run: bool,
    ) -> Vec<HelmObjectChange> {
        let mut out = Vec::with_capacity(steps.len());
        for (step, ar) in steps.iter_mut().zip(resources) {
            let r = self.step(step, ar, release, release_ns, dry_run).await;
            let (before, after, error) = match r {
                Ok((b, a)) => (b, a, None),
                Err(e) => (None, None, Some(ApplyError::from(&e))),
            };
            out.push(HelmObjectChange {
                api_version: step.doc.api_version.clone(),
                kind: step.doc.kind.clone(),
                namespace: step.doc.namespace.clone(),
                name: step.doc.name.clone(),
                change: step.change,
                before,
                after,
                error,
            });
        }
        out
    }

    /// The release's latest revision, refusing a pending one and, for a
    /// commit, one that is not `expected`.
    async fn current(&self, ns: &str, name: &str, expected: u32, dry_run: bool) -> Result<Release> {
        let cur = self.latest(ns, name).await?;
        if is_pending(&cur.info.status) || (!dry_run && cur.version != expected) {
            return Err(Error::Conflict(CHANGED.into()));
        }
        Ok(cur)
    }

    async fn supersede_others(&self, ns: &str, name: &str, keep: u32) -> Result<()> {
        let sel = format!("{},status=deployed", release_selector(name));
        for (_, _, v) in self.release_metas(Some(ns), &sel).await? {
            if v != keep {
                let mut old = self.read_release(ns, name, v).await?;
                old.info.status = "superseded".into();
                self.write_release(&old, false, None).await?;
            }
        }
        Ok(())
    }

    /// The newest deployed revision, if any.
    async fn last_deployed(&self, ns: &str, name: &str) -> Result<Option<Release>> {
        let sel = format!("{},status=deployed", release_selector(name));
        match self
            .release_metas(Some(ns), &sel)
            .await?
            .into_iter()
            .map(|(_, _, v)| v)
            .max()
        {
            Some(v) => Ok(Some(self.read_release(ns, name, v).await?)),
            None => Ok(None),
        }
    }

    /// Where the chart came from: the newest revision Secret carrying FleetOps' annotation.
    async fn chart_source(&self, ns: &str, name: &str) -> Result<Option<ChartRef>> {
        let lp = ListParams::default().labels(&release_selector(name));
        let mut list = timed(self.release_secrets(Some(ns)).list_metadata(&lp))
            .await?
            .items;
        list.sort_by_key(|m| {
            std::cmp::Reverse(
                m.metadata
                    .labels
                    .as_ref()
                    .and_then(|l| l.get("version"))
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0),
            )
        });
        Ok(list.iter().find_map(|m| {
            m.metadata
                .annotations
                .as_ref()?
                .get(CHART_SOURCE)
                .and_then(|s| serde_json::from_str(s).ok())
        }))
    }

    pub(crate) async fn helm_config_impl(&self, ns: &str, name: &str) -> Result<serde_json::Value> {
        let r = self.latest(ns, name).await?;
        Ok(if r.config.is_null() {
            serde_json::json!({})
        } else {
            r.config
        })
    }

    /// Refuses the whole deploy before anything is written when a hook falls outside `allowed`.
    async fn check_hooks(
        &self,
        hs: &[RenderedHook],
        ns: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<()> {
        for h in hs {
            for mut doc in crate::apply::split_all(&h.manifest)? {
                match self.resolve(&doc.api_version, &doc.kind).await {
                    Ok((_, namespaced)) => {
                        crate::apply::set_namespace(&mut doc, namespaced, ns);
                        fleetops_core::access::check_doc(
                            allowed,
                            namespaced,
                            doc.namespace.as_deref(),
                        )?;
                    }
                    Err(_) if allowed.is_some() => {
                        return Err(fleetops_core::access::cluster_scoped_denied())
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// Runs one hook (spec 3j §3); the error names the hook.
    async fn run_hook(
        &self,
        h: &RenderedHook,
        ns: &str,
        phase: &str,
    ) -> std::result::Result<(), String> {
        let label = format!("{phase} hook {} {}", h.kind, h.name);
        let r = self.run_hook_inner(h, ns).await;
        r.map_err(|m| format!("{label}: {m}"))
    }

    async fn run_hook_inner(&self, h: &RenderedHook, ns: &str) -> std::result::Result<(), String> {
        let mut doc = crate::apply::split_all(&h.manifest)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .ok_or("the hook is empty")?;
        let (ar, namespaced) = self
            .resolve(&doc.api_version, &doc.kind)
            .await
            .map_err(|e| e.to_string())?;
        crate::apply::set_namespace(&mut doc, namespaced, ns);
        let api: Api<DynamicObject> = match &doc.namespace {
            Some(n) => Api::namespaced_with(self.client.clone(), n, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        };
        let pols = hooks::policies(h);
        if pols.contains(&"before-hook-creation") {
            let dp = DeleteParams::background();
            match timed(api.delete(&doc.name, &dp)).await {
                Ok(_) | Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e.to_string()),
            }
            let start = tokio::time::Instant::now();
            while timed(api.get_opt(&doc.name))
                .await
                .map_err(|e| e.to_string())?
                .is_some()
            {
                if start.elapsed() > HOOK_DELETE_WAIT {
                    return Err("the previous hook object was not deleted within 60s".into());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        let obj: DynamicObject =
            serde_json::from_value(doc.value.clone()).map_err(|e| e.to_string())?;
        let pp = PostParams {
            dry_run: false,
            field_manager: Some(FIELD_MANAGER.into()),
        };
        timed(api.create(&pp, &obj))
            .await
            .map_err(|e| e.to_string())?;
        let start = tokio::time::Instant::now();
        let outcome = loop {
            let live = timed(api.get(&doc.name)).await.map_err(|e| e.to_string())?;
            match hooks::state(&doc.kind, &serde_json::to_value(&live).unwrap_or_default()) {
                HookState::Succeeded => break Ok(()),
                HookState::Failed(m) => break Err(m),
                HookState::Running if start.elapsed() > HOOK_TIMEOUT => {
                    break Err(format!("did not finish within {}s", HOOK_TIMEOUT.as_secs()))
                }
                HookState::Running => tokio::time::sleep(HOOK_POLL).await,
            }
        };
        let delete = (outcome.is_ok() && pols.contains(&"hook-succeeded"))
            || (outcome.is_err() && pols.contains(&"hook-failed"));
        if delete {
            let _ = timed(api.delete(&doc.name, &DeleteParams::background())).await;
        }
        outcome
    }

    pub(crate) async fn helm_deploy_impl(
        &self,
        d: &Deploy,
        mode: HelmMode,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<HelmPlan> {
        let (ns, name) = (d.namespace.as_str(), d.name.as_str());
        let latest = match self.latest(ns, name).await {
            Ok(r) => Some(r),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        let (current, base_manifest, before_config, first_deployed) = if d.upgrade {
            let cur =
                latest.ok_or_else(|| Error::NotFound(format!("release '{name}' not found")))?;
            if is_pending(&cur.info.status) {
                return Err(Error::Conflict(CHANGED.into()));
            }
            let base = self
                .last_deployed(ns, name)
                .await?
                .unwrap_or_else(|| cur.clone());
            (
                cur.version,
                base.manifest.clone(),
                cur.config.clone(),
                cur.info.first_deployed.clone(),
            )
        } else {
            let v = install_base(name, latest.as_ref())?;
            (v, String::new(), serde_json::Value::Null, None)
        };
        if let HelmMode::Commit { expected_current } = mode {
            if expected_current != current {
                return Err(Error::Conflict(CHANGED.into()));
            }
        }
        let mut steps = plan::rollback_steps(&base_manifest, &d.rendered.manifest, ns)?;
        let resources = self.resolve_steps(&mut steps, ns, allowed).await?;
        self.check_hooks(&d.rendered.hooks, ns, allowed).await?;
        let act = if d.upgrade { "upgrade" } else { "install" };
        let mut out = HelmPlan {
            current,
            note: String::new(),
            hooks: hooks::steps(&d.rendered.hooks, d.upgrade),
            values_before: d.upgrade.then(|| mask::values_yaml(&before_config)),
            values_after: Some(mask::values_yaml(&d.config)),
            notes: d.rendered.notes.clone(),
            chart: Some(d.rendered.meta.clone()),
            ..Default::default()
        };
        if mode == HelmMode::Plan {
            out.changes = self.run_steps(&mut steps, resources, name, ns, true).await;
            return Ok(out);
        }
        let now = storage::now();
        let mut next = Release {
            name: name.into(),
            namespace: ns.into(),
            version: current + 1,
            info: storage::Info {
                status: format!("pending-{act}"),
                description: String::new(),
                notes: d.rendered.notes.clone(),
                first_deployed: first_deployed.or_else(|| Some(now.clone())),
                last_deployed: Some(now),
                rest: Default::default(),
            },
            chart: d.rendered.chart.clone(),
            config: d.config.clone(),
            manifest: d.rendered.manifest.clone(),
            rest: [(
                "hooks".to_string(),
                serde_json::to_value(&d.rendered.hooks).expect("hooks serialise"),
            )]
            .into_iter()
            .collect(),
        };
        self.write_release(&next, true, Some(&d.source)).await?;
        let mut failure = None;
        for h in hooks::for_phase(&d.rendered.hooks, &format!("pre-{act}")) {
            if let Err(m) = self.run_hook(h, ns, &format!("pre-{act}")).await {
                failure = Some(m);
                break;
            }
        }
        if failure.is_none() {
            out.changes = self.run_steps(&mut steps, resources, name, ns, false).await;
            let failed = failures(&out.changes);
            if !failed.is_empty() {
                failure = Some(failed.join("; "));
            }
        }
        if failure.is_none() {
            for h in hooks::for_phase(&d.rendered.hooks, &format!("post-{act}")) {
                if let Err(m) = self.run_hook(h, ns, &format!("post-{act}")).await {
                    failure = Some(m);
                    break;
                }
            }
        }
        let verb = if d.upgrade { "Upgrade" } else { "Install" };
        match failure {
            None => {
                next.info.status = "deployed".into();
                next.info.description = format!("{verb} complete");
                self.write_release(&next, false, None).await?;
                self.supersede_others(ns, name, next.version).await?;
            }
            Some(m) => {
                next.info.status = "failed".into();
                next.info.description = truncate(format!("{verb} failed: {m}"));
                self.write_release(&next, false, None).await?;
            }
        }
        out.revision = Some(next.version);
        out.status = Some(next.info.status.clone());
        out.note = next.info.description.clone();
        Ok(out)
    }

    pub(crate) async fn helm_list_impl(&self, ns: Option<&str>) -> Result<Vec<HelmRelease>> {
        let metas = self.release_metas(ns, storage::OWNER_SELECTOR).await?;
        let mut out = Vec::new();
        for (ns, name, v) in storage::latest_versions(metas) {
            let r = self.read_release(&ns, &name, v).await;
            out.extend(list_row(&ns, &name, v, r)?);
        }
        Ok(out)
    }

    pub(crate) async fn helm_get_impl(
        &self,
        ns: &str,
        name: &str,
        revision: Option<u32>,
    ) -> Result<ReleaseDetail> {
        let r = match revision {
            Some(v) => self.read_release(ns, name, v).await?,
            None => self.latest(ns, name).await?,
        };
        Ok(ReleaseDetail {
            release: storage::summary(&r),
            description: r.info.description.clone(),
            notes: r.info.notes.clone(),
            values: mask::values_yaml(&r.config),
            manifest: mask::manifest(&r.manifest),
            chart_source: self.chart_source(ns, name).await?,
        })
    }

    pub(crate) async fn helm_history_impl(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<Vec<HelmRevision>> {
        let mut versions: Vec<u32> = self
            .release_metas(Some(ns), &release_selector(name))
            .await?
            .into_iter()
            .map(|(_, _, v)| v)
            .collect();
        if versions.is_empty() {
            return Err(Error::NotFound(format!("release '{name}' not found")));
        }
        versions.sort_unstable();
        let mut out = Vec::with_capacity(versions.len());
        for v in versions {
            out.push(match self.read_release(ns, name, v).await {
                Ok(r) => storage::revision(&r),
                Err(Error::Upstream {
                    status: 422,
                    message,
                }) => HelmRevision {
                    revision: v,
                    status: UNREADABLE.into(),
                    chart_version: String::new(),
                    app_version: String::new(),
                    description: message,
                    updated: 0,
                },
                Err(e) => return Err(e),
            });
        }
        Ok(out)
    }

    pub(crate) async fn helm_rollback_impl(
        &self,
        ns: &str,
        name: &str,
        to: u32,
        expected: u32,
        dry_run: bool,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<HelmPlan> {
        let cur = self.current(ns, name, expected, dry_run).await?;
        if to == cur.version {
            return Err(Error::Invalid(format!(
                "revision {to} is already the current one"
            )));
        }
        let target = self.read_release(ns, name, to).await?;
        let mut steps = plan::rollback_steps(&cur.manifest, &target.manifest, ns)?;
        let resources = self.resolve_steps(&mut steps, ns, allowed).await?;
        let mut plan = new_plan(cur.version);
        if dry_run {
            plan.changes = self.run_steps(&mut steps, resources, name, ns, true).await;
            return Ok(plan);
        }
        // Helm's own rollback record: the target's chart, values and
        // manifest at the next version, pending until the objects are done.
        let mut next = target.clone();
        next.version = cur.version + 1;
        next.info.status = "pending-rollback".into();
        next.info.description = format!("Rollback to {to}");
        next.info.first_deployed = cur.info.first_deployed.clone();
        next.info.last_deployed = Some(storage::now());
        self.write_release(&next, true, None).await?;
        plan.changes = self.run_steps(&mut steps, resources, name, ns, false).await;
        let failed = failures(&plan.changes);
        if failed.is_empty() {
            next.info.status = "deployed".into();
            next.info.description = format!("Rollback to {to} (FleetOps; hooks not run)");
            self.write_release(&next, false, None).await?;
            // As Helm does: every other deployed revision is superseded; a
            // failed one keeps its status.
            self.supersede_others(ns, name, next.version).await?;
        } else {
            next.info.status = "failed".into();
            next.info.description =
                truncate(format!("Rollback to {to} failed: {}", failed.join("; ")));
            self.write_release(&next, false, None).await?;
        }
        plan.revision = Some(next.version);
        plan.status = Some(next.info.status.clone());
        Ok(plan)
    }

    pub(crate) async fn helm_uninstall_impl(
        &self,
        ns: &str,
        name: &str,
        expected: u32,
        dry_run: bool,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<HelmPlan> {
        let mut cur = self.current(ns, name, expected, dry_run).await?;
        let mut steps = plan::uninstall_steps(&cur.manifest)?;
        let resources = self.resolve_steps(&mut steps, ns, allowed).await?;
        let mut plan = new_plan(cur.version);
        if dry_run {
            plan.changes = self.run_steps(&mut steps, resources, name, ns, true).await;
            return Ok(plan);
        }
        cur.info.status = "uninstalling".into();
        self.write_release(&cur, false, None).await?;
        plan.changes = self.run_steps(&mut steps, resources, name, ns, false).await;
        let failed = failures(&plan.changes);
        if failed.is_empty() {
            let api = self.release_secrets(Some(ns));
            for (_, _, v) in self
                .release_metas(Some(ns), &release_selector(name))
                .await?
            {
                match timed(api.delete(&storage::secret_name(name, v), &DeleteParams::default()))
                    .await
                {
                    Ok(_) | Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            plan.status = Some("uninstalled".into());
        } else {
            cur.info.status = "failed".into();
            cur.info.description = truncate(format!("Uninstall failed: {}", failed.join("; ")));
            self.write_release(&cur, false, None).await?;
            plan.status = Some("failed".into());
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> Release {
        storage::decode(
            &base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                r#"{"name":"a","namespace":"apps","version":2,"info":{"status":"deployed"}}"#,
            )
            .into_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn a_list_row_survives_an_unreadable_or_vanished_release() {
        let ok = list_row("apps", "a", 2, Ok(rec())).unwrap().unwrap();
        assert_eq!(ok.status, "deployed");
        let bad = list_row("apps", "a", 2, Err(unreadable_err("not gzip".into())))
            .unwrap()
            .unwrap();
        assert_eq!(bad.status, UNREADABLE);
        // Uninstalled between the metadata list and the read: not a row, not an error.
        let gone = list_row("apps", "a", 2, Err(Error::NotFound("gone".into()))).unwrap();
        assert!(gone.is_none());
        assert!(list_row("apps", "a", 2, Err(Error::Forbidden("no".into()))).is_err());
    }

    #[test]
    fn an_install_over_a_live_release_is_refused_and_over_an_uninstalled_one_continues() {
        let mut live = rec();
        live.info.status = "deployed".into();
        let e = install_base("a", Some(&live)).unwrap_err().to_string();
        assert!(e.contains("release 'a' exists; upgrade it"), "{e}");
        live.info.status = "uninstalled".into();
        assert_eq!(install_base("a", Some(&live)).unwrap(), 2);
        assert_eq!(install_base("a", None).unwrap(), 0);
    }
}
