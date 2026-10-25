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
async fn redeploy_and_rollback_need_no_body() {
    let sw = swarm().await;
    for a in ["redeploy", "rollback"] {
        let r = send(&sw, "POST", &format!("{S}/s1/{a}"), None).await;
        assert_eq!(r.status, 204, "{a}: {:?}", r.json);
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

/// A masked value can't follow its key to a new name: the edit is refused
/// rather than dropping the variable.
#[tokio::test]
async fn renaming_a_masked_variable_is_refused() {
    let (sw, bob) = with_custom(&[
        (Row::SwarmServices, Verb::View),
        (Row::SwarmServices, Verb::Operate),
    ])
    .await;
    let r =
        sw.t.send(req("GET", &format!("{S}/s1"), None, Some(&bob)))
            .await;
    let mut spec = r.json["spec"].clone();
    let env = spec["TaskTemplate"]["ContainerSpec"]["Env"]
        .as_array_mut()
        .unwrap();
    let masked = env[0].as_str().unwrap().replace("DB_PASSWORD=", "DB_PASS=");
    env[0] = json!(masked);
    let r =
        sw.t.send(req(
            "PUT",
            &format!("{S}/s1"),
            Some(json!({"spec": spec, "version": r.json["version"]})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
    assert!(
        r.json["message"].as_str().unwrap().contains("DB_PASS"),
        "{:?}",
        r.json
    );
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/s1/update")));
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

// Stack scoping (swarm spec §6).

const ALL: &[(Row, Verb)] = &[
    (Row::SwarmNodes, Verb::View),
    (Row::SwarmNodes, Verb::Operate),
    (Row::SwarmNodes, Verb::Delete),
    (Row::SwarmServices, Verb::View),
    (Row::SwarmServices, Verb::Create),
    (Row::SwarmServices, Verb::Operate),
    (Row::SwarmServices, Verb::Delete),
    (Row::SwarmSecrets, Verb::View),
    (Row::SwarmSecrets, Verb::Create),
    (Row::SwarmSecrets, Verb::Delete),
    (Row::SwarmConfigs, Verb::View),
    (Row::SwarmConfigs, Verb::Create),
    (Row::SwarmConfigs, Verb::Delete),
    (Row::SwarmConfigs, Verb::Reveal),
    (Row::Networks, Verb::View),
    (Row::Networks, Verb::Create),
    (Row::Networks, Verb::Delete),
];

async fn as_bob(sw: &Swarm, bob: &Session, method: &str, path: &str, body: Option<Value>) -> Resp {
    sw.t.send(req(method, path, body, Some(bob))).await
}

fn ends(r: &Resp, text: &str) {
    assert!(
        r.json["message"]
            .as_str()
            .unwrap_or_default()
            .ends_with(text),
        "{} {:?}",
        r.status,
        r.json
    );
}

const SW: &str = "/api/v1/envs/prod/swarm";

#[tokio::test]
async fn scoped_writes_follow_stacks() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    let scale = Some(json!({"replicas": 2}));
    assert_eq!(
        as_bob(
            &sw,
            &bob,
            "POST",
            &format!("{SW}/services/s1/scale"),
            scale.clone()
        )
        .await
        .status,
        204
    );
    let r = as_bob(&sw, &bob, "POST", &format!("{SW}/services/s2/scale"), scale).await;
    assert_eq!(r.status, 404);
    assert_eq!(
        as_bob(
            &sw,
            &bob,
            "POST",
            &format!("{SW}/services/s1/redeploy"),
            Some(json!({}))
        )
        .await
        .status,
        204
    );
    for (path, body) in [
        (
            format!("{SW}/services"),
            json!({"spec": {"Name": "x", "Labels": {"com.docker.stack.namespace": "blog"}}}),
        ),
        (format!("{SW}/secrets"), json!({"name": "x", "value": "v"})),
        (format!("{SW}/configs"), json!({"name": "x", "value": "v"})),
        (format!("{SW}/networks"), json!({"name": "x"})),
    ] {
        let r = as_bob(&sw, &bob, "POST", &path, Some(body)).await;
        assert_eq!(r.status, 403, "{path}");
        ends(&r, "only access to the whole swarm covers this");
    }
    let r = as_bob(
        &sw,
        &bob,
        "PATCH",
        &format!("{SW}/nodes/n2"),
        Some(json!({"availability": "pause"})),
    )
    .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        as_bob(&sw, &bob, "DELETE", &format!("{SW}/secrets/sec1"), None)
            .await
            .status,
        204
    );
    let r = as_bob(&sw, &bob, "GET", &format!("{SW}/configs/cfg2/data"), None).await;
    assert_eq!(r.status, 404);
    assert_eq!(
        as_bob(&sw, &bob, "DELETE", &format!("{SW}/networks/net1"), None)
            .await
            .status,
        204
    );
    assert_eq!(
        as_bob(&sw, &bob, "DELETE", &format!("{SW}/services/s1"), None)
            .await
            .status,
        204
    );
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/s2")));
}

async fn edit(sw: &Swarm, bob: &Session, spec: Value) -> Resp {
    as_bob(
        sw,
        bob,
        "PUT",
        &format!("{SW}/services/s1"),
        Some(json!({"spec": spec, "version": 10})),
    )
    .await
}

fn blog_spec() -> Value {
    let mut spec = common::fake_swarm::services_json()[0]["Spec"].clone();
    spec["TaskTemplate"]["ContainerSpec"]["Image"] = json!("nginx:1.28");
    spec
}

#[tokio::test]
async fn a_scoped_edit_cannot_move_a_service_out() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    let mut spec = blog_spec();
    spec["Labels"]["com.docker.stack.namespace"] = json!("shop");
    let r = edit(&sw, &bob, spec).await;
    assert_eq!(r.status, 403);
    ends(&r, "stack 'shop' is not in your access");
    let mut spec = blog_spec();
    spec["Labels"]
        .as_object_mut()
        .unwrap()
        .remove("com.docker.stack.namespace");
    let r = edit(&sw, &bob, spec).await;
    assert_eq!(r.status, 403);
    ends(&r, "only access to the whole swarm covers this");
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/update")));
    assert!(sw
        .t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn a_scoped_edit_cannot_reference_another_stacks_secret() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    sw.daemon.secrets.lock().unwrap().as_array_mut().unwrap().push(json!(
        {"ID": "sec9", "Version": {"Index": 1}, "Spec": {"Name": "shop_db", "Labels": {"com.docker.stack.namespace": "shop"}}}
    ));
    let mut spec = blog_spec();
    spec["TaskTemplate"]["ContainerSpec"]["Secrets"] =
        json!([{"SecretID": "sec9", "SecretName": "shop_db"}]);
    let r = edit(&sw, &bob, spec).await;
    assert_eq!(r.status, 403);
    ends(&r, "only access to the whole swarm covers this");
    // The unlabelled config blog_nginx is outside every stack.
    let r = edit(&sw, &bob, blog_spec()).await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    ends(&r, "only access to the whole swarm covers this");
    let mut spec = blog_spec();
    spec["TaskTemplate"]["ContainerSpec"]
        .as_object_mut()
        .unwrap()
        .remove("Configs");
    assert_eq!(edit(&sw, &bob, spec).await.status, 204);
}

/// Node pages place containers in stacks by their own stack label: a
/// scoped edit can't move its containers into another stack.
#[tokio::test]
async fn a_scoped_edit_cannot_relabel_its_containers() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    let mut spec = blog_spec();
    spec["TaskTemplate"]["ContainerSpec"]
        .as_object_mut()
        .unwrap()
        .remove("Configs");
    let mut moved = spec.clone();
    moved["TaskTemplate"]["ContainerSpec"]["Labels"] =
        json!({"com.docker.stack.namespace": "shop"});
    assert_eq!(edit(&sw, &bob, moved).await.status, 403);
    spec["TaskTemplate"]["ContainerSpec"]["Labels"] = json!({"com.docker.stack.namespace": "blog"});
    assert_eq!(edit(&sw, &bob, spec).await.status, 204);
}

/// A rollback puts back Docker's previous spec: for a grant limited to
/// stacks, it is checked as an edit to that spec would be.
#[tokio::test]
async fn a_scoped_rollback_checks_the_previous_spec() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    sw.daemon.secrets.lock().unwrap().as_array_mut().unwrap().push(json!(
        {"ID": "sec9", "Version": {"Index": 1}, "Spec": {"Name": "shop_db", "Labels": {"com.docker.stack.namespace": "shop"}}}
    ));
    let set_previous = |previous: Value| {
        let mut services = sw.daemon.services.lock().unwrap();
        let s1 = services
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["ID"] == "s1")
            .unwrap();
        s1["PreviousSpec"] = previous;
    };
    let path = format!("{SW}/services/s1/rollback");
    let rollback = || as_bob(&sw, &bob, "POST", &path, None);
    let mut spec = blog_spec();
    spec["TaskTemplate"]["ContainerSpec"]
        .as_object_mut()
        .unwrap()
        .remove("Configs");
    let mut other = spec.clone();
    other["TaskTemplate"]["ContainerSpec"]["Secrets"] =
        json!([{"SecretID": "sec9", "SecretName": "shop_db"}]);
    set_previous(other);
    let r = rollback().await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    let mut moved = spec.clone();
    moved["Labels"]["com.docker.stack.namespace"] = json!("shop");
    set_previous(moved);
    assert_eq!(rollback().await.status, 403);
    set_previous(Value::Null);
    let r = rollback().await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    let mut relabelled = spec.clone();
    relabelled["TaskTemplate"]["ContainerSpec"]["Labels"] =
        json!({"com.docker.stack.namespace": "shop"});
    set_previous(relabelled);
    assert_eq!(rollback().await.status, 403);
    set_previous(spec);
    assert_eq!(rollback().await.status, 204);
}

#[tokio::test]
async fn mixed_grants_view_all_operate_in_scope() {
    use common::fake_swarm::{custom_grant, with_grants};
    let (sw, bob) = with_grants(vec![
        fleetops_core::access::Grant::full(fleetops_core::access::EnvRole::Viewer),
        custom_grant(&[(Row::SwarmServices, Verb::Operate)], Some(&["blog"])),
    ])
    .await;
    sw.daemon.add_shop();
    let scale = Some(json!({"replicas": 2}));
    assert_eq!(
        as_bob(
            &sw,
            &bob,
            "POST",
            &format!("{SW}/services/s4/scale"),
            scale.clone()
        )
        .await
        .status,
        403
    );
    assert_eq!(
        as_bob(&sw, &bob, "POST", &format!("{SW}/services/s1/scale"), scale)
            .await
            .status,
        204
    );
}

#[tokio::test]
async fn hidden_objects_read_as_missing_to_writes() {
    // Bob may change `blog` only, and see nothing else.
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    sw.daemon.add_shop();
    sw.daemon.secrets.lock().unwrap().as_array_mut().unwrap().push(json!(
        {"ID": "sec9", "Version": {"Index": 1}, "Spec": {"Name": "shop_db", "Labels": {"com.docker.stack.namespace": "shop"}}}
    ));
    for (method, path, body, missing) in [
        (
            "POST",
            format!("{SW}/services/s4/scale"),
            Some(json!({"replicas": 2})),
            "no such service 's4'",
        ),
        (
            "DELETE",
            format!("{SW}/services/shop_api"),
            None,
            "no such service 'shop_api'",
        ),
        (
            "DELETE",
            format!("{SW}/secrets/shop_db"),
            None,
            "no such secret 'shop_db'",
        ),
        (
            "DELETE",
            format!("{SW}/secrets/nope"),
            None,
            "no such secret 'nope'",
        ),
        (
            "GET",
            format!("{SW}/configs/cfg2/data"),
            None,
            "no such config 'cfg2'",
        ),
        (
            "DELETE",
            format!("{SW}/networks/nope"),
            None,
            "no such swarm network 'nope'",
        ),
    ] {
        let r = as_bob(&sw, &bob, method, &path, body).await;
        assert_eq!(r.status, 404, "{path}: {:?}", r.json);
        ends(&r, missing);
    }
    assert!(sw
        .t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn a_scoped_edit_checks_every_reference() {
    let (sw, bob) = common::fake_swarm::with_custom_in(ALL, Some(&["blog"])).await;
    sw.daemon
        .networks
        .lock()
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(json!(
            {"Name": "shop_backend", "Id": "net9", "Scope": "swarm", "Driver": "overlay",
             "Labels": {"com.docker.stack.namespace": "shop"}}
        ));
    sw.daemon.configs.lock().unwrap().as_array_mut().unwrap().push(json!(
        {"ID": "cfg9", "Version": {"Index": 1}, "Spec": {"Name": "shop_cred", "Labels": {"com.docker.stack.namespace": "shop"}}}
    ));
    let base = || {
        let mut spec = blog_spec();
        spec["TaskTemplate"]["ContainerSpec"]
            .as_object_mut()
            .unwrap()
            .remove("Configs");
        spec
    };
    // The deprecated top-level Networks still attaches a service.
    for target in ["shop_backend", "net9", "nope"] {
        let mut spec = base();
        spec["Networks"] = json!([{"Target": target}]);
        let r = edit(&sw, &bob, spec).await;
        assert_eq!(r.status, 403, "{target}: {:?}", r.json);
        ends(&r, "only access to the whole swarm covers this");
    }
    let mut spec = base();
    spec["TaskTemplate"]["ContainerSpec"]["Privileges"] =
        json!({"CredentialSpec": {"Config": "cfg9"}});
    let r = edit(&sw, &bob, spec).await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/update")));
}
