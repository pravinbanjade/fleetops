mod common;

use common::{
    agent::TestAgent,
    fake_swarm::{FakeSwarm, NodeCert, SwarmPki},
    *,
};
use fleetops_tunnel::control::{ByeReason, Msg, SwarmClaim, PROTO_MAJOR};
use std::sync::Arc;

/// A swarm environment made in the store; returns (env id, agent key).
async fn swarm_env(t: &TestApp) -> (i64, String) {
    let key = fleetops_api::agents::new_agent_key();
    let rec = t
        .store
        .create_swarm_environment(
            "prod",
            &fleetops_api::auth_token_hash(&key),
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    t.state.envs.load(&t.store).await.unwrap();
    (rec.id, key)
}

async fn base(t: &TestApp) -> String {
    format!("http://{}", t.serve().await)
}

fn claim(pki: &SwarmPki, node: &str, manager: bool) -> SwarmClaim {
    SwarmClaim {
        node_id: node.into(),
        cluster_id: pki.cluster_id.clone(),
        manager,
    }
}

/// Connects as `node`, proves it with `cert`, and returns the agent and
/// the server's reply to the proof.
async fn join(
    base: &str,
    key: &str,
    daemon: &Arc<FakeSwarm>,
    c: SwarmClaim,
    cert: &NodeCert,
) -> (TestAgent, Option<Msg>) {
    let mut a = TestAgent::connect_swarm(base, key, daemon.clone())
        .await
        .unwrap();
    a.hello_swarm(c.clone()).await;
    a.answer_challenge(cert, &c).await;
    let reply = a.next().await;
    (a, reply)
}

fn bye(reason: ByeReason) -> Option<Msg> {
    Some(Msg::Bye { reason })
}

#[tokio::test]
async fn a_manager_pins_the_cluster_and_comes_up() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_a, reply) = join(&base, &key, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    assert!(matches!(reply, Some(Msg::Welcome { agent_key: None, .. })), "{reply:?}");
    eventually("registered", || t.state.agents.nodes(env_id).len() == 1).await;
    let pinned = t.store.get_swarm_cluster(env_id).await.unwrap().unwrap();
    assert_eq!(pinned.cluster_id, "c1");
    assert_eq!(pinned.trust_root, pki.root);
    let agents = t.store.list_swarm_agents(env_id).await.unwrap();
    assert_eq!(agents[0].node_id, "n1");
    assert!(agents[0].manager);
    // Audited just after the session is registered.
    for _ in 0..100 {
        if t.audit_actions().await.contains(&"agent.connect".to_string()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(t.audit_actions().await.contains(&"agent.connect".to_string()));
    let env = t.state.envs.get("prod").unwrap();
    assert!(matches!(
        env.status().await,
        fleetops_core::env::EnvStatus::Up { .. }
    ));
}

#[tokio::test]
async fn a_worker_before_any_manager_is_told_not_yet() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let mut w = TestAgent::connect_swarm(&base, &key, daemon.clone())
        .await
        .unwrap();
    w.hello_swarm(claim(&pki, "n2", false)).await;
    // Nothing is pinned yet, so the worker is turned away before any proof.
    assert_eq!(w.next().await, bye(ByeReason::NotYet));
    assert!(t.state.agents.nodes(env_id).is_empty());

    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    let (_w, reply) = join(&base, &key, &daemon, claim(&pki, "n2", false), &pki.node("n2", false)).await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
    eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
}

#[tokio::test]
async fn two_nodes_stay_connected() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let m_cert = pki.node("n1", true);
    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &m_cert).await;
    let (_w, _) = join(&base, &key, &daemon, claim(&pki, "n2", false), &pki.node("n2", false)).await;
    eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
    // The manager's agent restarts: only its own old session goes.
    let (_m2, reply) = join(&base, &key, &daemon, claim(&pki, "n1", true), &m_cert).await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })));
    eventually("still both", || t.state.agents.nodes(env_id).len() == 2).await;
    assert!(t.state.agents.get_node(env_id, "n2").is_some());
}

