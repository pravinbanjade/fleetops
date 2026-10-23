mod common;

use common::{
    agent::TestAgent,
    fake_swarm::{trust, FakeSwarm, SwarmPki},
    *,
};
use fleetops_tunnel::control::{Msg, SwarmClaim};
use serde_json::json;
use std::sync::Arc;

struct Swarm {
    t: TestApp,
    admin: Session,
    env_id: i64,
    _agents: Vec<TestAgent>,
}

/// A trusted swarm "prod" with n1 (manager) and n2 (worker) connected.
async fn swarm() -> Swarm {
    let t = app().await;
    let admin = t.setup_admin().await;
    let key = fleetops_api::agents::new_agent_key();
    let env_id = t
        .store
        .create_swarm_environment("prod", &fleetops_api::auth_token_hash(&key), fleetops_core::now_unix())
        .await
        .unwrap()
        .id;
    t.state.envs.load(&t.store).await.unwrap();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let base = format!("http://{}", t.serve().await);
    let mut agents = vec![];
    for (node, manager) in [("n1", true), ("n2", false)] {
        let c = SwarmClaim {
            node_id: node.into(),
            cluster_id: "c1".into(),
            manager,
        };
        let mut a = TestAgent::connect_swarm(&base, &key, daemon.clone() as Arc<FakeSwarm>)
            .await
            .unwrap();
        a.hello_swarm(c.clone()).await;
        a.answer_challenge(&pki.node(node, manager), &c).await;
        assert!(matches!(a.next().await, Some(Msg::Welcome { .. })));
        agents.push(a);
    }
    eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
    Swarm {
        t,
        admin,
        env_id,
        _agents: agents,
    }
}

#[tokio::test]
async fn node_views_list_that_nodes_containers() {
    let sw = swarm().await;
    let r = sw
        .t
        .send(req("GET", "/api/v1/envs/prod@n2/containers", None, Some(&sw.admin)))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json[0]["name"], "web.1.abc");
    let r = sw
        .t
        .send(req("GET", "/api/v1/envs/prod@n2/status", None, Some(&sw.admin)))
        .await;
    assert_eq!(r.json["state"], "up");
}

#[tokio::test]
async fn node_views_exist_only_for_swarm_environments() {
    let sw = swarm().await;
    sw.t.add_env(&sw.admin, "local").await;
    for path in ["/api/v1/envs/local@x/containers", "/api/v1/envs/prod@zz/containers"] {
        let r = sw.t.send(req("GET", path, None, Some(&sw.admin))).await;
        assert_eq!(r.status, 404, "{path}: {:?}", r.json);
    }
}

#[tokio::test]
async fn stacks_are_refused_on_a_node_view() {
    let sw = swarm().await;
    let r = sw
        .t
        .send(req("GET", "/api/v1/envs/prod@n2/stacks", None, Some(&sw.admin)))
        .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert_eq!(r.json["message"], "stacks are not available on a single swarm node");
}

#[tokio::test]
async fn compose_projects_list_on_a_node_view() {
    let sw = swarm().await;
    let r = sw
        .t
        .send(req("GET", "/api/v1/envs/prod@n2/compose-projects", None, Some(&sw.admin)))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert!(r.json.to_string().contains("blog"), "{:?}", r.json);
}

#[tokio::test]
async fn node_view_audit_names_the_node() {
    let sw = swarm().await;
    // The fake answers any other request with a version body, so a restart "succeeds".
    let r = sw
        .t
        .send(req(
            "POST",
            "/api/v1/envs/prod@n2/containers/c0ffee/restart",
            None,
            Some(&sw.admin),
        ))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    let rows = sw.t.store.list_audit(50, None).await.unwrap();
    let row = rows
        .iter()
        .find(|r| r.action.starts_with("container."))
        .expect("audited");
    assert_eq!(row.environment_id, Some(sw.env_id));
    assert_eq!(row.detail.as_ref().unwrap()["node"], json!("n2"));
}

#[tokio::test]
async fn a_viewer_of_the_swarm_can_read_a_node() {
    let sw = swarm().await;
    sw.t.create_user(&sw.admin, "bob", "bob-password", "user").await;
    sw.t.grant("bob", "prod", "viewer", None).await;
    let bob = session_from(&sw.t.login("bob", "bob-password").await);
    let r = sw
        .t
        .send(req("GET", "/api/v1/envs/prod@n2/containers", None, Some(&bob)))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
}
