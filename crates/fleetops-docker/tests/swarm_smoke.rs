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
        .create_secret(&secret, values[&(FileKind::Secret, "db".into())].as_bytes(), &ours)
        .await
        .unwrap();
    let config = object_name(&p, FileKind::Config, "site");
    let config_id = d
        .create_config(&config, values[&(FileKind::Config, "site".into())].as_bytes(), &ours)
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
