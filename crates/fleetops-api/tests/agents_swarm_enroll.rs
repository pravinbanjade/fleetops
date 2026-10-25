mod common;

use common::{
    agent::TestAgent,
    fake_swarm::{trust, FakeSwarm, SwarmPki},
    *,
};
use fleetops_api::ApiConfig;
use fleetops_tunnel::control::{ByeReason, Msg, SwarmClaim};
use serde_json::json;

const CA: &str = "-----BEGIN CERTIFICATE-----\nMIIFAKE\n-----END CERTIFICATE-----\n";

fn with_origin(
    mut r: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    r.headers_mut()
        .insert("origin", "https://fleet.example.com".parse().unwrap());
    r
}

async fn create(t: &TestApp, admin: &Session) -> Resp {
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "prod", "kind": "swarm-agent"})),
            Some(admin),
        )))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    r
}

fn env_id(t: &TestApp) -> i64 {
    t.state.envs.get("prod").unwrap().id()
}

/// Connects and proves node `node` with `key`; returns the live agent.
async fn join(
    t: &TestApp,
    key: &str,
    pki: &SwarmPki,
    daemon: &std::sync::Arc<FakeSwarm>,
    node: &str,
    manager: bool,
) -> TestAgent {
    let base = format!("http://{}", t.serve().await);
    let c = SwarmClaim {
        node_id: node.into(),
        cluster_id: pki.cluster_id.clone(),
        manager,
    };
    let mut a = TestAgent::connect_swarm(&base, key, daemon.clone())
        .await
        .unwrap();
    a.hello_swarm(c.clone()).await;
    a.answer_challenge(&pki.node(node, manager), &c).await;
    assert!(matches!(a.next().await, Some(Msg::Welcome { .. })));
    a
}

#[tokio::test]
async fn creating_a_swarm_environment_returns_the_key_once() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = create(&t, &admin).await;
    let e = &r.json["swarm_enrollment"];
    let key = e["agent_key"].as_str().unwrap();
    assert!(key.starts_with("fk_"));
    assert_eq!(e["key_generation"], 1);
    assert_eq!(e["secret_name"], "fleetops-agent-key");
    assert_eq!(
        e["secret_command"],
        "docker secret create fleetops-agent-key -"
    );
    let yaml = e["outside"]["stack_yaml"].as_str().unwrap();
    assert!(yaml.contains("mode: global"), "{yaml}");
    assert!(yaml.contains("/var/lib/docker/swarm/certificates:/swarm-certs:ro"));
    assert!(yaml.contains("fleetops-agent-key"));
    assert!(yaml.contains("https://fleet.example.com"));
    assert!(!yaml.contains(key), "the key never goes in the stack file");
    assert_eq!(
        e["outside"]["deploy_command"],
        "docker stack deploy -c fleetops-agent.yml fleetops-agent"
    );
    assert!(r.json["enrollment"].is_null());
    // The key is not shown again.
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert!(!list.json.to_string().contains(key));
    assert_eq!(list.json[0]["kind"], "swarm-agent");
    assert_eq!(list.json[0]["agent"]["enrolled"], true);
    assert_eq!(list.json[0]["agent"]["connected"], false);
}

#[tokio::test]
async fn inside_is_offered_only_with_a_generated_cert() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let e = create(&t, &admin).await.json["swarm_enrollment"].clone();
    assert!(e["inside"].is_null());
    assert!(e["inside_unavailable"]
        .as_str()
        .unwrap()
        .contains("--tls-bind"));

    let t = app_with(ApiConfig {
        agent_ca_pem: Some(CA.into()),
        ..ApiConfig::default()
    })
    .await;
    let admin = t.setup_admin().await;
    let e = create(&t, &admin).await.json["swarm_enrollment"].clone();
    assert!(e["inside_unavailable"].is_null());
    let yaml = e["inside"]["stack_yaml"].as_str().unwrap();
    assert!(yaml.contains("https://fleetops:9443"), "{yaml}");
    assert!(yaml.contains("FLEETOPS_AGENT_CA_PEM"));
    assert!(yaml.contains("MIIFAKE"));
    assert!(yaml.contains("external: true"));
}

