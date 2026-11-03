//! Turns scan results into batches: what is new to each rule, and what is
//! only the existing state (baseline).

use super::db;
use super::model::{in_scope, matches, repository, PendingItem, PendingKind, Rule};
use crate::scan::db as sdb;
use crate::scan::engine::ScanEvents;
use crate::scan::model::{Finding, ImageKey, ImageUse};
use async_trait::async_trait;
use fleetops_api::AppState;
use fleetops_core::edition::Feature;
use fleetops_core::{now_unix, Result};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The notifications side of [`ScanEvents`].
pub struct Matcher;

fn covers(rule: &Rule, env_id: i64) -> bool {
    rule.envs.is_empty() || rule.envs.contains(&env_id)
}

fn repo_of(u: &ImageUse) -> String {
    repository(&u.image_ref, u.image_id.as_deref())
}

async fn env_names(s: &AppState) -> Result<HashMap<i64, String>> {
    Ok(s.store
        .list_environments()
        .await?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect())
}

/// Every image use FleetOps knows, in every environment.
async fn all_uses(s: &AppState, names: &HashMap<i64, String>) -> Result<Vec<ImageUse>> {
    let ids: Vec<i64> = names.keys().copied().collect();
    sdb::uses_for_envs(&s.store, &ids).await
}

fn items(
    repo: &str,
    uses: &[&ImageUse],
    f: &Finding,
    names: &HashMap<i64, String>,
) -> Vec<PendingItem> {
    // One line per (image, environment), with its running count.
    let mut by: BTreeMap<(String, i64), u32> = BTreeMap::new();
    for u in uses {
        *by.entry((u.image_ref.clone(), u.env_id)).or_default() += u32::from(u.running);
    }
    by.into_iter()
        .map(|((image_ref, env_id), running)| PendingItem {
            repository: repo.to_string(),
            image_ref,
            env: names.get(&env_id).cloned().unwrap_or_default(),
            running,
            cve: f.vuln_id.clone(),
            severity: f.severity,
            pkg_name: f.pkg_name.clone(),
            installed_version: f.installed_version.clone(),
            fixed_version: f.fixed_version.clone(),
            title: f.title.clone(),
        })
        .collect()
}

