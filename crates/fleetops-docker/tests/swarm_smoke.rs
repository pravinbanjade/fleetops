//! A Swarm stack's spec against a real manager: what FleetOps renders must
//! read back without drift once Docker has stored it. Needs a swarm on
//! DOCKER_HOST (`docker swarm init`); run with
//! `cargo test -p fleetops-docker --features docker-it --test swarm_smoke -- --ignored`.
#![cfg(feature = "docker-it")]

use fleetops_core::compose::files::resolve;
use fleetops_core::compose::swarm::{
    canonical, named, object_name, sample_variables, service_spec, spec_changes, spec_hash,
    LABEL_SPEC_HASH, SAMPLE_STACK,
};
use fleetops_core::compose::{parse_mode, FileKind, Mode};
use fleetops_core::swarm::{NewSwarmNetwork, SwarmOps, STACK_LABEL};
use fleetops_docker::LocalDocker;
use std::collections::BTreeMap;

fn host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into())
}

#[tokio::test]
#[ignore = "needs a Swarm manager on DOCKER_HOST"]
async fn a_deployed_spec_reads_back_without_drift() {
    let d = LocalDocker::connect(&host()).unwrap();
    let stack = format!("fosmoke{}", std::process::id());
    let vars = sample_variables();
    let mut p = parse_mode(SAMPLE_STACK, &vars, &stack, Mode::Swarm)
        .unwrap()
        .project;
    let values = resolve(&mut p, &vars, &Default::default(), &[7; 32]).unwrap();
    let ours = BTreeMap::from([(STACK_LABEL.to_string(), stack.clone())]);

    let net = &p.networks["front"];
    let net_id = d
        .create_network(
            &NewSwarmNetwork {
                name: net.name.clone(),
                attachable: net.attachable,
                internal: false,
                encrypted: false,
                subnet: None,
            },
            &ours,
            &Default::default(),
        )
        .await
        .unwrap();
    let secret = object_name(&p, FileKind::Secret, "db");
    let secret_id = d
        .create_secret(
            &secret,
            values[&(FileKind::Secret, "db".into())].as_bytes(),
            &ours,
        )
        .await
        .unwrap();
    let config = object_name(&p, FileKind::Config, "site");
    let config_id = d
        .create_config(
            &config,
            values[&(FileKind::Config, "site".into())].as_bytes(),
            &ours,
        )
        .await
        .unwrap();

    let spec = service_spec(&p, "web");
    let mut sent = spec.clone();
    sent["TaskTemplate"]["ContainerSpec"]["Secrets"][0]["SecretID"] = secret_id.clone().into();
    sent["TaskTemplate"]["ContainerSpec"]["Configs"][0]["ConfigID"] = config_id.clone().into();
    let created = d.create_service(&sent).await;
    let read = match &created {
        Ok(id) => d.service(id).await,
        Err(_) => Ok(None),
    };
    let names: BTreeMap<String, String> = d
        .networks()
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.id, n.name))
        .collect();

    // Clean up before asserting, so a failure leaves nothing behind.
    if let Ok(id) = &created {
        let _ = d.remove_service(id).await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let _ = d.remove_secret(&secret_id).await;
    let _ = d.remove_config(&config_id).await;
    let _ = d.remove_network(&net_id).await;

    created.expect("the service is created");
    let live = read.unwrap().expect("the service reads back");
    let live_spec = named(&live.spec, &names);
    let changes = spec_changes(&canonical(&spec), &canonical(&live_spec));
    assert!(
        changes.is_empty(),
        "{changes:?}\nsent: {}\nread: {}",
        canonical(&spec),
        canonical(&live_spec)
    );
    assert_eq!(
        spec_hash(&live_spec),
        spec["Labels"][LABEL_SPEC_HASH].as_str().unwrap()
    );
}

