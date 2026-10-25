//! Swarm stacks (swarm spec §5): the live swarm a plan is made against,
//! and deploys a restart cut off.

use super::recover::INTERRUPTED;
use super::try_lock;
use crate::routes::swarm_protect::service_guard;
use crate::AppState;
use fleetops_core::compose::docker::LABEL_HASH;
use fleetops_core::compose::plan::{Plan, ServiceAction};
use fleetops_core::compose::swarm_plan::SwarmLive;
use fleetops_core::swarm::SwarmOps;
use fleetops_core::Result;
use fleetops_store::StackRecord;

/// The swarm as `stack`'s plan sees it: its services with their specs,
/// every secret, config and swarm network, and which of them are its own.
pub(crate) async fn live(ops: &dyn SwarmOps, stack: &str) -> Result<SwarmLive> {
    let ours = |s: &Option<String>| s.as_deref() == Some(stack);
    let mut services = Vec::new();
    for svc in ops.services().await?.into_iter().filter(|s| ours(&s.stack)) {
        if let Some(d) = ops.service(&svc.id).await? {
            services.push(d);
        }
    }
    let secrets = ops.secrets().await?;
    let configs = ops.configs().await?;
    let networks = ops.networks().await?;
    Ok(SwarmLive {
        services,
        stack_secrets: secrets
            .iter()
            .filter(|s| ours(&s.stack))
            .map(|s| s.name.clone())
            .collect(),
        secrets: secrets.into_iter().map(|s| s.name).collect(),
        stack_configs: configs
            .iter()
            .filter(|c| ours(&c.stack))
            .map(|c| c.name.clone())
            .collect(),
        configs: configs.into_iter().map(|c| c.name).collect(),
        network_hashes: networks
            .iter()
            .filter(|n| ours(&n.stack))
            .filter_map(|n| Some((n.name.clone(), n.labels.get(LABEL_HASH)?.clone())))
            .collect(),
        networks,
    })
}

/// Refuses a plan that would change or remove a service FleetOps runs on:
/// the deploy could stop itself half way.
pub(crate) fn protect(plan: &mut Plan, live: &SwarmLive, stack: &str) {
    for sp in &plan.services {
        if !matches!(sp.action, ServiceAction::Update | ServiceAction::Remove) {
            continue;
        }
        let name = format!("{stack}_{}", sp.name);
        if let Some(l) = live.service(&name) {
            if service_guard(&l.summary).is_some() {
                plan.blocking.push(format!(
                    "service '{name}' runs FleetOps; change it outside FleetOps"
                ));
            }
        }
    }
}

/// A swarm deploy a restart cut off is marked failed. Nothing is undone:
/// Swarm already rolled out what it was given.
pub(crate) async fn settle_interrupted(s: &AppState, stack: &StackRecord) -> Result<()> {
    let Some(_lock) = try_lock(s, stack.id) else {
        return Ok(());
    };
    let Some(latest) = s.store.list_revisions(stack.id).await?.into_iter().next() else {
        return Ok(());
    };
    if latest.status == "deploying" {
        s.store
            .finish_revision(stack.id, latest.number, Some(INTERRUPTED))
            .await?;
        tracing::info!(stack = %stack.name, revision = latest.number, "an interrupted swarm deploy was marked failed");
    }
    Ok(())
}