#[tokio::test]
async fn rotate_names_a_new_secret_and_keeps_the_old_key_for_24h() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let old = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/prod/swarm/rotate",
            None,
            Some(&admin),
        )))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["key_generation"], 2);
    assert_eq!(r.json["secret_name"], "fleetops-agent-key-2");
    assert!(r.json["outside"]["stack_yaml"]
        .as_str()
        .unwrap()
        .contains("source: fleetops-agent-key-2"));
    let new = r.json["agent_key"].as_str().unwrap();
    assert_ne!(new, old);
    let now = fleetops_core::now_unix();
    let id = env_id(&t);
    let hash = fleetops_api::auth_token_hash;
    assert!(t
        .store
        .swarm_by_key(&hash(&old), now)
        .await
        .unwrap()
        .is_some());
    assert!(t
        .store
        .swarm_by_key(&hash(&old), now + 86_401)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        t.store
            .swarm_by_key(&hash(new), now)
            .await
            .unwrap()
            .map(|x| x.0),
        Some(id)
    );
    assert!(t
        .audit_actions()
        .await
        .contains(&"agent.rotate".to_string()));
}

/// The list says until when a rotation's old key works, so Finish is
/// offered only while one is pending.
#[tokio::test]
async fn the_env_list_says_when_a_rotation_ends() {
    let t = app().await;
    let admin = t.setup_admin().await;
    create(&t, &admin).await;
    let list = || t.send(req("GET", "/api/v1/envs", None, Some(&admin)));
    assert!(list().await.json[0]["agent"]["rotation_ends_at"].is_null());
    t.send(with_origin(req(
        "POST",
        "/api/v1/envs/prod/swarm/rotate",
        None,
        Some(&admin),
    )))
    .await;
    let ends = list().await.json[0]["agent"]["rotation_ends_at"].as_i64();
    let now = fleetops_core::now_unix();
    assert!(ends.is_some_and(|e| e > now + 86_000), "{ends:?}");
    t.send(req(
        "POST",
        "/api/v1/envs/prod/swarm/rotate/finish",
        None,
        Some(&admin),
    ))
    .await;
    assert!(list().await.json[0]["agent"]["rotation_ends_at"].is_null());
}

/// A swarm is up only while a manager's agent is connected: workers alone
/// don't make it "connected".
#[tokio::test]
async fn workers_alone_are_not_connected() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let key = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let m = join(&t, &key, &pki, &daemon, "n1", true).await;
    let _w = join(&t, &key, &pki, &daemon, "n2", false).await;
    let list = || t.send(req("GET", "/api/v1/envs", None, Some(&admin)));
    assert_eq!(list().await.json[0]["agent"]["connected"], true);
    drop(m);
    for _ in 0..200 {
        if list().await.json[0]["swarm_nodes_connected"] == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let env = &list().await.json[0];
    assert_eq!(env["swarm_nodes_connected"], 1);
    assert_eq!(env["agent"]["connected"], false, "{env}");
}

#[tokio::test]
async fn finish_rotation_revokes_sessions_on_the_old_key() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let old = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let mut m = join(&t, &old, &pki, &daemon, "n1", true).await;
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/prod/swarm/rotate",
            None,
            Some(&admin),
        )))
        .await;
    let new = r.json["agent_key"].as_str().unwrap().to_string();
    let _w = join(&t, &new, &pki, &daemon, "n2", false).await;
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/rotate/finish",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    loop {
        match m.next().await {
            Some(Msg::Bye { reason }) => {
                assert_eq!(reason, ByeReason::Revoked);
                break;
            }
            Some(Msg::Ping { .. }) => continue,
            other => panic!("{other:?}"),
        }
    }
    let id = env_id(&t);
    eventually("old session gone", || {
        t.state.agents.get_node(id, "n1").is_none()
    })
    .await;
    assert!(t.state.agents.get_node(id, "n2").is_some());
    assert_eq!(
        TestAgent::connect(&format!("http://{}", t.serve().await), &old)
            .await
            .err(),
        Some(401)
    );
}

#[tokio::test]
async fn retrust_clears_the_pin_and_drops_sessions() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let key = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let _m = join(&t, &key, &pki, &daemon, "n1", true).await;
    let id = env_id(&t);
    assert!(t.store.get_swarm_cluster(id).await.unwrap().is_some());
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/retrust",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(t.store.get_swarm_cluster(id).await.unwrap().is_none());
    eventually("dropped", || t.state.agents.nodes(id).is_empty()).await;
    assert!(t
        .audit_actions()
        .await
        .contains(&"swarm.retrust".to_string()));
}