/// What a stack says little about: Docker fills in the rest, and none of it
/// may read as drift.
#[tokio::test]
#[ignore = "needs a Swarm manager on DOCKER_HOST"]
async fn partial_deploy_blocks_read_back_without_drift() {
    let d = LocalDocker::connect(&host()).unwrap();
    let stack = format!("fopartial{}", std::process::id());
    let yaml = r#"
services:
  health:
    image: nginx:1.27
    healthcheck: { test: ["CMD", "true"], interval: 10s }
  policies:
    image: nginx:1.27
    deploy:
      restart_policy: { condition: on-failure }
      update_config: { parallelism: 1 }
      rollback_config: { delay: 5s }
  everywhere:
    image: nginx:1.27
    deploy: { mode: global }
  once:
    image: alpine:3.20
    command: ["true"]
    deploy: { mode: replicated-job }
  plain:
    image: nginx:1.27
    ports: ["18080:80"]
"#;
    let mut p = parse_mode(yaml, &[], &stack, Mode::Swarm).unwrap().project;
    resolve(&mut p, &[], &Default::default(), &[7; 32]).unwrap();
    let net = &p.networks["default"];
    let net_id = d
        .create_network(
            &NewSwarmNetwork {
                name: net.name.clone(),
                attachable: false,
                internal: false,
                encrypted: false,
                subnet: None,
            },
            &BTreeMap::from([(STACK_LABEL.to_string(), stack.clone())]),
            &Default::default(),
        )
        .await
        .unwrap();
    let mut failures = vec![];
    let mut created = vec![];
    for key in p.services.keys() {
        let spec = service_spec(&p, key);
        let id = match d.create_service(&spec).await {
            Ok(id) => id,
            Err(e) => {
                failures.push(format!("{key}: create: {e}"));
                continue;
            }
        };
        created.push(id.clone());
        let names: BTreeMap<String, String> = d
            .networks()
            .await
            .unwrap()
            .into_iter()
            .map(|n| (n.id, n.name))
            .collect();
        let live = named(&d.service(&id).await.unwrap().unwrap().spec, &names);
        let changes = spec_changes(&canonical(&spec), &canonical(&live));
        if !changes.is_empty() {
            failures.push(format!(
                "{key}: {changes:?}\nsent: {}\nread: {}",
                canonical(&spec),
                canonical(&live)
            ));
        }
    }
    for id in &created {
        let _ = d.remove_service(id).await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let _ = d.remove_network(&net_id).await;
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A service on the swarm's own `host` network: Docker stores an id its
/// network list lacks, which `network_name` must name back.
#[tokio::test]
#[ignore = "needs a Swarm manager on DOCKER_HOST"]
async fn a_host_network_service_reads_back_without_drift() {
    let d = LocalDocker::connect(&host()).unwrap();
    let stack = format!("fohost{}", std::process::id());
    let yaml = r#"
services:
  web:
    image: nginx:1.27
    networks: [hostnet]
networks:
  hostnet: { external: true, name: host }
"#;
    let mut p = parse_mode(yaml, &[], &stack, Mode::Swarm).unwrap().project;
    resolve(&mut p, &[], &Default::default(), &[7; 32]).unwrap();
    let spec = service_spec(&p, "web");
    let created = d.create_service(&spec).await;
    let read = match &created {
        Ok(id) => d.service(id).await,
        Err(_) => Ok(None),
    };
    let mut names: BTreeMap<String, String> = d
        .networks()
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.id, n.name))
        .collect();
    if let Ok(Some(live)) = &read {
        for n in live.spec["TaskTemplate"]["Networks"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let id = n["Target"].as_str().unwrap_or_default();
            if !names.contains_key(id) {
                if let Some(name) = d.network_name(id).await.unwrap() {
                    names.insert(id.to_string(), name);
                }
            }
        }
    }
    if let Ok(id) = &created {
        let _ = d.remove_service(id).await;
    }
    created.expect("the service is created");
    let live = named(&read.unwrap().expect("the service reads back").spec, &names);
    let changes = spec_changes(&canonical(&spec), &canonical(&live));
    assert!(
        changes.is_empty(),
        "{changes:?}\nsent: {}\nread: {}",
        canonical(&spec),
        canonical(&live)
    );
}
