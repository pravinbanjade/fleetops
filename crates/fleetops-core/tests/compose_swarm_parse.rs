use fleetops_core::compose::docker::service_hash;
use fleetops_core::compose::files::resolve;
use fleetops_core::compose::*;
use std::collections::BTreeMap;

const SAMPLE: &str = r#"
services:
  web:
    image: nginx:1.27
    command: ["nginx", "-g", "daemon off;"]
    environment: { A: "1" }
    ports: ["8080:80", { target: 443, published: 8443, mode: host }]
    volumes: ["data:/data"]
    deploy:
      replicas: 2
      resources: { limits: { memory: 64M, cpus: "0.5" } }
    secrets: [db]
    configs: [site]
volumes: { data: }
secrets: { db: { file: ./db.txt } }
configs: { site: { content: "hello" } }
"#;

fn swarm(yaml: &str) -> Parsed {
    match parse_mode(yaml, &[], "shop", Mode::Swarm) {
        Ok(p) => p,
        Err(e) => panic!("unexpected issues: {e:?}"),
    }
}

fn swarm_issues(yaml: &str) -> Vec<ComposeIssue> {
    parse_mode(yaml, &[], "shop", Mode::Swarm).expect_err("expected issues")
}

fn has(issues: &[ComposeIssue], path: &str, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.path == path && i.message.contains(needle))
}

#[test]
fn compose_mode_is_unchanged() {
    let p = parse(SAMPLE, &[], "shop").unwrap().project;
    assert_eq!(
        service_hash(&p.services["web"]),
        "f5e9c4abd24cb46f16cc28ae4261e8b5d6169bc593ab530dbb0a712e3bde8d10"
    );
    assert_eq!(p.services["web"].ports[1].mode, None);
    let i = parse(
        "services:\n  web:\n    image: x\n    deploy:\n      placement: { constraints: [a] }\n",
        &[],
        "shop",
    )
    .expect_err("issues");
    assert!(has(&i, "services.web.deploy.placement", "unknown"), "{i:?}");
}

#[test]
fn swarm_mode_reads_the_deploy_block() {
    let p = swarm(
        r#"
services:
  web:
    image: nginx:1.27
    deploy:
      mode: replicated
      replicas: 3
      placement:
        constraints: ["node.role == worker"]
        preferences: [{ spread: node.labels.zone }]
        max_replicas_per_node: 2
      resources:
        limits: { cpus: "0.5", memory: 128M, pids: 100 }
        reservations: { cpus: "0.25", memory: 64M }
      restart_policy: { condition: on-failure, delay: 5s, max_attempts: 3, window: 2m }
      update_config: { parallelism: 2, delay: 10s, failure_action: rollback, monitor: 30s, max_failure_ratio: 0.1, order: start-first }
      rollback_config: { parallelism: 1, failure_action: pause, order: stop-first }
      labels: { team: web }
      endpoint_mode: dnsrr
"#,
    );
    let s = &p.project.services["web"];
    assert_eq!(s.replicas, 3);
    assert_eq!(s.memory_bytes, Some(128 * 1024 * 1024));
    assert_eq!(s.nano_cpus, Some(500_000_000));
    let d = s.swarm.as_ref().expect("deploy block");
    assert_eq!(d.mode, SwarmMode::Replicated);
    assert_eq!(d.constraints, vec!["node.role == worker".to_string()]);
    assert_eq!(d.preferences, vec!["node.labels.zone".to_string()]);
    assert_eq!(d.max_replicas_per_node, Some(2));
    assert_eq!(d.pids_limit, Some(100));
    assert_eq!(d.reserve_nano_cpus, Some(250_000_000));
    assert_eq!(d.reserve_memory_bytes, Some(64 * 1024 * 1024));
    assert_eq!(
        d.restart_policy,
        Some(RestartPolicy {
            condition: Some("on-failure".into()),
            delay_secs: Some(5),
            max_attempts: Some(3),
            window_secs: Some(120),
        })
    );
    assert_eq!(
        d.update_config,
        Some(UpdatePolicy {
            parallelism: Some(2),
            delay_secs: Some(10),
            failure_action: Some("rollback".into()),
            monitor_secs: Some(30),
            max_failure_ratio: Some(0.1),
            order: Some("start-first".into()),
        })
    );
    assert_eq!(
        d.rollback_config
            .as_ref()
            .unwrap()
            .failure_action
            .as_deref(),
        Some("pause")
    );
    assert_eq!(d.labels, BTreeMap::from([("team".into(), "web".into())]));
    assert_eq!(d.endpoint_mode.as_deref(), Some("dnsrr"));
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
}

#[test]
fn every_service_has_a_deploy_block_in_swarm_mode() {
    let p = swarm("services:\n  web:\n    image: x\n");
    let d = p.project.services["web"]
        .swarm
        .as_ref()
        .expect("default deploy");
    assert_eq!(d.mode, SwarmMode::Replicated);
    assert_eq!(p.project.services["web"].replicas, 1);
}

#[test]
fn rollback_config_cannot_roll_back() {
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    deploy:\n      rollback_config: { failure_action: rollback }\n",
    );
    assert!(
        has(
            &i,
            "services.web.deploy.rollback_config.failure_action",
            "continue or pause"
        ),
        "{i:?}"
    );
}

