mod common;

use common::{fake_swarm::connected_swarm as swarm, *};

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
