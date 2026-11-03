//! Which images each environment has, and where they run.

use super::model::{image_key, ImageKey, ImageUse};
use async_trait::async_trait;
use fleetops_core::docker::{ContainerSummary, DockerOps, ImageSummary};
use fleetops_core::env::{EnvKind, Environment};
use fleetops_core::kube::KubeImageUse;
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Label and name prefix of the scanner's own containers.
pub const SCAN_LABEL: &str = "fleetops.scan";
pub const SCAN_NAME_PREFIX: &str = "fleetops-scan-";
/// The namespace Kubernetes scan Jobs run in.
pub const SCAN_NAMESPACE: &str = "fleetops-scan";

#[async_trait]
pub trait Inventory: Send + Sync {
    async fn uses(&self, env: &Arc<dyn Environment>) -> Result<Vec<ImageUse>>;
}

/// Reads environments through their Docker, Swarm and Kubernetes APIs.
pub struct LiveInventory;

#[async_trait]
impl Inventory for LiveInventory {
    async fn uses(&self, env: &Arc<dyn Environment>) -> Result<Vec<ImageUse>> {
        match env.kind() {
            k if k.is_kube() => {
                let kube = env
                    .kube()
                    .ok_or_else(|| Error::Invalid("not a Kubernetes environment".into()))?;
                Ok(kube_uses(env.id(), kube.image_uses().await?))
            }
            EnvKind::SwarmAgent => {
                let swarm = env
                    .swarm()
                    .ok_or_else(|| Error::Invalid("not a swarm".into()))?;
                let mut out = Vec::new();
                for n in swarm.nodes().await? {
                    let Some(d) = env.node(&n.id) else { continue };
                    match docker_host(env.id(), &n.id, d.as_ref()).await {
                        Ok(u) => out.extend(u),
                        Err(e) => {
                            tracing::info!(env = env.name(), node = n.hostname, error = %e,
                                "skipping an unreachable swarm node in the image inventory")
                        }
                    }
                }
                Ok(out)
            }
            _ => {
                let d = env
                    .docker()
                    .ok_or_else(|| Error::Invalid("not a Docker environment".into()))?;
                docker_host(env.id(), "", d.as_ref()).await
            }
        }
    }
}

async fn docker_host(env_id: i64, node: &str, d: &dyn DockerOps) -> Result<Vec<ImageUse>> {
    let images = d.list_images().await?;
    let mut inspected = Vec::with_capacity(images.len());
    for i in images {
        let v = d.inspect_image(&i.id).await.unwrap_or(Value::Null);
        inspected.push((i, v));
    }
    let containers = d.list_containers().await?;
    Ok(docker_uses(env_id, node, &inspected, &containers))
}

fn is_scanner(c: &ContainerSummary) -> bool {
    c.labels.contains_key(SCAN_LABEL)
        || c.name.trim_start_matches('/').starts_with(SCAN_NAME_PREFIX)
}

