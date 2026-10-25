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
        paths
            .iter()
            .any(|p| p.contains("/nodes/n2/update") && p.contains("version=5")),
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
    let r = send(
        &sw,
        "PATCH",
        &format!("{N}/n1"),
        Some(json!({"availability": "drain"})),
    )
    .await;
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
    let r = send(
        &sw,
        "PATCH",
        &format!("{N}/n1"),
        Some(json!({"availability": "pause"})),
    )
    .await;
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
    let r =
        sw.t.send(req("DELETE", &format!("{N}/n2"), None, Some(&bob)))
            .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}

const S: &str = "/api/v1/envs/prod/swarm/services";

#[tokio::test]
async fn create_service_posts_the_spec() {
    let sw = swarm().await;
    let spec = json!({"Name": "api", "TaskTemplate": {"ContainerSpec": {"Image": "nginx:1.27"}},
                      "Mode": {"Replicated": {"Replicas": 2}}});
    let r = send(&sw, "POST", S, Some(json!({"spec": spec}))).await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(r.json["id"], "service-new");
    let body = sw.daemon.body_of("/services/create");
    assert_eq!(body["Name"], "api");
    assert_eq!(body["Mode"]["Replicated"]["Replicas"], 2);
    assert_eq!(body["TaskTemplate"]["ContainerSpec"]["Image"], "nginx:1.27");
    let a = audit(&sw, "swarm.service.create").await;
    assert_eq!(a.target_id.as_deref(), Some("api"));
    assert_eq!(a.target_type.as_deref(), Some("swarm-service"));
}

#[tokio::test]
async fn scale_updates_replicas_at_the_inspected_version() {
    let sw = swarm().await;
    let r = send(
        &sw,
        "POST",
        &format!("{S}/s1/scale"),
        Some(json!({"replicas": 5})),
    )
    .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/s1/update") && p.contains("version=10")));
    assert_eq!(
        sw.daemon.body_of("/services/s1/update")["Mode"]["Replicated"]["Replicas"],
        5
    );
    assert_eq!(
        audit(&sw, "swarm.service.scale").await.detail.unwrap()["replicas"],
        5
    );
    // The agent service is global, and protected.
    let r = send(
        &sw,
        "POST",
        &format!("{S}/s2/scale"),
        Some(json!({"replicas": 2})),
    )
    .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert_eq!(r.json["code"], "confirm_required");
    let r = send(
        &sw,
        "POST",
        &format!("{S}/s2/scale"),
        Some(json!({"replicas": 2, "confirm": "agent"})),
    )
    .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .ends_with("only replicated services scale; 'agent' is global"));
}

#[tokio::test]
async fn redeploy_and_rollback() {
    let sw = swarm().await;
    let r = send(&sw, "POST", &format!("{S}/s1/redeploy"), Some(json!({}))).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert_eq!(
        sw.daemon.body_of("/services/s1/update")["TaskTemplate"]["ForceUpdate"],
        1
    );
    let r = send(&sw, "POST", &format!("{S}/s1/rollback"), Some(json!({}))).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/s1/update") && p.contains("rollback=previous")));
    let actions = sw.t.audit_actions().await;
    for a in ["swarm.service.redeploy", "swarm.service.rollback"] {
        assert!(actions.contains(&a.to_string()), "{a}: {actions:?}");
    }
}

#[tokio::test]
async fn an_edit_without_reveal_keeps_real_env_values() {
    let (sw, bob) = with_custom(&[
        (Row::SwarmServices, Verb::View),
        (Row::SwarmServices, Verb::Operate),
    ])
    .await;
    let r =
        sw.t.send(req("GET", &format!("{S}/s1"), None, Some(&bob)))
            .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let spec = r.json["spec"].clone();
    assert!(!spec.to_string().contains("hunter2"));
    let r =
        sw.t.send(req(
            "PUT",
            &format!("{S}/s1"),
            Some(json!({"spec": spec, "version": r.json["version"]})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert_eq!(
        sw.daemon.body_of("/services/s1/update")["TaskTemplate"]["ContainerSpec"]["Env"],
        json!(["DB_PASSWORD=hunter2", "MODE=prod"])
    );
    assert!(!sw
        .t
        .store
        .list_audit(1000, None)
        .await
        .unwrap()
        .iter()
        .any(|a| a
            .detail
            .as_ref()
            .is_some_and(|d| d.to_string().contains("hunter2"))));
}

#[tokio::test]
async fn a_stale_edit_is_refused() {
    let sw = swarm().await;
    let spec = send(&sw, "GET", &format!("{S}/s1"), None).await.json["spec"].clone();
    let r = send(
        &sw,
        "PUT",
        &format!("{S}/s1"),
        Some(json!({"spec": spec, "version": 9})),
    )
    .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(
        r.json["message"]
            .as_str()
            .unwrap()
            .contains("update out of sequence"),
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn the_agent_service_is_protected_by_its_image() {
    let sw = swarm().await;
    let r = send(&sw, "DELETE", &format!("{S}/s2"), None).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert_eq!(r.json["code"], "confirm_required");
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .starts_with("'agent' runs the FleetOps agents"));
    assert!(sw.daemon.write_paths().is_empty());
    let r = send(&sw, "DELETE", &format!("{S}/s2?confirm=agent"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert_eq!(
        audit(&sw, "swarm.service.remove").await.detail.unwrap()["confirmed"],
        true
    );
}

#[tokio::test]
async fn fleetops_own_service_is_protected_by_its_label() {
    let sw = swarm().await;
    sw.daemon.add_fleetops_server("n1");
    let r = send(
        &sw,
        "POST",
        &format!("{S}/s3/scale"),
        Some(json!({"replicas": 0})),
    )
    .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert_eq!(
        r.json["message"],
        "'fleetops_fleetops' is FleetOps' own service; this can stop FleetOps. Type 'fleetops_fleetops' to confirm."
    );
    let r = send(
        &sw,
        "POST",
        &format!("{S}/s3/scale"),
        Some(json!({"replicas": 0, "confirm": "fleetops_fleetops"})),
    )
    .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
}

#[tokio::test]
async fn service_writes_need_their_cells() {
    let (sw, bob) = with_custom(&[(Row::SwarmServices, Verb::View)]).await;
    for (m, p, b) in [
        ("POST", S.to_string(), Some(json!({"spec": {"Name": "x"}}))),
        (
            "POST",
            format!("{S}/s1/scale"),
            Some(json!({"replicas": 1})),
        ),
        ("DELETE", format!("{S}/s1"), None),
    ] {
        let r = sw.t.send(req(m, &p, b, Some(&bob))).await;
        assert_eq!(r.status, 403, "{m} {p}: {:?}", r.json);
    }
}

#[tokio::test]
async fn demoting_or_relabelling_fleetops_node_needs_its_name() {
    let sw = swarm().await;
    sw.daemon.add_fleetops_server("n1");
    for change in [json!({"role": "worker"}), json!({"labels": {"zone": "a"}})] {
        let r = send(&sw, "PATCH", &format!("{N}/n1"), Some(change.clone())).await;
        assert_eq!(r.status, 409, "{change}: {:?}", r.json);
        assert_eq!(r.json["code"], "confirm_required");
    }
    assert!(sw.daemon.write_paths().is_empty());
}
