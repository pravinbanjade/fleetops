use fleetops_core::compose::files::resolve;
use fleetops_core::compose::plan::{ResourceAction, ServiceAction};
use fleetops_core::compose::swarm::*;
use fleetops_core::compose::swarm_plan::*;
use fleetops_core::compose::*;
use fleetops_core::swarm::{ServiceDetail, ServiceSummary, SwarmNetwork};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const FILE: &str = r#"
services:
  web:
    image: nginx:1.27
    deploy: { replicas: 3 }
    secrets: [db]
  api:
    image: api:1
secrets:
  db: { environment: DB_PW }
"#;

fn project(yaml: &str, pw: &str) -> Project {
    let vars = vec![format!("DB_PW={pw}")];
    let mut p = parse_mode(yaml, &vars, "blog", Mode::Swarm)
        .unwrap()
        .project;
    resolve(&mut p, &vars, &Default::default(), &[7; 32]).unwrap();
    p
}

fn detail(spec: Value, version: u64) -> ServiceDetail {
    let name = spec["Name"].as_str().unwrap().to_string();
    ServiceDetail {
        summary: ServiceSummary {
            id: format!("id-{name}"),
            name,
            stack: Some("blog".into()),
            mode: "replicated".into(),
            replicas_desired: spec["Mode"]["Replicated"]["Replicas"].as_u64(),
            tasks_running: 0,
            tasks_desired: 0,
            image: String::new(),
            ports: vec![],
            update_state: None,
            updated_at: 0,
            labels: BTreeMap::new(),
        },
        spec,
        update_status: None,
        version,
    }
}

fn network(name: &str, stack: Option<&str>) -> SwarmNetwork {
    SwarmNetwork {
        id: format!("netid-{name}"),
        name: name.into(),
        driver: "overlay".into(),
        stack: stack.map(Into::into),
        attachable: false,
        internal: false,
        encrypted: false,
        subnets: vec![],
        used_by: vec![],
        labels: Default::default(),
    }
}

/// What `p` looks like once deployed: its services, objects and networks.
fn deployed(p: &Project) -> SwarmLive {
    let mut live = SwarmLive::default();
    for key in p.services.keys() {
        live.services.push(detail(service_spec(p, key), 10));
    }
    let secret = object_name(p, FileKind::Secret, "db");
    live.secrets.push(secret.clone());
    live.stack_secrets.push(secret);
    live.networks.push(network("blog_default", Some("blog")));
    live.network_hashes.insert(
        "blog_default".into(),
        fleetops_core::compose::docker::network_hash(&p.networks["default"]),
    );
    live
}

fn action(plan: &fleetops_core::compose::plan::Plan, name: &str) -> ServiceAction {
    plan.services
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .action
}

#[test]
fn new_services_are_created() {
    let p = project(FILE, "a");
    let plan = plan_swarm(&p, None, &SwarmLive::default());
    assert_eq!(action(&plan, "web"), ServiceAction::Create);
    assert_eq!(action(&plan, "api"), ServiceAction::Create);
    let web = plan.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!((web.replicas_before, web.replicas_after), (0, 3));
    assert_eq!(plan.networks[0].name, "blog_default");
    assert_eq!(plan.networks[0].action, ResourceAction::Create);
    assert_eq!(plan.secrets.len(), 1);
    assert_eq!(plan.secrets[0].action, ResourceAction::Create);
    assert!(plan.images.is_empty() && plan.volumes.is_empty());
    assert!(plan.blocking.is_empty(), "{:?}", plan.blocking);
}

#[test]
fn same_spec_is_unchanged() {
    let p = project(FILE, "a");
    let plan = plan_swarm(&p, Some((&p, 1)), &deployed(&p));
    assert_eq!(action(&plan, "web"), ServiceAction::Unchanged);
    assert_eq!(action(&plan, "api"), ServiceAction::Unchanged);
    assert_eq!(plan.secrets[0].action, ResourceAction::Exists);
    assert_eq!(plan.networks[0].action, ResourceAction::Exists);
}

#[test]
fn a_changed_file_updates_with_reasons() {
    let old = project(FILE, "a");
    let new = project(&FILE.replace("nginx:1.27", "nginx:1.28"), "a");
    let plan = plan_swarm(&new, Some((&old, 1)), &deployed(&old));
    let web = plan.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.action, ServiceAction::Update);
    assert_eq!(web.reasons, vec!["image: nginx:1.27 → nginx:1.28"]);
    assert!(web.drift.is_empty());
    assert_eq!(action(&plan, "api"), ServiceAction::Unchanged);
}

#[test]
fn drift_forces_an_update_and_names_the_fields() {
    let p = project(FILE, "a");
    let mut live = deployed(&p);
    let web = live
        .services
        .iter_mut()
        .find(|s| s.summary.name == "blog_web")
        .unwrap();
    web.spec["Mode"]["Replicated"]["Replicas"] = json!(5);
    let plan = plan_swarm(&p, Some((&p, 1)), &live);
    let web = plan.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.action, ServiceAction::Update);
    assert_eq!(web.drift, vec!["replicas: 3 → 5"]);
    assert_eq!(web.reasons, vec!["replicas: 5 → 3"]);
}

#[test]
fn drift_without_a_deployed_revision_is_named_plainly() {
    let p = project(FILE, "a");
    let mut live = deployed(&p);
    live.services[0].spec["Mode"]["Replicated"]["Replicas"] = json!(9);
    let plan = plan_swarm(&p, None, &live);
    let changed = plan.services.iter().find(|s| !s.drift.is_empty()).unwrap();
    assert_eq!(changed.drift, vec!["changed outside the stack"]);
}

