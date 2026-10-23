//! The shipped FleetOps-in-Swarm stack and the agents' inside variant must
//! agree on the network, the service name and the HTTPS port.

use fleetops_api::agents::swarm_manifest::{INSIDE_NETWORK, INSIDE_URL};
use serde_json::Value;

#[test]
fn shipped_swarm_stack_matches_the_inside_variant() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/swarm/fleetops.yml"
    );
    let text = std::fs::read_to_string(path).unwrap();
    let v: Value = serde_saphyr::from_str(&text).unwrap();

    let net = &v["networks"][INSIDE_NETWORK];
    assert_eq!(
        net["name"], INSIDE_NETWORK,
        "not prefixed by the stack name"
    );
    assert_eq!(net["driver"], "overlay");
    assert_eq!(net["attachable"], true);

    // https://<service>:<port>
    let (host, port) = INSIDE_URL
        .strip_prefix("https://")
        .unwrap()
        .split_once(':')
        .unwrap();
    let svc = &v["services"][host];
    assert!(svc.is_object(), "service {host} in {path}");
    assert_eq!(
        svc["environment"]["FLEETOPS_TLS_BIND"],
        format!("0.0.0.0:{port}")
    );
    assert_eq!(svc["networks"][0], INSIDE_NETWORK);
    assert_eq!(svc["deploy"]["replicas"], 1);
}
