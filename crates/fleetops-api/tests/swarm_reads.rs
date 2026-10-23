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

/// Grants set by the test, by user id: Bob's custom role.
#[derive(Default)]
struct Fixed(std::sync::Mutex<Vec<fleetops_core::access::Grant>>);

#[async_trait::async_trait]
impl fleetops_core::access::ExtraGrants for Fixed {
    async fn env_grants(
        &self,
        _: i64,
        _: i64,
    ) -> fleetops_core::Result<Vec<fleetops_core::access::Grant>> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn user_grants(
        &self,
        _: i64,
    ) -> fleetops_core::Result<std::collections::HashMap<i64, Vec<fleetops_core::access::Grant>>>
    {
        Ok(std::collections::HashMap::from([(
            1,
            self.0.lock().unwrap().clone(),
        )]))
    }
}

/// A swarm, and Bob with a custom role holding exactly `perms` on it.
async fn with_custom(
    perms: &[(fleetops_core::perm::Row, fleetops_core::perm::Verb)],
) -> (common::fake_swarm::Swarm, Session) {
    use fleetops_core::perm::{PermSet, Permission};
    let extra = std::sync::Arc::new(Fixed::default());
    let t = app_with_extra_and(extra.clone(), fleetops_api::ApiConfig::default()).await;
    let sw = common::fake_swarm::connected_swarm_on(t).await;
    sw.t.create_user(&sw.admin, "bob", "bob-password", "user")
        .await;
    let set: PermSet = perms.iter().map(|(r, v)| Permission::new(*r, *v)).collect();
    *extra.0.lock().unwrap() = vec![fleetops_core::access::Grant::custom(
        fleetops_core::access::CustomRef {
            id: 1,
            name: "test".into(),
        },
        set,
        None,
    )];
    let bob = session_from(&sw.t.login("bob", "bob-password").await);
    (sw, bob)
}

#[tokio::test]
async fn a_service_spec_masks_env_without_reveal() {
    use fleetops_core::perm::{Row, Verb};
    let (sw, bob) = with_custom(&[(Row::Containers, Verb::View)]).await;
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
async fn service_logs_need_containers_view() {
    use fleetops_core::perm::{Row, Verb};
    let (sw, bob) = with_custom(&[(Row::Images, Verb::View)]).await;
    let mut ws = Ws::open(sw.t.serve().await, &bob).await;
    ws.send(serde_json::json!({"op": "sub", "sub": 1, "topic": "logs", "env": "prod", "id": "service/s1", "opts": {}}))
        .await;
    let err = ws.until(|v| v["type"] == "error").await;
    assert_eq!(err["code"], "forbidden", "{err}");
}