#[test]
fn cli_deployed_services_are_updated_in_place() {
    let p = project(FILE, "a");
    let mut live = deployed(&p);
    for s in &mut live.services {
        s.spec["Labels"]
            .as_object_mut()
            .unwrap()
            .remove(LABEL_SPEC_HASH);
    }
    let plan = plan_swarm(&p, None, &live);
    let web = plan.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.action, ServiceAction::Update);
    assert_eq!(web.reasons, vec!["not deployed by FleetOps"]);
    assert_eq!(plan.services.len(), 2, "no duplicates: {:?}", plan.services);
}

#[test]
fn services_that_left_the_file_are_removed() {
    let old = project(FILE, "a");
    let new = project(
        "services:\n  web:\n    image: nginx:1.27\n    deploy: { replicas: 3 }\n    secrets: [db]\nsecrets:\n  db: { environment: DB_PW }\n",
        "a",
    );
    let plan = plan_swarm(&new, Some((&old, 1)), &deployed(&old));
    let api = plan.services.iter().find(|s| s.name == "api").unwrap();
    assert_eq!(api.action, ServiceAction::Remove);
    assert_eq!(api.reasons, vec!["no longer in the file"]);
}

#[test]
fn missing_external_objects_block() {
    let p = project(
        r#"
services:
  web:
    image: x
    networks: [shared]
    secrets: [tls]
    configs: [site]
networks: { shared: { external: true } }
secrets: { tls: { external: true, name: shared_tls } }
configs: { site: { external: true } }
"#,
        "a",
    );
    let plan = plan_swarm(&p, None, &SwarmLive::default());
    assert_eq!(
        plan.blocking,
        vec![
            "external network shared does not exist",
            "external secret shared_tls does not exist",
            "external config site does not exist",
        ]
    );
    assert!(plan.secrets.is_empty() && plan.configs.is_empty());
    let mut live = SwarmLive::default();
    live.networks.push(network("shared", None));
    live.secrets.push("shared_tls".into());
    live.configs.push("site".into());
    assert!(plan_swarm(&p, None, &live).blocking.is_empty());
}

#[test]
fn a_changed_network_blocks() {
    let p = project(FILE, "a");
    let mut live = deployed(&p);
    live.network_hashes
        .insert("blog_default".into(), "other".into());
    let plan = plan_swarm(&p, Some((&p, 1)), &live);
    assert!(
        plan.blocking
            .iter()
            .any(|b| b.starts_with("network blog_default changed")),
        "{:?}",
        plan.blocking
    );
}

#[test]
fn object_plans() {
    let old = project(FILE, "a");
    let new = project(FILE, "b");
    let live = deployed(&old);
    let plan = plan_swarm(&new, Some((&old, 1)), &live);
    let new_name = object_name(&new, FileKind::Secret, "db");
    let old_name = object_name(&old, FileKind::Secret, "db");
    assert_ne!(new_name, old_name);
    let by = |n: &str| plan.secrets.iter().find(|r| r.name == n).map(|r| r.action);
    assert_eq!(by(&new_name), Some(ResourceAction::Create));
    assert_eq!(by(&old_name), Some(ResourceAction::Remove));
    let web = plan.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.action, ServiceAction::Update);
    assert_eq!(
        web.reasons,
        vec!["TaskTemplate.ContainerSpec.Secrets changed"]
    );
}

#[test]
fn the_token_changes_with_the_live_version() {
    let p = project(FILE, "a");
    let live = deployed(&p);
    let a = plan_swarm(&p, Some((&p, 1)), &live).token;
    let mut later = live.clone();
    later.services[0].version = 11;
    assert_ne!(a, plan_swarm(&p, Some((&p, 1)), &later).token);
    assert_eq!(a, plan_swarm(&p, Some((&p, 1)), &live).token);
}

#[test]
fn a_network_of_another_stack_must_be_declared_external() {
    let p = project(
        "services:\n  web:\n    image: x\n    networks: [backend]\nnetworks:\n  backend: { name: shop_backend }\n",
        "a",
    );
    for owner in [Some("shop"), None] {
        let mut live = SwarmLive::default();
        live.networks.push(network("shop_backend", owner));
        let plan = plan_swarm(&p, None, &live);
        assert_eq!(
            plan.blocking,
            vec!["network shop_backend already exists outside this stack; declare it external"],
            "{owner:?}"
        );
    }
}

const HOST: &str = r#"
services:
  web:
    image: nginx:1.27
    networks: [hostnet]
networks:
  hostnet: { external: true, name: host }
"#;

/// Docker's predefined networks are on every node and in no stack: like
/// `docker stack deploy`, a plan neither looks them up nor scopes them, and
/// gives them no aliases.
#[test]
fn the_host_network_needs_no_lookup() {
    let p = project(HOST, "x");
    let mut plan = plan_swarm(&p, None, &SwarmLive::default());
    apply_scope(&mut plan, &p, &SwarmLive::default(), &Default::default());
    assert!(plan.blocking.is_empty(), "{:?}", plan.blocking);
    let spec = service_spec(&p, "web");
    assert_eq!(
        spec["TaskTemplate"]["Networks"],
        json!([{"Target": "host"}])
    );
}

/// Docker stores the id of the swarm's own `host` network, which its
/// network list lacks: read back by name, the spec is unchanged.
#[test]
fn a_service_on_the_host_network_is_unchanged() {
    let p = project(HOST, "x");
    let mut stored = service_spec(&p, "web");
    stored["TaskTemplate"]["Networks"][0]["Target"] = json!("hostid");
    let mut live = SwarmLive::default();
    live.services.push(detail(stored, 10));
    live.other_networks.insert("hostid".into(), "host".into());
    let plan = plan_swarm(&p, Some((&p, 1)), &live);
    assert_eq!(action(&plan, "web"), ServiceAction::Unchanged);
}