/// One use per container, plus one idle use for an image no container uses.
pub fn docker_uses(
    env_id: i64,
    node: &str,
    images: &[(ImageSummary, Value)],
    containers: &[ContainerSummary],
) -> Vec<ImageUse> {
    let mut out = Vec::new();
    let mut used: HashSet<&str> = HashSet::new();
    let by_id: HashMap<&str, &(ImageSummary, Value)> =
        images.iter().map(|i| (i.0.id.as_str(), i)).collect();
    let describe = |i: &(ImageSummary, Value)| {
        let digests: Vec<String> = i.1["RepoDigests"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let image_ref =
            i.0.repo_tags
                .iter()
                .find(|t| t.as_str() != "<none>:<none>")
                .cloned()
                .unwrap_or_else(|| i.0.id.clone());
        (image_key(env_id, node, &digests, &i.0.id), image_ref)
    };
    for c in containers.iter().filter(|c| !is_scanner(c)) {
        let Some(img) = by_id.get(c.image_id.as_str()) else {
            continue;
        };
        if img.1["Config"]["Labels"][SCAN_LABEL].is_string() {
            continue;
        }
        used.insert(c.image_id.as_str());
        let (key, image_ref) = describe(img);
        out.push(ImageUse {
            env_id,
            node: node.into(),
            key,
            image_ref,
            image_id: Some(img.0.id.clone()),
            namespace: String::new(),
            workload: c.compose_project.clone().unwrap_or_default(),
            container: c.name.trim_start_matches('/').to_string(),
            running: c.state == "running",
            pull_secrets: vec![],
        });
    }
    for img in images.iter().filter(|i| !used.contains(i.0.id.as_str())) {
        if img.1["Config"]["Labels"][SCAN_LABEL].is_string() {
            continue;
        }
        let (key, image_ref) = describe(img);
        out.push(ImageUse {
            env_id,
            node: node.into(),
            key,
            image_ref,
            image_id: Some(img.0.id.clone()),
            namespace: String::new(),
            workload: String::new(),
            container: String::new(),
            running: false,
            pull_secrets: vec![],
        });
    }
    out
}

/// Kubernetes uses keyed by the pulled digest; a template's image takes the
/// digest of a pod running the same image, if any.
pub fn kube_uses(env_id: i64, uses: Vec<KubeImageUse>) -> Vec<ImageUse> {
    let pulled: HashMap<String, String> = uses
        .iter()
        .filter_map(|u| u.image_id.clone().map(|d| (u.image.clone(), d)))
        .collect();
    uses.into_iter()
        .filter(|u| u.namespace != SCAN_NAMESPACE)
        .map(|u| {
            let key = u
                .image_id
                .clone()
                .or_else(|| pulled.get(&u.image).cloned())
                .map(ImageKey)
                .unwrap_or_else(|| ImageKey(format!("ref:{env_id}:{}", u.image)));
            ImageUse {
                env_id,
                node: String::new(),
                key,
                image_ref: u.image,
                image_id: None,
                namespace: u.namespace,
                workload: format!("{}/{}", u.owner_kind, u.owner_name),
                container: u.container,
                running: u.running,
                pull_secrets: u.pull_secrets,
            }
        })
        .collect()
}

/// Keys in `after` that `before` did not have: new images, and tags moved to
/// a new digest.
pub fn new_keys(before: &[ImageUse], after: &[ImageUse]) -> Vec<ImageKey> {
    let had: HashSet<&ImageKey> = before.iter().map(|u| &u.key).collect();
    let mut seen = HashSet::new();
    after
        .iter()
        .filter(|u| !had.contains(&u.key) && seen.insert(u.key.clone()))
        .map(|u| u.key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn image(id: &str, tag: &str, digest: Option<&str>) -> (ImageSummary, Value) {
        (
            ImageSummary {
                id: id.into(),
                repo_tags: vec![tag.into()],
                size: 1,
                created: 0,
                in_use: false,
            },
            json!({"RepoDigests": digest.map(|d| vec![format!("x@{d}")]).unwrap_or_default()}),
        )
    }

    fn container(name: &str, image_id: &str, state: &str) -> ContainerSummary {
        ContainerSummary {
            id: format!("id-{name}"),
            name: name.into(),
            image: "x".into(),
            image_id: image_id.into(),
            state: state.into(),
            status: String::new(),
            created: 0,
            ports: vec![],
            labels: BTreeMap::new(),
            compose_project: None,
            compose_service: None,
            volumes: vec![],
        }
    }

    #[test]
    fn unused_image_gets_one_idle_use() {
        let u = docker_uses(
            1,
            "",
            &[image("sha256:i", "redis:7", Some("sha256:d"))],
            &[],
        );
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].key.as_str(), "sha256:d");
        assert_eq!(u[0].image_ref, "redis:7");
        assert!(u[0].container.is_empty() && !u[0].running);
    }

    #[test]
    fn two_containers_two_uses_one_running() {
        let u = docker_uses(
            1,
            "",
            &[image("sha256:i", "nginx:1.25", Some("sha256:d"))],
            &[
                container("a", "sha256:i", "running"),
                container("b", "sha256:i", "exited"),
            ],
        );
        assert_eq!(u.len(), 2, "no idle use for an image with containers");
        assert_eq!(u.iter().filter(|u| u.running).count(), 1);
    }

    #[test]
    fn scan_containers_are_ignored() {
        let mut scanner = container("fleetops-scan-abc-1", "sha256:t", "running");
        scanner.labels.insert(SCAN_LABEL.into(), "1".into());
        let mut trivy = image("sha256:t", "aquasec/trivy:0.76.1", Some("sha256:tt"));
        trivy.1["Config"] = json!({"Labels": {SCAN_LABEL: "1"}});
        let u = docker_uses(
            1,
            "",
            &[trivy, image("sha256:i", "app:1", None)],
            &[scanner],
        );
        assert_eq!(u.len(), 1, "{u:?}");
        assert_eq!(u[0].key.as_str(), "local:1::sha256:i");
    }

    fn kuse(ns: &str, owner: &str, image: &str, id: Option<&str>) -> KubeImageUse {
        KubeImageUse {
            namespace: ns.into(),
            owner_kind: "Deployment".into(),
            owner_name: owner.into(),
            container: "app".into(),
            image: image.into(),
            image_id: id.map(Into::into),
            running: id.is_some(),
            pull_secrets: vec![],
        }
    }

    #[test]
    fn kube_template_shares_key_with_pod_of_same_image() {
        let u = kube_uses(
            7,
            vec![
                kuse("a", "web", "nginx:1.25", Some("sha256:d")),
                kuse("b", "idle", "nginx:1.25", None),
                kuse("b", "never", "busybox:1.37", None),
            ],
        );
        assert_eq!(u[1].key.as_str(), "sha256:d");
        assert_eq!(u[2].key.as_str(), "ref:7:busybox:1.37");
        assert_eq!(u[0].workload, "Deployment/web");
    }

    #[test]
    fn new_keys_detects_moved_tag() {
        let before = docker_uses(
            1,
            "",
            &[image("sha256:i", "app:latest", Some("sha256:old"))],
            &[],
        );
        let after = docker_uses(
            1,
            "",
            &[image("sha256:j", "app:latest", Some("sha256:new"))],
            &[],
        );
        assert_eq!(
            new_keys(&before, &after),
            vec![ImageKey("sha256:new".into())]
        );
        assert!(new_keys(&after, &after).is_empty());
    }
}
