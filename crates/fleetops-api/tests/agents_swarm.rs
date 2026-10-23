mod common;

use common::{
    agent::TestAgent,
    fake_swarm::{trust, FakeSwarm, NodeCert, SwarmPki},
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
async fn a_pending_swarm_registers_no_agent() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_a, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    assert_eq!(reply, bye(ByeReason::NotYet));
    assert!(t.state.agents.nodes(env_id).is_empty());
    let pinned = t.store.get_swarm_cluster(env_id).await.unwrap().unwrap();
    assert_eq!(pinned.confirmed_at, None);
    assert_eq!(pinned.trust_root, pki.root);
    let fp = fleetops_tunnel::swarm_proof::fingerprint(&pki.root).unwrap();
    let reason = t.state.agents.node_offline_reason(env_id, "n1").unwrap();
    assert_eq!(
        reason,
        format!("waiting for an admin to trust swarm c1 (root CA {fp})")
    );
    assert!(t.audit_actions().await.contains(&"swarm.pin".to_string()));
}

#[tokio::test]
async fn confirming_lets_the_manager_in() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    let (_pending, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/trust",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["confirmed"], false);
    assert_eq!(r.json["cluster_id"], "c1");
    assert_eq!(r.json["pinned_by"], "host-n1");
    assert_eq!(
        r.json["check_command"],
        "docker swarm ca | openssl x509 -noout -fingerprint -sha256"
    );
    let fp = r.json["fingerprint"].as_str().unwrap().to_string();
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/trust",
            Some(serde_json::json!({"cluster_id": "c1", "fingerprint": fp})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    let pinned = t.store.get_swarm_cluster(env_id).await.unwrap().unwrap();
    assert!(pinned.confirmed_at.is_some());
    assert!(t.audit_actions().await.contains(&"swarm.trust".to_string()));
    let (_m, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    assert!(
        matches!(
            reply,
            Some(Msg::Welcome {
                agent_key: None,
                ..
            })
        ),
        "{reply:?}"
    );
    eventually("registered", || t.state.agents.nodes(env_id).len() == 1).await;
    // The first manager after confirmation fills the known managers, just
    // after it is registered.
    for _ in 0..100 {
        let p = t.store.get_swarm_cluster(env_id).await.unwrap().unwrap();
        if p.known_managers.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let known = t
        .store
        .get_swarm_cluster(env_id)
        .await
        .unwrap()
        .unwrap()
        .known_managers;
    assert_eq!(known, Some(vec!["n1".to_string()]));
    for _ in 0..100 {
        if t.audit_actions()
            .await
            .contains(&"agent.connect".to_string())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(t
        .audit_actions()
        .await
        .contains(&"agent.connect".to_string()));
    let env = t.state.envs.get("prod").unwrap();
    assert!(matches!(
        env.status().await,
        fleetops_core::env::EnvStatus::Up { .. }
    ));
}

#[tokio::test]
async fn only_admins_confirm() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let (env_id, _key) = swarm_env(&t).await;
    let pki = SwarmPki::new("c1");
    t.store
        .pin_swarm_cluster(
            env_id,
            "c1",
            &pki.root,
            "n1",
            "host-n1",
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    t.create_user(&admin, "bob", "bob-password", "user").await;
    t.grant("bob", "prod", "operator", None).await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    let fp = fleetops_tunnel::swarm_proof::fingerprint(&pki.root).unwrap();
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/trust",
            Some(serde_json::json!({"cluster_id": "c1", "fingerprint": fp})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/trust",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 200, "operators may read it");
}

#[tokio::test]
async fn confirming_a_stale_fingerprint_is_409() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let (env_id, _key) = swarm_env(&t).await;
    let pki = SwarmPki::new("c1");
    t.store
        .pin_swarm_cluster(
            env_id,
            "c1",
            &pki.root,
            "n1",
            "host-n1",
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/trust",
            Some(serde_json::json!({"cluster_id": "c1", "fingerprint": "AA:BB"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(r.json.to_string().contains("the swarm's pin changed"));
}

#[tokio::test]
async fn rejecting_clears_the_pending_pin() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let (env_id, _key) = swarm_env(&t).await;
    let pki = SwarmPki::new("c1");
    t.store
        .pin_swarm_cluster(
            env_id,
            "c1",
            &pki.root,
            "n1",
            "host-n1",
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/prod/swarm/retrust",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 204);
    assert!(t.store.get_swarm_cluster(env_id).await.unwrap().is_none());
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/prod/swarm/trust",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, 404);
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

    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    // Registered just after its welcome.
    eventually("manager registered", || {
        t.state.agents.nodes(env_id).len() == 1
    })
    .await;
    let (_w, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", false),
        &pki.node("n2", false),
    )
    .await;
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
    trust(&t, env_id, &pki, &daemon).await;
    let m_cert = pki.node("n1", true);
    let (_m, _) = join(&base, &key, &daemon, claim(&pki, "n1", true), &m_cert).await;
    let (_w, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", false),
        &pki.node("n2", false),
    )
    .await;
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
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    daemon.remove("n2");
    // Its certificate is still valid; membership is what refuses it.
    let (_w, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", false),
        &pki.node("n2", false),
    )
    .await;
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
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    // A worker's certificate cannot back a manager claim.
    let (_w, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", true),
        &pki.node("n2", false),
    )
    .await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    assert_eq!(t.state.agents.nodes(env_id).len(), 1);
    assert!(t
        .audit_actions()
        .await
        .contains(&"agent.reject".to_string()));
}

#[tokio::test]
async fn a_proof_from_another_swarm_is_refused() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    // Same ids, another CA.
    let rogue = SwarmPki::new("c1");
    let (_w, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", false),
        &rogue.node("n2", false),
    )
    .await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    // Unproven: the reason is the environment's, not the claimed node's.
    let reason = t.state.agents.offline_reason(env_id).unwrap();
    assert!(reason.contains("root CA"), "{reason}");
}

#[tokio::test]
async fn a_failed_proof_writes_no_node_state() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    // A key holder claims manager n1 with a certificate that cannot back it.
    let rogue = SwarmPki::new("c1");
    let (_r, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &rogue.node("n1", true),
    )
    .await;
    assert_eq!(reply, bye(ByeReason::Incompatible));
    assert_eq!(t.state.agents.node_offline_reason(env_id, "n1"), None);
    let rows = t.store.list_swarm_agents(env_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].last_error, None, "the real n1's row is untouched");
    assert!(
        t.state.agents.get_node(env_id, "n1").is_some(),
        "the real n1 stays"
    );
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
    let mut a = TestAgent::connect_swarm(&base, &token, daemon)
        .await
        .unwrap();
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
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, reply) = join(
        &base,
        &old,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
    let (_w, reply) = join(
        &base,
        &new,
        &daemon,
        claim(&pki, "n2", false),
        &pki.node("n2", false),
    )
    .await;
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
    assert!(
        t.store.get_swarm_cluster(env_id).await.unwrap().is_none(),
        "nothing pinned"
    );
    assert!(t
        .audit_actions()
        .await
        .contains(&"agent.reject".to_string()));
}

async fn known(t: &TestApp, env_id: i64) -> Vec<String> {
    t.store
        .get_swarm_cluster(env_id)
        .await
        .unwrap()
        .unwrap()
        .known_managers
        .unwrap_or_default()
}

#[tokio::test]
async fn a_removed_former_manager_is_refused_after_restart() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", true), ("n3", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    // n2 leaves the swarm; n1's next registration notices.
    daemon.remove("n2");
    drop(m);
    eventually("n1 gone", || t.state.agents.nodes(env_id).is_empty()).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    for _ in 0..100 {
        if known(&t, env_id).await == ["n1"] {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(known(&t, env_id).await, ["n1"]);

    // A restart: n2's agent, still holding a valid manager certificate and
    // a daemon that says it is a manager, is first and alone.
    let t2 = app_on(t.store.clone()).await;
    let base2 = format!("http://{}", t2.serve().await);
    let liar = FakeSwarm::new(&pki, &[("n1", true), ("n2", true)]);
    let (_r, reply) = join(
        &base2,
        &key,
        &liar,
        claim(&pki, "n2", true),
        &pki.node("n2", true),
    )
    .await;
    assert_eq!(reply, bye(ByeReason::NotYet));
    assert!(t2.state.agents.nodes(env_id).is_empty());
    let reason = t2.state.agents.node_offline_reason(env_id, "n2").unwrap();
    assert_eq!(
        reason,
        "waiting for a known manager to confirm new manager n2"
    );
}

#[tokio::test]
async fn a_lone_known_manager_checks_itself_after_restart() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", true)]);
    trust(&t, env_id, &pki, &daemon).await;
    let t2 = app_on(t.store.clone()).await;
    let base2 = format!("http://{}", t2.serve().await);
    let (_m, reply) = join(
        &base2,
        &key,
        &daemon,
        claim(&pki, "n2", true),
        &pki.node("n2", true),
    )
    .await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
}

#[tokio::test]
async fn a_promoted_manager_is_confirmed_by_a_known_one() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n4", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    daemon.set_role("n4", true);
    let (_p, reply) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n4", true),
        &pki.node("n4", true),
    )
    .await;
    assert!(matches!(reply, Some(Msg::Welcome { .. })), "{reply:?}");
    for _ in 0..100 {
        if known(&t, env_id).await.contains(&"n4".to_string()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(known(&t, env_id).await, ["n1", "n4"]);
}

#[tokio::test]
async fn a_demoted_manager_session_is_revoked_when_a_known_manager_refreshes() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", true)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (m1, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    let (mut m2, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n2", true),
        &pki.node("n2", true),
    )
    .await;
    eventually("both", || t.state.agents.nodes(env_id).len() == 2).await;
    daemon.set_role("n2", false);
    drop(m1);
    eventually("n1 gone", || t.state.agents.nodes(env_id).len() == 1).await;
    let (_m1, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    loop {
        match m2.next().await {
            Some(Msg::Ping { n }) => m2.pong(n).await,
            Some(Msg::Bye { reason }) => {
                assert_eq!(reason, ByeReason::Revoked);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn a_swarm_counts_each_node() {
    let t = app().await;
    let (env_id, key) = swarm_env(&t).await;
    // No manager connected: the swarm counts as one.
    assert_eq!(t.state.envs.count_nodes().await, 1);
    let base = base(&t).await;
    let pki = SwarmPki::new("c1");
    let daemon = FakeSwarm::new(&pki, &[("n1", true), ("n2", false), ("n3", false)]);
    trust(&t, env_id, &pki, &daemon).await;
    let (_m, _) = join(
        &base,
        &key,
        &daemon,
        claim(&pki, "n1", true),
        &pki.node("n1", true),
    )
    .await;
    eventually("registered", || t.state.agents.nodes(env_id).len() == 1).await;
    assert_eq!(t.state.envs.count_nodes().await, 3);
}
