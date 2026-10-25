//! Self-protection (swarm spec §3): before a change that can stop FleetOps
//! or disconnect the swarm, the caller types the object's name. FleetOps'
//! own service and the agent service are known by their `io.fleetops.role`
//! label, or by image for stacks deployed before the label existed.

use fleetops_core::swarm::{ServiceSummary, SwarmOps, ROLE_LABEL};
use fleetops_core::{Error, Result};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Server,
    Agent,
}

/// The repository of `image`: no digest, tag or `docker.io/`.
fn repository(image: &str) -> &str {
    let image = image.split('@').next().unwrap_or_default();
    let image = match image.rfind(':') {
        Some(i) if !image[i..].contains('/') => &image[..i],
        _ => image,
    };
    image.strip_prefix("docker.io/").unwrap_or(image)
}

/// Which part of FleetOps a service runs, if any.
pub fn part_of(svc: &ServiceSummary) -> Option<Part> {
    match svc.labels.get(ROLE_LABEL).map(String::as_str) {
        Some("server") => return Some(Part::Server),
        Some("agent") => return Some(Part::Agent),
        _ => {}
    }
    match repository(&svc.image) {
        "fleets/fleetops" => Some(Part::Server),
        "fleets/fleetops-agent" => Some(Part::Agent),
        _ => None,
    }
}

/// Why changing `svc` needs its name typed, if it does.
pub fn service_guard(svc: &ServiceSummary) -> Option<String> {
    let name = &svc.name;
    part_of(svc).map(|p| match p {
        Part::Server => format!("'{name}' is FleetOps' own service; this can stop FleetOps."),
        Part::Agent => format!("'{name}' runs the FleetOps agents; this can disconnect the swarm."),
    })
}

/// The nodes running FleetOps' own service.
pub async fn fleetops_nodes(ops: &dyn SwarmOps) -> Result<HashSet<String>> {
    let servers: HashSet<String> = ops
        .services()
        .await?
        .into_iter()
        .filter(|s| part_of(s) == Some(Part::Server))
        .map(|s| s.id)
        .collect();
    if servers.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(ops
        .tasks()
        .await?
        .into_iter()
        .filter(|t| servers.contains(&t.service_id) && t.desired_state == "running")
        .filter_map(|t| t.node_id)
        .collect())
}

/// Why draining or removing node `id` needs its name typed, if it does.
pub async fn node_guard(ops: &dyn SwarmOps, id: &str, hostname: &str) -> Result<Option<String>> {
    Ok(fleetops_nodes(ops)
        .await?
        .contains(id)
        .then(|| format!("FleetOps runs on node '{hostname}'; this can stop FleetOps.")))
}

/// Passes when nothing is protected or `confirm` is `name`; `Ok(true)`
/// means a protection was confirmed.
pub fn require(reason: Option<String>, name: &str, confirm: Option<&str>) -> Result<bool> {
    match reason {
        None => Ok(false),
        Some(_) if confirm == Some(name) => Ok(true),
        Some(r) => Err(Error::ConfirmRequired(format!(
            "{r} Type '{name}' to confirm."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories() {
        assert_eq!(
            repository("docker.io/fleets/fleetops-agent:0.7.0@sha256:ab"),
            "fleets/fleetops-agent"
        );
        assert_eq!(repository("fleets/fleetops"), "fleets/fleetops");
        assert_eq!(
            repository("localhost:5000/fleets/fleetops"),
            "localhost:5000/fleets/fleetops"
        );
    }

    #[test]
    fn confirmation() {
        assert!(!require(None, "x", None).unwrap());
        assert!(require(Some("r.".into()), "x", Some("x")).unwrap());
        let e = require(Some("r.".into()), "x", Some("y")).unwrap_err();
        assert_eq!(e.code(), "confirm_required");
        assert_eq!(e.to_string(), "r. Type 'x' to confirm.");
    }
}
