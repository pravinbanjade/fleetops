mod common;

use common::{
    fake_swarm::{connected_swarm as swarm, with_custom, with_custom_in},
    *,
};
use fleetops_core::perm::{Row, Verb};

async fn get(sw: &common::fake_swarm::Swarm, path: &str) -> Resp {
    sw.t.send(req("GET", path, None, Some(&sw.admin))).await
}

#[tokio::test]
async fn lists_nodes_services_tasks_secrets_configs_networks() {
    let sw = swarm().await;
    let r = get(&sw, "/api/v1/envs/prod/swarm/nodes").await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json.as_array().unwrap().len(), 2);
    let r = get(&sw, "/api/v1/envs/prod/swarm/nodes/n1").await;
    assert_eq!(r.json["hostname"], "host-n1");
    let r = get(&sw, "/api/v1/envs/prod/swarm/services").await;
    assert_eq!(r.json[0]["name"], "blog_web");
    assert_eq!(r.json[0]["tasks_running"], 2);
    let r = get(&sw, "/api/v1/envs/prod/swarm/services/s1").await;
    assert_eq!(r.json["spec"]["Name"], "blog_web");
    let r = get(&sw, "/api/v1/envs/prod/swarm/tasks").await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
    let r = get(&sw, "/api/v1/envs/prod/swarm/secrets").await;
    assert_eq!(r.json[0]["used_by"][0], "blog_web");
    assert!(r.json[0].get("data").is_none());
    let r = get(&sw, "/api/v1/envs/prod/swarm/configs").await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
    let r = get(&sw, "/api/v1/envs/prod/swarm/networks").await;
    assert_eq!(r.json[0]["name"], "blog_default");
}

