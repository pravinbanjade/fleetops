use fleetops_core::compose::docker::*;
use fleetops_core::compose::plan::*;
use fleetops_core::compose::*;
use fleetops_core::docker::{ContainerSummary, NetworkSummary, VolumeSummary};
use std::collections::BTreeMap;

fn project(yaml: &str) -> Project {
    parse(yaml, &[], "shop").expect("valid").project
}

fn container(id: &str, svc: &str, hash: Option<&str>) -> ContainerSummary {
    let mut labels = BTreeMap::from([
        (LABEL_PROJECT.to_string(), "shop".to_string()),
        (LABEL_SERVICE.to_string(), svc.to_string()),
    ]);
    if let Some(h) = hash {
        labels.insert(LABEL_HASH.into(), h.into());
    }
    ContainerSummary {
        id: id.into(),
        name: format!("shop-{svc}-{id}"),
        image: "x".into(),
        image_id: "sha".into(),
        state: "running".into(),
        status: String::new(),
        created: 0,
        ports: vec![],
        labels,
        compose_project: Some("shop".into()),
        compose_service: Some(svc.into()),
        volumes: vec![],
    }
}

fn network(name: &str, hash: Option<&str>) -> NetworkSummary {
    let mut labels = BTreeMap::from([(LABEL_PROJECT.to_string(), "shop".to_string())]);
    if let Some(h) = hash {
        labels.insert(LABEL_HASH.into(), h.into());
    }
    NetworkSummary {
        id: name.into(),
        name: name.into(),
        driver: "bridge".into(),
        scope: "local".into(),
        internal: false,
        attachable: false,
        subnet: None,
        gateway: None,
        labels,
    }
}

fn volume(name: &str) -> VolumeSummary {
    VolumeSummary {
        name: name.into(),
        driver: "local".into(),
        mountpoint: String::new(),
        created_at: None,
        labels: BTreeMap::from([(LABEL_PROJECT.to_string(), "shop".to_string())]),
        in_use: false,
    }
}

fn action(p: &Plan, svc: &str) -> ServiceAction {
    p.services.iter().find(|s| s.name == svc).unwrap().action
}

fn reasons<'a>(p: &'a Plan, svc: &str) -> &'a [String] {
    &p.services.iter().find(|s| s.name == svc).unwrap().reasons
}

/// The live state a successful deploy of `p` leaves.
fn deployed_live(p: &Project) -> Live {
    let mut live = Live::default();
    for (name, svc) in &p.services {
        for n in 1..=svc.replicas {
            live.containers.push(container(
                &format!("{name}{n}"),
                name,
                Some(&service_hash(svc)),
            ));
        }
    }
    for net in p.networks.values() {
        live.networks
            .push(network(&net.name, Some(&network_hash(net))));
    }
    live
}

const BASE: &str = "services:\n  web:\n    image: nginx:1.26\n    environment: {LOG_LEVEL: info}\n";

#[test]
fn fresh_stack_creates_everything() {
    let p = project("services:\n  web: {image: x, volumes: ['data:/d']}\nvolumes:\n  data:\n");
    let plan = plan(&p, None, &Live::default());
    assert_eq!(action(&plan, "web"), ServiceAction::Create);
    assert_eq!(plan.networks[0].action, ResourceAction::Create);
    assert_eq!(plan.volumes[0].action, ResourceAction::Create);
    assert_eq!(
        plan.images,
        vec![ImagePlan {
            image: "x".into(),
            pull: true
        }]
    );
    assert!(plan.blocking.is_empty());
}

#[test]
fn unchanged_when_hashes_match() {
    let p = project(BASE);
    let plan = plan(&p, Some((&p, 1)), &deployed_live(&p));
    assert_eq!(action(&plan, "web"), ServiceAction::Unchanged);
    assert!(plan.images.is_empty());
}

