//! What a Swarm stack deploy will do (swarm spec §5), from the new
//! project, the deployed one and the live swarm. Pure.

use super::docker::network_hash;
use super::model::*;
use super::plan::{Plan, ResourceAction, ResourcePlan, ServiceAction, ServicePlan};
use super::swarm::{
    canonical, named, object_name, predefined, service_name, service_spec, spec_changes, spec_hash,
    LABEL_SPEC_HASH,
};
use crate::swarm::{ServiceDetail, SwarmNetwork};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The swarm as a stack's plan sees it.
#[derive(Debug, Clone, Default)]
pub struct SwarmLive {
    /// The stack's services (by stack label), with their specs.
    pub services: Vec<ServiceDetail>,
    /// Every secret and config name in the swarm.
    pub secrets: Vec<String>,
    pub configs: Vec<String>,
    /// The names of those labelled with this stack.
    pub stack_secrets: Vec<String>,
    pub stack_configs: Vec<String>,
    /// Every secret's and config's stack, by name (`None`: in no stack).
    pub secret_stacks: BTreeMap<String, Option<String>>,
    pub config_stacks: BTreeMap<String, Option<String>>,
    /// Every swarm-scoped network.
    pub networks: Vec<SwarmNetwork>,
    /// This stack's networks' config-hash label, by name.
    pub network_hashes: BTreeMap<String, String>,
    /// Names, by id, of networks the stack's services join that `networks`
    /// lacks, such as the swarm's own `host` network.
    pub other_networks: BTreeMap<String, String>,
}

impl SwarmLive {
    /// Network names by id, to read a live spec with [`named`].
    pub fn network_names(&self) -> BTreeMap<String, String> {
        self.networks
            .iter()
            .map(|n| (n.id.clone(), n.name.clone()))
            .chain(self.other_networks.clone())
            .collect()
    }

    /// The live service `name`.
    pub fn service(&self, name: &str) -> Option<&ServiceDetail> {
        self.services.iter().find(|s| s.summary.name == name)
    }
}

const NOT_OURS: &str = "not deployed by FleetOps";

fn replicas_of(svc: &Service) -> u32 {
    match svc.swarm.as_ref().map(|d| d.mode).unwrap_or_default() {
        SwarmMode::Replicated | SwarmMode::ReplicatedJob => svc.replicas,
        SwarmMode::Global | SwarmMode::GlobalJob => 0,
    }
}

/// What the live service changed since `deployed` rendered it, when its
/// spec no longer matches its own hash label.
pub fn drift(live: &Value, deployed: Option<&Project>, key: &str) -> Vec<String> {
    let label = live["Labels"][LABEL_SPEC_HASH].as_str();
    if label.is_none() || label == Some(spec_hash(live).as_str()) {
        return vec![];
    }
    let changes = deployed
        .filter(|p| p.services.contains_key(key))
        .map(|p| spec_changes(&canonical(&service_spec(p, key)), &canonical(live)))
        .unwrap_or_default();
    if changes.is_empty() {
        vec!["changed outside the stack".to_string()]
    } else {
        changes
    }
}

/// Applies a deployer's stack scope to `plan` (swarm spec §6): an external
/// secret, config or network of no stack in `scope` blocks it, a grant
/// limited to stacks lending a stack nothing of another's. A missing one
/// and one outside the scope read the same, so the plan tells the deployer
/// nothing about objects they may not see.
pub fn apply_scope(plan: &mut Plan, project: &Project, live: &SwarmLive, scope: &BTreeSet<String>) {
    let lines = outside_scope(project, live, scope);
    let missing: Vec<String> = lines
        .iter()
        .map(|l| {
            l.replace(
                " does not exist or is outside your stacks",
                " does not exist",
            )
        })
        .collect();
    plan.blocking.retain(|b| !missing.contains(b));
    plan.blocking.extend(lines);
}

fn outside_scope(project: &Project, live: &SwarmLive, scope: &BTreeSet<String>) -> Vec<String> {
    let inside = |stack: Option<&String>| stack.is_some_and(|s| scope.contains(s));
    let mut out = Vec::new();
    for (kind, decls, stacks) in [
        (FileKind::Secret, &project.secrets, &live.secret_stacks),
        (FileKind::Config, &project.configs, &live.config_stacks),
    ] {
        let used: BTreeSet<&String> = project
            .services
            .values()
            .flat_map(|s| &s.files)
            .filter(|f| f.kind == kind)
            .map(|f| &f.source)
            .collect();
        for key in used {
            if let Some(FileSource::External { name }) = decls.get(key) {
                if !inside(stacks.get(name).and_then(Option::as_ref)) {
                    out.push(format!(
                        "external {} {name} does not exist or is outside your stacks",
                        kind.as_str()
                    ));
                }
            }
        }
    }
    for net in project
        .networks
        .values()
        .filter(|n| n.external && !predefined(&n.name))
    {
        let stack = live
            .networks
            .iter()
            .find(|n| n.name == net.name)
            .and_then(|n| n.stack.as_ref());
        if !inside(stack) {
            out.push(format!(
                "external network {} does not exist or is outside your stacks",
                net.name
            ));
        }
    }
    out
}