#[test]
fn swarm_mode_refuses_build() {
    let i = swarm_issues("services:\n  web:\n    image: x\n    build: .\n");
    assert!(
        has(
            &i,
            "services.web.build",
            "Swarm cannot build images; push the image to a registry and name it in image:"
        ),
        "{i:?}"
    );
}

#[test]
fn swarm_mode_warns_and_ignores() {
    let p = swarm(
        r#"
services:
  web:
    image: x
    container_name: front
    restart: always
    mem_limit: 64m
    cpus: "1"
    depends_on: [db]
    links: [db]
    privileged: true
    ports: [{ target: 80, published: 80, host_ip: 127.0.0.1 }]
    networks: { default: { ipv4_address: 10.0.0.5 } }
  db:
    image: postgres
"#,
    );
    let w = &p.warnings;
    for (path, text) in [
        (
            "services.web.container_name",
            "'container_name' is ignored in a Swarm stack",
        ),
        (
            "services.web.restart",
            "'restart' is ignored in a Swarm stack: use deploy.restart_policy",
        ),
        (
            "services.web.mem_limit",
            "'mem_limit' is ignored in a Swarm stack: use deploy.resources.limits",
        ),
        (
            "services.web.cpus",
            "'cpus' is ignored in a Swarm stack: use deploy.resources.limits",
        ),
        (
            "services.web.depends_on",
            "'depends_on' is ignored in a Swarm stack",
        ),
        ("services.web.links", "'links' is ignored in a Swarm stack"),
        (
            "services.web.privileged",
            "'privileged' is ignored in a Swarm stack",
        ),
        (
            "services.web.ports[0].host_ip",
            "'host_ip' is ignored in a Swarm stack",
        ),
        (
            "services.web.networks.default.ipv4_address",
            "'ipv4_address' is ignored in a Swarm stack",
        ),
    ] {
        assert!(has(w, path, text), "{path}: {w:?}");
    }
    let s = &p.project.services["web"];
    assert_eq!(s.container_name, None);
    assert_eq!(s.restart, None);
    assert_eq!(s.memory_bytes, None);
    assert_eq!(s.nano_cpus, None);
    assert!(s.depends_on.is_empty());
    assert!(!s.privileged);
    assert_eq!(s.ports[0].host_ip, None);
    assert_eq!(s.networks["default"].ipv4_address, None);
}

#[test]
fn global_with_replicas_is_an_error() {
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    deploy:\n      mode: global\n      replicas: 2\n",
    );
    assert!(
        has(
            &i,
            "services.web.deploy.replicas",
            "replicas cannot be set with mode global"
        ),
        "{i:?}"
    );
}

#[test]
fn external_secrets_and_configs() {
    let p = swarm(
        r#"
services:
  web:
    image: x
    secrets: [tls]
    configs: [site]
secrets: { tls: { external: true, name: shared_tls } }
configs: { site: { external: true } }
"#,
    );
    assert_eq!(
        p.project.secrets["tls"],
        FileSource::External {
            name: "shared_tls".into()
        }
    );
    assert_eq!(
        p.project.configs["site"],
        FileSource::External {
            name: "site".into()
        }
    );
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    secrets: [tls]\nsecrets:\n  tls: { file: ./a, name: other }\n",
    );
    assert!(
        has(
            &i,
            "secrets.tls.name",
            "name is only supported with external: true; FleetOps names the stack's secrets and configs itself"
        ),
        "{i:?}"
    );
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    secrets: [tls]\nsecrets:\n  tls: { file: ./a, driver: vault }\n",
    );
    assert!(has(&i, "secrets.tls.driver", "not supported"), "{i:?}");
}

#[test]
fn external_sources_need_no_value() {
    let mut p = swarm(
        "services:\n  web:\n    image: x\n    secrets: [tls]\nsecrets:\n  tls: { external: true }\n",
    )
    .project;
    let values = resolve(&mut p, &[], &Default::default(), &[7; 32]).expect("resolves");
    assert!(values.is_empty());
    assert_eq!(p.services["web"].files[0].digest, "");
}

#[test]
fn swarm_networks_are_overlay() {
    let p = swarm("services:\n  web:\n    image: x\n");
    assert_eq!(p.project.networks["default"].driver, None);
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    networks: [front]\nnetworks:\n  front: { driver: bridge }\n",
    );
    assert!(
        has(
            &i,
            "networks.front.driver",
            "a Swarm stack's networks use the overlay driver"
        ),
        "{i:?}"
    );
    swarm(
        "services:\n  web:\n    image: x\n    networks: [front, hostnet]\nnetworks:\n  front: { driver: overlay }\n  hostnet: { external: true, name: host }\n",
    );
}

#[test]
fn port_mode_is_kept_in_swarm_mode_only() {
    let p = swarm(SAMPLE.replace("build", "x").as_str());
    assert_eq!(
        p.project.services["web"].ports[1].mode.as_deref(),
        Some("host")
    );
    assert_eq!(p.project.services["web"].ports[0].mode, None);
    let i = swarm_issues(
        "services:\n  web:\n    image: x\n    ports: [{ target: 80, mode: bridge }]\n",
    );
    assert!(
        has(&i, "services.web.ports[0].mode", "ingress or host"),
        "{i:?}"
    );
}