#[test]
fn image_change_reason() {
    let old = project(BASE);
    let new = project(&BASE.replace("1.26", "1.27"));
    let plan = plan(&new, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(action(&plan, "web"), ServiceAction::Recreate);
    assert_eq!(reasons(&plan, "web"), ["image: nginx:1.26 → nginx:1.27"]);
}

#[test]
fn env_change_reason_hides_values() {
    let old = project(BASE);
    let new = project(&BASE.replace("info", "debug"));
    let plan = plan(&new, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(reasons(&plan, "web"), ["environment.LOG_LEVEL changed"]);
    assert!(!reasons(&plan, "web")[0].contains("debug"));
}

#[test]
fn scale_up_and_down() {
    let old = project(BASE);
    let up = project(&format!("{BASE}    deploy: {{replicas: 3}}\n"));
    let p = plan(&up, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(action(&p, "web"), ServiceAction::Scale);
    assert_eq!(
        (p.services[0].replicas_before, p.services[0].replicas_after),
        (1, 3)
    );
    let down = plan(&old, Some((&up, 2)), &deployed_live(&up));
    assert_eq!(action(&down, "web"), ServiceAction::Scale);
    assert_eq!(down.services[0].replicas_after, 1);
}

#[test]
fn orphan_removed() {
    let old = project(&format!("{BASE}  worker:\n    image: w\n"));
    let new = project(BASE);
    let p = plan(&new, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(action(&p, "worker"), ServiceAction::Remove);
}

#[test]
fn volume_kept_not_removed() {
    let new = project(BASE);
    let mut live = deployed_live(&new);
    live.volumes.push(volume("shop_olddata"));
    let p = plan(&new, Some((&new, 1)), &live);
    assert_eq!(
        p.volumes,
        vec![ResourcePlan {
            name: "shop_olddata".into(),
            action: ResourceAction::Kept
        }]
    );
}

#[test]
fn changed_network_blocks() {
    let p = project(BASE);
    let mut live = deployed_live(&p);
    live.networks[0]
        .labels
        .insert(LABEL_HASH.into(), "different".into());
    let plan = plan(&p, Some((&p, 1)), &live);
    assert_eq!(
        plan.blocking,
        ["network shop_default changed; remove the stack (keeping volumes) and deploy again"]
    );
}

#[test]
fn pull_rules() {
    let mk = |policy: &str| {
        project(&format!(
            "services:\n  web: {{image: nginx:1.27, pull_policy: {policy}}}\n"
        ))
    };
    let present = Live {
        images: vec!["nginx:1.27".into()],
        ..Default::default()
    };
    assert!(plan(&mk("always"), None, &present).images[0].pull);
    assert!(!plan(&mk("missing"), None, &present).images[0].pull);
    assert!(plan(&mk("missing"), None, &Live::default()).images[0].pull);
    assert!(!plan(&mk("never"), None, &present).images[0].pull);
    assert_eq!(
        plan(&mk("never"), None, &Live::default()).blocking,
        ["image nginx:1.27 is not present and pull_policy is never"]
    );
}

#[test]
fn token_changes_with_live_state() {
    let p = project(BASE);
    let live = deployed_live(&p);
    let a = plan(&p, Some((&p, 1)), &live);
    let b = plan(&p, Some((&p, 1)), &live);
    assert_eq!(a.token, b.token);
    let mut changed = live.clone();
    changed.containers[0].id = "other".into();
    assert_ne!(a.token, plan(&p, Some((&p, 1)), &changed).token);
    assert_ne!(a.token, plan(&p, Some((&p, 2)), &live).token);
}

#[test]
fn cli_project_is_recreated() {
    let p = project(&format!("{BASE}  db:\n    image: postgres\n"));
    let live = Live {
        containers: vec![container("a", "web", None), container("b", "db", None)],
        ..Default::default()
    };
    let plan = plan(&p, None, &live);
    for s in ["web", "db"] {
        assert_eq!(action(&plan, s), ServiceAction::Recreate);
        assert_eq!(reasons(&plan, s), ["not created by FleetOps"]);
    }
}