pub fn plan_swarm(new: &Project, deployed: Option<(&Project, i64)>, live: &SwarmLive) -> Plan {
    let names = live.network_names();
    let deployed_project = deployed.map(|(p, _)| p);
    let mut blocking = Vec::new();
    let mut services = Vec::new();
    let mut rendered = BTreeMap::new();
    for (key, svc) in &new.services {
        let spec = service_spec(new, key);
        let after = replicas_of(svc);
        let plan = match live.service(&service_name(new, key)) {
            None => ServicePlan {
                name: key.clone(),
                action: ServiceAction::Create,
                reasons: vec![],
                replicas_before: 0,
                replicas_after: after,
                drift: vec![],
            },
            Some(l) => {
                let current = named(&l.spec, &names);
                let before = l
                    .summary
                    .replicas_desired
                    .unwrap_or(l.summary.tasks_desired) as u32;
                let label = current["Labels"][LABEL_SPEC_HASH]
                    .as_str()
                    .map(str::to_string);
                let drift = drift(&current, deployed_project, key);
                let (action, reasons) = match label {
                    None => (ServiceAction::Update, vec![NOT_OURS.to_string()]),
                    Some(h) if h != spec["Labels"][LABEL_SPEC_HASH] || !drift.is_empty() => {
                        let r = spec_changes(&canonical(&current), &canonical(&spec));
                        let r = if r.is_empty() {
                            vec!["configuration changed".to_string()]
                        } else {
                            r
                        };
                        (ServiceAction::Update, r)
                    }
                    Some(_) => (ServiceAction::Unchanged, vec![]),
                };
                ServicePlan {
                    name: key.clone(),
                    action,
                    reasons,
                    replicas_before: before,
                    replicas_after: after,
                    drift,
                }
            }
        };
        services.push(plan);
        rendered.insert(service_name(new, key), spec);
    }
    let prefix = format!("{}_", new.name);
    for l in &live.services {
        if rendered.contains_key(&l.summary.name) {
            continue;
        }
        services.push(ServicePlan {
            name: l
                .summary
                .name
                .strip_prefix(&prefix)
                .unwrap_or(&l.summary.name)
                .to_string(),
            action: ServiceAction::Remove,
            reasons: vec!["no longer in the file".into()],
            replicas_before: l
                .summary
                .replicas_desired
                .unwrap_or(l.summary.tasks_desired) as u32,
            replicas_after: 0,
            drift: vec![],
        });
    }

    let mut networks = Vec::new();
    let mut wanted = BTreeSet::new();
    for net in new.networks.values() {
        wanted.insert(net.name.clone());
        let live_net = live.networks.iter().find(|n| n.name == net.name);
        let found = live_net.is_some();
        if net.external {
            if !found && !predefined(&net.name) {
                blocking.push(format!("external network {} does not exist", net.name));
            }
            continue;
        }
        // A stack's own network is labelled with it; joining any other one
        // must be said, as `external`, so it is checked as one.
        if live_net.is_some_and(|n| n.stack.as_deref() != Some(new.name.as_str())) {
            blocking.push(format!(
                "network {} already exists outside this stack; declare it external",
                net.name
            ));
            continue;
        }
        if found {
            if live
                .network_hashes
                .get(&net.name)
                .is_some_and(|h| *h != network_hash(net))
            {
                blocking.push(format!(
                    "network {} changed; remove the stack and deploy again",
                    net.name
                ));
            }
            networks.push(ResourcePlan {
                name: net.name.clone(),
                action: ResourceAction::Exists,
            });
        } else {
            networks.push(ResourcePlan {
                name: net.name.clone(),
                action: ResourceAction::Create,
            });
        }
    }
    for n in &live.networks {
        if n.stack.as_deref() == Some(new.name.as_str()) && !wanted.contains(&n.name) {
            networks.push(ResourcePlan {
                name: n.name.clone(),
                action: ResourceAction::Remove,
            });
        }
    }

    let mut objects = |kind: FileKind, all: &[String], ours: &[String]| {
        let decls = match kind {
            FileKind::Secret => &new.secrets,
            FileKind::Config => &new.configs,
        };
        let used: BTreeSet<&String> = new
            .services
            .values()
            .flat_map(|s| &s.files)
            .filter(|f| f.kind == kind)
            .map(|f| &f.source)
            .collect();
        let mut out = Vec::new();
        let mut wanted = BTreeSet::new();
        for key in used {
            let name = object_name(new, kind, key);
            if let Some(FileSource::External { .. }) = decls.get(key) {
                if !all.contains(&name) {
                    blocking.push(format!("external {} {name} does not exist", kind.as_str()));
                }
                continue;
            }
            let action = if all.contains(&name) {
                ResourceAction::Exists
            } else {
                ResourceAction::Create
            };
            wanted.insert(name.clone());
            out.push(ResourcePlan { name, action });
        }
        for name in ours {
            if !wanted.contains(name) {
                out.push(ResourcePlan {
                    name: name.clone(),
                    action: ResourceAction::Remove,
                });
            }
        }
        out
    };
    let secrets = objects(FileKind::Secret, &live.secrets, &live.stack_secrets);
    let configs = objects(FileKind::Config, &live.configs, &live.stack_configs);

    let mut versions: Vec<(&String, u64)> = live
        .services
        .iter()
        .map(|s| (&s.summary.name, s.version))
        .collect();
    versions.sort();
    let sorted = |v: &[String]| -> Vec<String> {
        let mut v = v.to_vec();
        v.sort();
        v
    };
    let mut nets: Vec<&String> = live.networks.iter().map(|n| &n.name).collect();
    nets.sort();
    let token = hex::encode(Sha256::digest(
        serde_json::to_vec(&json!([
            new,
            deployed.map(|(_, n)| n).unwrap_or(0),
            versions,
            sorted(&live.secrets),
            sorted(&live.configs),
            nets,
            live.network_hashes,
        ]))
        .expect("serialisable"),
    ));
    Plan {
        services,
        networks,
        volumes: vec![],
        images: vec![],
        secrets,
        configs,
        blocking,
        token,
    }
}
