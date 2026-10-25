mod common;

use common::{
    fake_swarm::{connected_swarm as swarm, with_custom, Swarm},
    *,
};
use fleetops_core::perm::{Row, Verb};
use fleetops_store::AuditEntry;
use serde_json::{json, Value};

async fn send(sw: &Swarm, method: &str, path: &str, body: Option<Value>) -> Resp {
    sw.t.send(req(method, path, body, Some(&sw.admin))).await
}

/// The newest audit row with `action`.
async fn audit(sw: &Swarm, action: &str) -> AuditEntry {
    sw.t.store
        .list_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.action == action)
        .unwrap_or_else(|| panic!("no {action} audit row"))
}

const N: &str = "/api/v1/envs/prod/swarm/nodes";

#[tokio::test]
async fn node_availability_labels_and_role_update() {
    let sw = swarm().await;
    let r = send(
        &sw,
        "PATCH",
        &format!("{N}/n2"),
        Some(json!({"availability": "pause", "labels": {"zone": "a"}, "role": "manager"})),
    )
    .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    let paths = sw.daemon.write_paths();
    assert!(
        paths.iter().any(|p| p.contains("/nodes/n2/update") && p.contains("version=5")),
        "{paths:?}"
    );
    let body = sw.daemon.body_of("/nodes/n2/update");
    assert_eq!(body["Availability"], "pause");
    assert_eq!(body["Role"], "manager");
    assert_eq!(body["Labels"], json!({"zone": "a"}));
    let a = audit(&sw, "swarm.node.update").await;
    assert_eq!(a.target_id.as_deref(), Some("n2"));
    assert_eq!(a.target_type.as_deref(), Some("swarm-node"));
}

#[tokio::test]
async fn removing_an_up_node_or_a_manager_is_refused() {
    let sw = swarm().await;
    let r = send(&sw, "DELETE", &format!("{N}/n2"), None).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(r.json["message"].as_str().unwrap().ends_with(
        "node 'host-n2' is up; run 'docker swarm leave' on it, or wait until it is down, before removing it"
    ), "{:?}", r.json);
    sw.daemon.set_state("n1", "down");
    let r = send(&sw, "DELETE", &format!("{N}/n1"), None).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .ends_with("demote node 'host-n1' before removing it"));
    assert!(sw.daemon.write_paths().is_empty());
}

#[tokio::test]
async fn removing_a_down_worker_works() {
    let sw = swarm().await;
    sw.daemon.set_state("n2", "down");
    let r = send(&sw, "DELETE", &format!("{N}/n2"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.starts_with("DELETE") && p.contains("/nodes/n2")));
    assert_eq!(
        audit(&sw, "swarm.node.remove").await.target_id.as_deref(),
        Some("n2")
    );
}

#[tokio::test]
async fn draining_fleetops_node_needs_its_name() {
    let sw = swarm().await;
    sw.daemon.add_fleetops_server("n1");
    let r = send(&sw, "PATCH", &format!("{N}/n1"), Some(json!({"availability": "drain"}))).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert_eq!(r.json["code"], "confirm_required");
    assert_eq!(
        r.json["message"],
        "FleetOps runs on node 'host-n1'; this can stop FleetOps. Type 'host-n1' to confirm."
    );
    assert!(sw.daemon.write_paths().is_empty());
    let r = send(
        &sw,
        "PATCH",
        &format!("{N}/n1"),
        Some(json!({"availability": "drain", "confirm": "host-n1"})),
    )
    .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert_eq!(
        audit(&sw, "swarm.node.update").await.detail.unwrap()["confirmed"],
        true
    );
    // Pausing stops nothing that runs.
    let r = send(&sw, "PATCH", &format!("{N}/n1"), Some(json!({"availability": "pause"}))).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
}

#[tokio::test]
async fn node_writes_need_their_cells() {
    let (sw, bob) = with_custom(&[(Row::SwarmNodes, Verb::View)]).await;
    let r =
        sw.t.send(req(
            "PATCH",
            &format!("{N}/n2"),
            Some(json!({"availability": "pause"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    let r = sw.t.send(req("DELETE", &format!("{N}/n2"), None, Some(&bob))).await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}