#[tokio::test]
async fn docker_routes_refuse_a_swarm_environment() {
    let t = app().await;
    let admin = t.setup_admin().await;
    create(&t, &admin).await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/containers",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(r.json.to_string().contains("wrong environment kind"));
}

#[tokio::test]
async fn agents_list_merges_nodes_without_agents() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let key = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false), ("n3", false)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let _m = join(&t, &key, &pki, &daemon, "n1", true).await;
    let _w = join(&t, &key, &pki, &daemon, "n2", false).await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/agents",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let rows = r.json.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["node_id"], "n1");
    assert_eq!(rows[0]["role"], "manager");
    assert_eq!(rows[0]["availability"], "active");
    assert_eq!(rows[0]["connected"], true);
    assert_eq!(rows[0]["agent_version"], "0.7.0-test");
    assert_eq!(rows[1]["connected"], true);
    assert_eq!(rows[2]["node_id"], "n3");
    assert_eq!(rows[2]["hostname"], "host-n3");
    assert_eq!(rows[2]["connected"], false);
    assert!(rows[2]["agent_version"].is_null());
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert_eq!(list.json[0]["swarm_nodes_connected"], 2);
}

#[tokio::test]
async fn only_admins_rotate_or_retrust() {
    let t = app().await;
    let admin = t.setup_admin().await;
    create(&t, &admin).await;
    t.create_user(&admin, "bob", "bob-password", "user").await;
    t.grant("bob", "prod", "operator", None).await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    for path in [
        "/api/v1/envs/prod/swarm/rotate",
        "/api/v1/envs/prod/swarm/rotate/finish",
        "/api/v1/envs/prod/swarm/retrust",
    ] {
        let r = t
            .send(with_origin(req("POST", path, None, Some(&bob))))
            .await;
        assert_eq!(r.status, 403, "{path}: {:?}", r.json);
    }
    // Operators may read the agents list.
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/agents",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
}

#[tokio::test]
async fn a_swarm_environment_takes_no_url_or_kube_options() {
    let t = app().await;
    let admin = t.setup_admin().await;
    for body in [
        json!({"name": "s1", "kind": "swarm-agent", "url": "unix:///x.sock"}),
        json!({"name": "s2", "kind": "swarm-agent", "access": "read-only"}),
        json!({"name": "s3", "kind": "swarm-agent", "custom_resources": true}),
    ] {
        let r = t
            .send(with_origin(req(
                "POST",
                "/api/v1/envs",
                Some(body.clone()),
                Some(&admin),
            )))
            .await;
        assert_eq!(r.status, 400, "{body}: {:?}", r.json);
    }
}

async fn rotate(t: &TestApp, admin: &Session) -> String {
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/prod/swarm/rotate",
            None,
            Some(admin),
        )))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    r.json["agent_key"].as_str().unwrap().to_string()
}

/// Reads control messages, answering pings, until a bye.
async fn until_bye(a: &mut TestAgent) -> ByeReason {
    loop {
        match a.next().await {
            Some(Msg::Ping { n }) => a.pong(n).await,
            Some(Msg::Bye { reason }) => return reason,
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn a_second_rotation_revokes_sessions_on_the_first_key() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let first = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let mut m = join(&t, &first, &pki, &daemon, "n1", true).await;
    rotate(&t, &admin).await;
    rotate(&t, &admin).await;
    assert_eq!(until_bye(&mut m).await, ByeReason::Revoked);
}

#[tokio::test]
async fn sessions_on_the_previous_key_end_when_its_grace_ends() {
    let t = app_with(ApiConfig {
        agent_ping_interval: std::time::Duration::from_millis(200),
        ..ApiConfig::default()
    })
    .await;
    let admin = t.setup_admin().await;
    let old = create(&t, &admin).await.json["swarm_enrollment"]["agent_key"]
        .as_str()
        .unwrap()
        .to_string();
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true)]);
    trust(&t, env_id(&t), &pki, &daemon).await;
    let mut m = join(&t, &old, &pki, &daemon, "n1", true).await;
    // A grace period that ends a second from now.
    t.store
        .rotate_swarm_key(
            env_id(&t),
            &fleetops_api::auth_token_hash("fk_next"),
            fleetops_core::now_unix() + 1,
        )
        .await
        .unwrap();
    let reason = tokio::time::timeout(std::time::Duration::from_secs(10), until_bye(&mut m))
        .await
        .expect("ended within 10 s");
    assert_eq!(reason, ByeReason::Revoked);
}
