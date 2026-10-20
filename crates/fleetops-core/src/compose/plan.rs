//! What a deploy will do (stacks spec §3), worked out from the new project,
//! the deployed one and what is running. Pure.

use super::docker::{network_hash, service_hash, LABEL_HASH, LABEL_PROJECT, LABEL_SERVICE};
use super::model::*;
use crate::docker::{ContainerSummary, NetworkSummary, VolumeSummary};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;

/// Marks a container a deploy has set aside (renamed) until it commits.
pub const OLD_MARKER: &str = "-fleetops-old-";

/// The stack's live objects, plus every local image tag.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub containers: Vec<ContainerSummary>,
    pub networks: Vec<NetworkSummary>,
    pub volumes: Vec<VolumeSummary>,
    /// Local `repo:tag` references.
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceAction {
    Create,
    Recreate,
    Scale,
    Unchanged,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceAction {
    Create,
    Exists,
    Remove,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ServicePlan {
    pub name: String,
    pub action: ServiceAction,
    pub reasons: Vec<String>,
    pub replicas_before: u32,
    pub replicas_after: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResourcePlan {
    /// The Docker name.
    pub name: String,
    pub action: ResourceAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ImagePlan {
    pub image: String,
    pub pull: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Plan {
    pub services: Vec<ServicePlan>,
    pub networks: Vec<ResourcePlan>,
    pub volumes: Vec<ResourcePlan>,
    pub images: Vec<ImagePlan>,
    /// Problems that stop this plan from being deployed.
    pub blocking: Vec<String>,
    pub token: String,
}

/// `nginx` → `nginx:latest`; a digest or explicit tag is kept.
pub fn normalise_image(image: &str) -> String {
    let last = image.rsplit('/').next().unwrap_or(image);
    if image.contains('@') || last.contains(':') {
        image.to_string()
    } else {
        format!("{image}:latest")
    }
}

fn in_project(labels: &BTreeMap<String, String>, project: &str) -> bool {
    labels.get(LABEL_PROJECT).map(String::as_str) == Some(project)
}

/// Field-level differences between two versions of a service. Values are
/// never shown, except the image.
pub fn service_reasons(old: &Service, new: &Service) -> Vec<String> {
    let mut out = Vec::new();
    if old.image != new.image {
        out.push(format!("image: {} → {}", old.image, new.image));
    }
    let (Value::Object(a), Value::Object(b)) = (json!(old), json!(new)) else {
        return out;
    };
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for k in keys {
        if matches!(k.as_str(), "image" | "name" | "replicas") {
            continue;
        }
        let (va, vb) = (
            a.get(k).unwrap_or(&Value::Null),
            b.get(k).unwrap_or(&Value::Null),
        );
        if va == vb {
            continue;
        }
        match (va, vb) {
            (Value::Object(ma), Value::Object(mb))
                if matches!(k.as_str(), "environment" | "labels") =>
            {
                let sub: BTreeSet<&String> = ma.keys().chain(mb.keys()).collect();
                for s in sub {
                    match (ma.get(s), mb.get(s)) {
                        (None, Some(_)) => out.push(format!("{k}.{s} added")),
                        (Some(_), None) => out.push(format!("{k}.{s} removed")),
                        (Some(x), Some(y)) if x != y => out.push(format!("{k}.{s} changed")),
                        _ => {}
                    }
                }
            }
            _ => out.push(format!("{k} changed")),
        }
    }
    out
}

pub fn plan(new: &Project, deployed: Option<(&Project, i64)>, live: &Live) -> Plan {
    let name = &new.name;
    let containers: Vec<&ContainerSummary> = live
        .containers
        .iter()
        .filter(|c| in_project(&c.labels, name) || c.compose_project.as_deref() == Some(name))
        .filter(|c| !c.name.contains(OLD_MARKER))
        .collect();
    let by_service = |svc: &str| -> Vec<&ContainerSummary> {
        containers
            .iter()
            .copied()
            .filter(|c| {
                c.labels.get(LABEL_SERVICE).map(String::as_str) == Some(svc)
                    || c.compose_service.as_deref() == Some(svc)
            })
            .collect()
    };
    let mut blocking = Vec::new();
    let mut services = Vec::new();
    let mut needs_containers = BTreeSet::new();
    for (sname, svc) in &new.services {
        let current = by_service(sname);
        let before = current.len() as u32;
        let hash = service_hash(svc);
        let (action, reasons) = if current.is_empty() {
            (ServiceAction::Create, vec![])
        } else if current.iter().any(|c| !c.labels.contains_key(LABEL_HASH)) {
            (
                ServiceAction::Recreate,
                vec!["not created by FleetOps".to_string()],
            )
        } else if current.iter().any(|c| c.labels[LABEL_HASH] != hash) {
            let reasons = match deployed.and_then(|(p, _)| p.services.get(sname)) {
                Some(old) => {
                    let r = service_reasons(old, svc);
                    if r.is_empty() {
                        vec!["configuration changed".to_string()]
                    } else {
                        r
                    }
                }
                None => vec!["configuration changed".to_string()],
            };
            (ServiceAction::Recreate, reasons)
        } else if before != svc.replicas {
            (
                ServiceAction::Scale,
                vec![format!("replicas: {before} → {}", svc.replicas)],
            )
        } else {
            (ServiceAction::Unchanged, vec![])
        };
        if matches!(action, ServiceAction::Create | ServiceAction::Recreate)
            || (action == ServiceAction::Scale && svc.replicas > before)
        {
            needs_containers.insert(sname.clone());
        }
        services.push(ServicePlan {
            name: sname.clone(),
            action,
            reasons,
            replicas_before: before,
            replicas_after: svc.replicas,
        });
    }
    let live_services: BTreeSet<String> = containers
        .iter()
        .filter_map(|c| {
            c.labels
                .get(LABEL_SERVICE)
                .cloned()
                .or_else(|| c.compose_service.clone())
        })
        .collect();
    for orphan in live_services.difference(&new.services.keys().cloned().collect()) {
        services.push(ServicePlan {
            name: orphan.clone(),
            action: ServiceAction::Remove,
            reasons: vec!["no longer in the file".into()],
            replicas_before: by_service(orphan).len() as u32,
            replicas_after: 0,
        });
    }

    let mut networks = Vec::new();
    let mut wanted_networks = BTreeSet::new();
    for net in new.networks.values() {
        wanted_networks.insert(net.name.clone());
        let found = live.networks.iter().find(|n| n.name == net.name);
        if net.external {
            if found.is_none() {
                blocking.push(format!("external network {} does not exist", net.name));
            }
            continue;
        }
        match found {
            None => networks.push(ResourcePlan {
                name: net.name.clone(),
                action: ResourceAction::Create,
            }),
            Some(n) => {
                if n.labels
                    .get(LABEL_HASH)
                    .is_some_and(|h| *h != network_hash(net))
                {
                    blocking.push(format!(
                        "network {} changed; remove the stack (keeping volumes) and deploy again",
                        net.name
                    ));
                }
                networks.push(ResourcePlan {
                    name: net.name.clone(),
                    action: ResourceAction::Exists,
                });
            }
        }
    }
    for n in &live.networks {
        if in_project(&n.labels, name) && !wanted_networks.contains(&n.name) {
            networks.push(ResourcePlan {
                name: n.name.clone(),
                action: ResourceAction::Remove,
            });
        }
    }

    let mut volumes = Vec::new();
    let mut wanted_volumes = BTreeSet::new();
    for vol in new.volumes.values() {
        wanted_volumes.insert(vol.name.clone());
        let found = live.volumes.iter().any(|v| v.name == vol.name);
        if vol.external {
            if !found {
                blocking.push(format!("external volume {} does not exist", vol.name));
            }
            continue;
        }
        volumes.push(ResourcePlan {
            name: vol.name.clone(),
            action: if found {
                ResourceAction::Exists
            } else {
                ResourceAction::Create
            },
        });
    }
    for v in &live.volumes {
        if in_project(&v.labels, name) && !wanted_volumes.contains(&v.name) {
            volumes.push(ResourcePlan {
                name: v.name.clone(),
                action: ResourceAction::Kept,
            });
        }
    }

    let local: BTreeSet<String> = live.images.iter().map(|i| normalise_image(i)).collect();
    let mut images: BTreeMap<String, bool> = BTreeMap::new();
    for sname in &needs_containers {
        let svc = &new.services[sname];
        let present = local.contains(&normalise_image(&svc.image));
        let pull = match svc.pull_policy {
            PullPolicy::Always => true,
            PullPolicy::Missing => !present,
            PullPolicy::Never => {
                if !present {
                    blocking.push(format!(
                        "image {} is not present and pull_policy is never",
                        svc.image
                    ));
                }
                false
            }
        };
        let e = images.entry(svc.image.clone()).or_insert(false);
        *e |= pull;
    }
    let images = images
        .into_iter()
        .map(|(image, pull)| ImagePlan { image, pull })
        .collect();

    let mut pairs: Vec<(String, String)> = containers
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                c.labels.get(LABEL_HASH).cloned().unwrap_or_default(),
            )
        })
        .collect();
    pairs.sort();
    let token = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!([new, deployed.map(|(_, n)| n).unwrap_or(0), pairs]))
                .expect("serialisable")
        )
    );
    Plan {
        services,
        networks,
        volumes,
        images,
        blocking,
        token,
    }
}
