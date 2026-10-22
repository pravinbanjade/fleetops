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
            pull: true,
            build: false,
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

#[test]
fn network_mode_dependents_recreate_with_target() {
    let yaml = |tag: &str| {
        format!("services:\n  vpn: {{image: 'v:{tag}'}}\n  app: {{image: a, network_mode: 'service:vpn'}}\n")
    };
    let old = project(&yaml("1"));
    let new = project(&yaml("2"));
    let plan = plan(&new, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(action(&plan, "vpn"), ServiceAction::Recreate);
    assert_eq!(action(&plan, "app"), ServiceAction::Recreate);
    assert_eq!(
        reasons(&plan, "app"),
        ["vpn is recreated, and this service shares its network"]
    );
}

#[test]
fn file_reasons_name_the_item() {
    let yaml = |extra: &str| {
        project(&format!(
            "services:\n  db:\n    image: x\n    secrets: [{extra}]\nsecrets:\n  db_password: {{ file: ./pw }}\n  other: {{ file: ./o }}\n"
        ))
    };
    let with_digest = |mut p: Project, d: &str| {
        for f in &mut p.services.get_mut("db").unwrap().files {
            f.digest = d.to_string();
        }
        p
    };
    let a = with_digest(yaml("db_password"), "aa");
    let b = with_digest(yaml("db_password"), "bb");
    assert_eq!(
        service_reasons(&a.services["db"], &b.services["db"]),
        vec!["secret db_password changed"]
    );
    let none = project("services:\n  db: { image: x }\n");
    assert_eq!(
        service_reasons(&none.services["db"], &a.services["db"]),
        vec!["secret db_password added"]
    );
    assert_eq!(
        service_reasons(&a.services["db"], &none.services["db"]),
        vec!["secret db_password removed"]
    );
    let mode = with_digest(yaml("{ source: db_password, mode: 0400 }"), "aa");
    assert_eq!(
        service_reasons(&a.services["db"], &mode.services["db"]),
        vec!["secret db_password changed"]
    );
}

fn built(yaml: &str, tree: &str) -> Project {
    let mut p = project(yaml);
    let trees: fleetops_core::compose::build::Trees = p
        .services
        .iter()
        .filter(|(_, s)| s.build.is_some())
        .map(|(n, _)| (n.clone(), tree.to_string()))
        .collect();
    fleetops_core::compose::build::apply(&mut p, &trees, true, &[1; 32]).unwrap();
    p
}

const BUILD_YAML: &str = "services:\n  api:\n    build: ./api\n  db:\n    image: postgres\n";

#[test]
fn build_planned_only_when_missing_and_recreated() {
    let p = built(BUILD_YAML, "t1");
    let tag = p.services["api"].image.clone();
    let fresh = plan(&p, None, &Live::default());
    let img = fresh.images.iter().find(|i| i.image == tag).unwrap();
    assert!(img.build && !img.pull, "{img:?}");
    let mut live = Live::default();
    live.images.push(tag.clone());
    let present = plan(&p, None, &live);
    let img = present.images.iter().find(|i| i.image == tag).unwrap();
    assert!(!img.build && !img.pull, "{img:?}");
    let mut same = deployed_live(&p);
    same.images.push(tag.clone());
    let unchanged = plan(&p, Some((&p, 1)), &same);
    assert!(unchanged.images.iter().all(|i| i.image != tag));
}

#[test]
fn pull_policy_build_builds_a_recreated_service_only() {
    let yaml = "services:\n  api:\n    build: ./api\n    pull_policy: build\n";
    let p = built(yaml, "t1");
    let tag = p.services["api"].image.clone();
    let mut live = deployed_live(&p);
    live.images.push(tag.clone());
    let unchanged = plan(&p, Some((&p, 1)), &live);
    assert!(unchanged.images.is_empty(), "{:?}", unchanged.images);
    let mut fresh = Live::default();
    fresh.images.push(tag.clone());
    let created = plan(&p, None, &fresh);
    assert!(created.images.iter().any(|i| i.image == tag && i.build));
}

#[test]
fn build_change_reason() {
    let old = built(BUILD_YAML, "t1");
    let new = built(BUILD_YAML, "t2");
    let p = plan(&new, Some((&old, 1)), &deployed_live(&old));
    assert_eq!(action(&p, "api"), ServiceAction::Recreate);
    assert_eq!(reasons(&p, "api"), ["build changed"]);
    assert_eq!(action(&p, "db"), ServiceAction::Unchanged);
}