#[tokio::test]
async fn a_removed_node_is_refused() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    daemon.remove("n2");
    // Its certificate is still valid; membership is what refuses it.
    let (_w, reply) = join(&base, &key, &daemon, claim(&pki, "n2", false), &pki.node("n2", false)).await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    let reason = t.state.agents.node_offline_reason(env_id, "n2").unwrap();
    assert!(reason.contains("not a member of this swarm"), "{reason}");
    assert_eq!(t.state.agents.nodes(env_id).len(), 1);
}

#[tokio::test]
async fn a_worker_claiming_manager_is_refused() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    // A worker's certificate cannot back a manager claim.
    let (_w, reply) = join(&base, &key, &daemon, claim(&pki, "n2", true), &pki.node("n2", false)).await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    assert_eq!(t.state.agents.nodes(env_id).len(), 1);
    assert!(t.audit_actions().await.contains(&"agent.reject".to_string()));
}

#[tokio::test]
async fn a_proof_from_another_swarm_is_refused() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    // Same ids, another CA.
    let rogue = SwarmPki::new("c1");
    let (_w, reply) = join(&base, &key, &daemon, claim(&pki, "n2", false), &rogue.node("n2", false)).await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    let reason = t.state.agents.node_offline_reason(env_id, "n2").unwrap();
    assert!(reason.contains("root CA"), "{reason}");
}

#[tokio::test]
async fn a_docker_agent_on_a_swarm_environment_is_refused() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let mut a = TestAgent::connect(&base, &key).await.unwrap();
    a.hello(PROTO_MAJOR).await;
    assert_eq!(a.next().await, bye(ByeReason::Incompatible));
    let reason = t.state.agents.offline_reason(env_id).unwrap();
    assert!(reason.contains("expects swarm"), "{reason}");
}

#[tokio::test]
async fn a_swarm_agent_on_a_docker_environment_is_refused() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let mut r = req(
        "POST",
        "/api/v1/envs",
        Some(serde_json::json!({"name": "web-01", "kind": "docker-agent"})),
        Some(&admin),
    );
    r.headers_mut()
        .insert("origin", "http://127.0.0.1".parse().unwrap());
    let token = t.send(r).await.json["enrollment"]["join_token"]
        .as_str()
        .unwrap()
        .to_string();
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true)]);
    let mut a = TestAgent::connect_swarm(&base, &token, daemon).await.unwrap();
    a.hello_swarm(claim(&pki, "n1", true)).await;
    assert_eq!(a.next().await, bye(ByeReason::Incompatible));
}

#[tokio::test]
async fn the_previous_key_connects_during_rotation_grace() {
    let t = app().await;
    let (env_id, old) = swarm_env(&t).await;
    let new = fleetops_api::agents::new_agent_key();
    t.store
        .rotate_swarm_key(
            env_id,
            &fleetops_api::auth_token_hash(&new),
            fleetops_core::now_unix() + 86_400,
        )
        .await
        .unwrap();
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_m, reply) = join(&base, &old, &daemon, claim(&pki, "n1", true), &pki.node("n1", true)).await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
    let (_w, reply) = join(&base, &new, &daemon, claim(&pki, "n2", false), &pki.node("n2", false)).await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
    eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
}

#[tokio::test]
async fn a_bad_signature_is_refused_and_audited() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true)]);
    let c = claim(&pki, "n1", true);
    let cert = pki.node("n1", true);
    let mut a = TestAgent::connect_swarm(&base, &key, daemon).await.unwrap();
    a.hello_swarm(c.clone()).await;
    let Some(Msg::Challenge { .. }) = a.next().await else {
        panic!("expected a challenge")
    };
    // Signs some other nonce.
    let msg = fleetops_tunnel::swarm_proof::proof_message(&"00".repeat(32), &c);
    a.send(&Msg::Proof {
        cert_chain_pem: cert.chain.clone(),
        signature: fleetops_tunnel::swarm_proof::sign(&cert.key, &msg).unwrap(),
    })
    .await;
    assert_eq!(a.next().await, bye(ByeReason::Incompatible));
    assert!(t.state.agents.nodes(env_id).is_empty());
    assert!(t.store.get_swarm_cluster(env_id).await.unwrap().is_none(), "nothing pinned");
    assert!(t.audit_actions().await.contains(&"agent.reject".to_string()));
}