#[tokio::test]
async fn tasks_filter_by_service_and_node() {
    let sw = swarm().await;
    let r = get(&sw, "/api/v1/envs/prod/swarm/tasks?node=n3").await;
    assert_eq!(r.json.as_array().unwrap().len(), 1);
    assert_eq!(r.json[0]["id"], "t2");
    let r = get(&sw, "/api/v1/envs/prod/swarm/tasks?service=s2").await;
    assert!(r.json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_service_is_404() {
    let sw = swarm().await;
    let r = get(&sw, "/api/v1/envs/prod/swarm/services/zz").await;
    assert_eq!(r.status, 404);
    assert!(
        r.json["message"]
            .as_str()
            .unwrap()
            .ends_with("no such service 'zz'"),
        "{:?}",
        r.json
    );
    let r = get(&sw, "/api/v1/envs/prod/swarm/nodes/zz").await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn without_a_manager_agent_reads_are_503() {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.store
        .create_swarm_environment("prod", "h", fleetops_core::now_unix())
        .await
        .unwrap();
    t.state.envs.load(&t.store).await.unwrap();
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/services",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 503, "{:?}", r.json);
    assert!(r.json.to_string().contains("no manager agent is connected"));
}

#[tokio::test]
async fn docker_environments_get_wrong_kind() {
    let sw = swarm().await;
    sw.t.add_env(&sw.admin, "local").await;
    let r = get(&sw, "/api/v1/envs/local/swarm/services").await;
    assert_eq!(r.status, 409);
    assert!(r.json.to_string().contains("wrong environment kind"));
}

#[tokio::test]
async fn viewers_may_read() {
    let sw = swarm().await;
    sw.t.create_user(&sw.admin, "bob", "bob-password", "user")
        .await;
    sw.t.grant("bob", "prod", "viewer", None).await;
    let bob = session_from(&sw.t.login("bob", "bob-password").await);
    let r =
        sw.t.send(req(
            "GET",
            "/api/v1/envs/prod/swarm/tasks",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
}

#[tokio::test]
async fn a_service_spec_masks_env_without_reveal() {
    let (sw, bob) = with_custom(&[(Row::SwarmServices, Verb::View)]).await;
    let r =
        sw.t.send(req(
            "GET",
            "/api/v1/envs/prod/swarm/services/s1",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let env = r.json["spec"]["TaskTemplate"]["ContainerSpec"]["Env"].to_string();
    assert!(!env.contains("hunter2"), "{env}");
    assert!(env.contains("DB_PASSWORD="), "{env}");
    // Admins have every permission, Reveal included.
    let r = get(&sw, "/api/v1/envs/prod/swarm/services/s1").await;
    assert!(r.json["spec"]["TaskTemplate"]["ContainerSpec"]["Env"]
        .to_string()
        .contains("hunter2"));
}

#[tokio::test]
async fn service_logs_need_service_view() {
    let (sw, bob) = with_custom(&[(Row::Images, Verb::View)]).await;
    let mut ws = Ws::open(sw.t.serve().await, &bob).await;
    ws.send(serde_json::json!({"op": "sub", "sub": 1, "topic": "logs", "env": "prod", "id": "service/s1", "opts": {}}))
        .await;
    let err = ws.until(|v| v["type"] == "error").await;
    assert_eq!(err["code"], "forbidden", "{err}");
}

async fn get_as(sw: &common::fake_swarm::Swarm, who: &Session, path: &str) -> Resp {
    sw.t.send(req("GET", path, None, Some(who))).await
}

#[tokio::test]
async fn swarm_reads_need_their_rows() {
    let (sw, bob) = with_custom(&[(Row::Containers, Verb::View)]).await;
    for what in [
        "nodes", "services", "tasks", "secrets", "configs", "networks",
    ] {
        let r = get_as(&sw, &bob, &format!("/api/v1/envs/prod/swarm/{what}")).await;
        assert_eq!(r.status, 403, "{what}: {:?}", r.json);
    }
    let (sw, bob) = with_custom(&[(Row::SwarmNodes, Verb::View)]).await;
    assert_eq!(
        get_as(&sw, &bob, "/api/v1/envs/prod/swarm/nodes")
            .await
            .status,
        200
    );
    assert_eq!(
        get_as(&sw, &bob, "/api/v1/envs/prod/swarm/services")
            .await
            .status,
        403
    );
}

const READS: &[(Row, Verb)] = &[
    (Row::SwarmNodes, Verb::View),
    (Row::SwarmServices, Verb::View),
    (Row::SwarmSecrets, Verb::View),
    (Row::SwarmConfigs, Verb::View),
    (Row::Networks, Verb::View),
];

fn names(r: &Resp, key: &str) -> Vec<String> {
    r.json
        .as_array()
        .unwrap_or_else(|| panic!("{:?}", r.json))
        .iter()
        .map(|o| o[key].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn scoped_grants_see_their_stacks() {
    let (sw, bob) = with_custom_in(READS, Some(&["blog"])).await;
    sw.daemon.add_shop();
    let p = "/api/v1/envs/prod/swarm";
    let r = get_as(&sw, &bob, &format!("{p}/services")).await;
    assert_eq!(names(&r, "name"), ["blog_web"]);
    // What the grant may not see reads exactly as what does not exist.
    for id in ["s2", "s4", "shop_api", "nope"] {
        let r = get_as(&sw, &bob, &format!("{p}/services/{id}")).await;
        assert_eq!(r.status, 404, "{id}: {:?}", r.json);
        let m = r.json["message"].as_str().unwrap();
        assert!(m.ends_with(&format!("no such service '{id}'")), "{id}: {m}");
    }
    assert_eq!(
        get_as(&sw, &bob, &format!("{p}/services/s1")).await.status,
        200
    );
    let r = get_as(&sw, &bob, &format!("{p}/tasks")).await;
    assert_eq!(names(&r, "id"), ["t1", "t2"]);
    let r = get_as(&sw, &bob, &format!("{p}/secrets")).await;
    assert_eq!(names(&r, "name"), ["blog_db"]);
    let r = get_as(&sw, &bob, &format!("{p}/configs")).await;
    assert!(names(&r, "name").is_empty());
    let r = get_as(&sw, &bob, &format!("{p}/networks")).await;
    assert_eq!(names(&r, "name"), ["blog_default"]);
    assert!(sw
        .t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn scoped_grants_list_nodes_but_not_their_detail() {
    let (sw, bob) = with_custom_in(READS, Some(&["blog"])).await;
    let r = get_as(&sw, &bob, "/api/v1/envs/prod/swarm/nodes").await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json.as_array().unwrap().len(), 2);
    let r = get_as(&sw, &bob, "/api/v1/envs/prod/swarm/nodes/n1").await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}

#[tokio::test]
async fn mixed_grants_view_everything() {
    use common::fake_swarm::{custom_grant, with_grants};
    let (sw, bob) = with_grants(vec![
        fleetops_core::access::Grant::full(fleetops_core::access::EnvRole::Viewer),
        custom_grant(&[(Row::SwarmServices, Verb::Operate)], Some(&["blog"])),
    ])
    .await;
    sw.daemon.add_shop();
    let r = get_as(&sw, &bob, "/api/v1/envs/prod/swarm/services").await;
    assert_eq!(names(&r, "name").len(), 3, "{:?}", r.json);
    assert_eq!(
        get_as(&sw, &bob, "/api/v1/envs/prod/swarm/services/s4")
            .await
            .status,
        200
    );
}