/// Applies `findings` of `key` to each of `rules`.
async fn process(
    s: &AppState,
    rules: &[Rule],
    key: &ImageKey,
    findings: &[Finding],
    uses: &[ImageUse],
    names: &HashMap<i64, String>,
) -> Result<()> {
    let now = now_unix();
    let of_key: Vec<&ImageUse> = uses.iter().filter(|u| &u.key == key).collect();
    for rule in rules.iter().filter(|r| r.enabled) {
        let mut by_repo: BTreeMap<String, Vec<&ImageUse>> = BTreeMap::new();
        for u in of_key.iter().filter(|u| in_scope(rule, u)) {
            by_repo.entry(repo_of(u)).or_default().push(u);
        }
        let hits: Vec<&Finding> = findings.iter().filter(|f| matches(rule, f)).collect();
        for (repo, us) in by_repo {
            match db::known(&s.store, rule.id, &repo).await? {
                // Baseline: the existing state, recorded without a message.
                Some(true) => {
                    let cves: Vec<String> = hits.iter().map(|f| f.vuln_id.clone()).collect();
                    db::mark_reported(&s.store, rule.id, &repo, &cves, now).await?;
                    db::set_known(&s.store, rule.id, &repo, false).await?;
                }
                // New to the rule: one summary for the repository.
                None => {
                    if hits.is_empty() {
                        db::set_known(&s.store, rule.id, &repo, false).await?;
                        continue;
                    }
                    let all: Vec<PendingItem> = hits
                        .iter()
                        .flat_map(|f| items(&repo, &us, f, names))
                        .collect();
                    db::add_pending(&s.store, rule.id, PendingKind::Image, &repo, &all, now)
                        .await?;
                }
                // Known: one message per CVE it hasn't been told about.
                Some(false) => {
                    for f in &hits {
                        if db::is_reported(&s.store, rule.id, &repo, &f.vuln_id).await? {
                            continue;
                        }
                        let it = items(&repo, &us, f, names);
                        db::add_pending(&s.store, rule.id, PendingKind::Cve, &f.vuln_id, &it, now)
                            .await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The findings of each key's latest finished scan, for keys that have one.
async fn stored(s: &AppState, keys: &[ImageKey]) -> Result<Vec<(ImageKey, Vec<Finding>)>> {
    let mut out = Vec::new();
    for (key, st) in sdb::latest(&s.store, keys).await? {
        if let Some(done) = st.last_done {
            let f = sdb::findings(&s.store, done.id).await?;
            out.push((key, f.into_iter().map(|r| r.finding).collect()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Marks every repository in `rules`' scope among `uses` as baseline
/// pending, then records the ones that already have results.
async fn baseline(
    s: &AppState,
    rules: &[Rule],
    env: Option<i64>,
    uses: &[ImageUse],
    names: &HashMap<i64, String>,
) -> Result<()> {
    let mut keys: HashSet<ImageKey> = HashSet::new();
    for rule in rules {
        for u in uses
            .iter()
            .filter(|u| covers(rule, u.env_id) && env.is_none_or(|e| u.env_id == e))
        {
            db::set_known(&s.store, rule.id, &repo_of(u), true).await?;
            keys.insert(u.key.clone());
        }
    }
    let keys: Vec<ImageKey> = keys.into_iter().collect();
    for (key, findings) in stored(s, &keys).await? {
        process(s, rules, &key, &findings, uses, names).await?;
    }
    Ok(())
}

/// A rule was created or changed: what it now covers is the existing state.
pub async fn baseline_rule(s: &AppState, rule_id: i64) -> Result<()> {
    let Some(rule) = db::get_rule(&s.store, rule_id).await? else {
        return Ok(());
    };
    let names = env_names(s).await?;
    let uses = all_uses(s, &names).await?;
    // Repositories it already knew keep their state; only new scope baselines.
    let mut fresh = Vec::new();
    for u in &uses {
        if covers(&rule, u.env_id) && db::known(&s.store, rule.id, &repo_of(u)).await?.is_none() {
            fresh.push(u.clone());
        }
    }
    baseline(s, std::slice::from_ref(&rule), None, &fresh, &names).await
}

impl Matcher {
    async fn on_scan(&self, s: &AppState, key: &ImageKey, findings: &[Finding]) -> Result<()> {
        if !s.entitlements.has(Feature::Notifications) {
            return Ok(());
        }
        let rules = db::list_rules(&s.store).await?;
        if rules.is_empty() {
            return Ok(());
        }
        let names = env_names(s).await?;
        let uses = all_uses(s, &names).await?;
        process(s, &rules, key, findings, &uses, &names).await
    }

    async fn on_inventory(
        &self,
        s: &AppState,
        env_id: i64,
        after: &[ImageUse],
        enabling: bool,
    ) -> Result<()> {
        if !s.entitlements.has(Feature::Notifications) {
            return Ok(());
        }
        let rules: Vec<Rule> = db::list_rules(&s.store)
            .await?
            .into_iter()
            .filter(|r| covers(r, env_id))
            .collect();
        if rules.is_empty() {
            return Ok(());
        }
        let names = env_names(s).await?;
        let uses = all_uses(s, &names).await?;
        if enabling {
            return baseline(s, &rules, Some(env_id), after, &names).await;
        }
        // Repositories new to a rule here (promoted, or arrived and already
        // scanned elsewhere) are judged on the results FleetOps already has.
        let mut keys: HashSet<ImageKey> = HashSet::new();
        for rule in &rules {
            for u in after.iter().filter(|u| in_scope(rule, u)) {
                if db::known(&s.store, rule.id, &repo_of(u)).await?.is_none() {
                    keys.insert(u.key.clone());
                }
            }
        }
        let keys: Vec<ImageKey> = keys.into_iter().collect();
        for (key, findings) in stored(s, &keys).await? {
            process(s, &rules, &key, &findings, &uses, &names).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ScanEvents for Matcher {
    async fn scan_finished(&self, s: &AppState, key: &ImageKey, findings: &[Finding]) {
        if let Err(e) = self.on_scan(s, key, findings).await {
            tracing::warn!(error = %e, "notifications: could not match a scan");
        }
    }

    async fn inventory_changed(
        &self,
        s: &AppState,
        env_id: i64,
        _before: &[ImageUse],
        after: &[ImageUse],
        enabling: bool,
    ) {
        if let Err(e) = self.on_inventory(s, env_id, after, enabling).await {
            tracing::warn!(error = %e, "notifications: could not match an inventory");
        }
    }
}
