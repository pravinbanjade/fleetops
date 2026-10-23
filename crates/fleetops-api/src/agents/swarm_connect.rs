//! The Swarm half of the connect handler (swarm spec §2): challenge the
//! agent, check its node certificate against the swarm's root CA (pinned on
//! the first manager), check membership through a manager, then register
//! the session under its node.

use super::connect::{control_loop, disconnect_audit_due, say_bye, Control, HANDSHAKE};
use super::publish_status;
use crate::{auth::token::generate_token, routes::audit, AppState};
use fleetops_core::{now_unix, swarm::NodeRole, swarm::SwarmOps};
use fleetops_store::{EnvironmentRecord, SwarmAgentRecord};
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, Msg, Serves, SwarmClaim},
    registry::AgentInfo,
    swarm_proof, Session,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// How long the server waits on a swarm call during the handshake.
const SWARM_CALL: std::time::Duration = std::time::Duration::from_secs(10);

/// Refuses the agent: says bye, remembers why (per node once the node is
/// known), and audits it. `NotYet` is routine while a swarm starts, so it
/// is neither audited nor stored as an error.
async fn refuse(
    s: &AppState,
    env: &EnvironmentRecord,
    c: &mut Control,
    session: &Session,
    info: &AgentInfo,
    reason: String,
    bye: ByeReason,
) -> String {
    let node = info.host.swarm.as_ref().map(|n| n.node_id.as_str());
    match node {
        Some(node) => s.agents.set_node_offline_reason(env.id, node, &reason),
        None => s.agents.set_offline_reason(env.id, &reason),
    }
    if bye != ByeReason::NotYet {
        match (node, &info.host.swarm) {
            (Some(node), Some(claim)) => {
                let _ = s
                    .store
                    .record_swarm_agent(
                        env.id,
                        &SwarmAgentRecord {
                            node_id: node.to_string(),
                            hostname: Some(info.host.hostname.clone()),
                            manager: claim.manager,
                            agent_version: Some(info.agent_version.clone()),
                            last_seen_at: Some(now_unix()),
                            last_error: Some(reason.clone()),
                        },
                    )
                    .await;
            }
            _ => {
                let _ = s
                    .store
                    .record_agent_seen(
                        env.id,
                        now_unix(),
                        Some(&info.agent_version),
                        None,
                        Some(&reason),
                    )
                    .await;
            }
        }
        audit(s, None, "agent.reject", "environment", &env.name, "failure").await;
    }
    // Last: the agent hangs up on reading it, and whoever watches the
    // environment should already see why.
    say_bye(c, session, bye).await;
    reason
}

/// A registered manager's session other than `except`'s, for checks.
fn manager_session(s: &AppState, env_id: i64, except: &str) -> Option<Session> {
    s.agents
        .nodes(env_id)
        .into_iter()
        .find(|(n, l)| n != except && l.info.host.swarm.as_ref().is_some_and(|c| c.manager))
        .map(|(_, l)| l.session)
}

async fn call<T>(
    f: impl std::future::Future<Output = fleetops_core::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(SWARM_CALL, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {SWARM_CALL:?}")),
    }
}

pub(super) async fn serve(
    s: &AppState,
    env: &EnvironmentRecord,
    key_hash: String,
    c: &mut Control,
    session: &Session,
    info: AgentInfo,
) -> String {
    let claim: SwarmClaim = match (&info.host.serves, &info.host.swarm) {
        (Serves::Swarm, Some(claim)) => claim.clone(),
        (serves, _) => {
            let reason = format!(
                "agent serves {}; this environment expects swarm",
                serves.as_str()
            );
            return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
        }
    };
    let Some(through) = s.swarm_through.clone() else {
        let reason = "this server cannot reach Swarm agents".to_string();
        return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
    };
    let pinned = match s.store.get_swarm_cluster(env.id).await {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    let checker = manager_session(s, env.id, &claim.node_id);

    // Workers wait for a manager: it pins the swarm and checks membership.
    if !claim.manager && (pinned.is_none() || checker.is_none()) {
        let reason = "waiting for a manager's agent to connect".to_string();
        return refuse(s, env, c, session, &info, reason, ByeReason::NotYet).await;
    }
    if let Some(p) = &pinned {
        if p.cluster_id != claim.cluster_id {
            let reason = format!(
                "this agent belongs to swarm {}; the environment is pinned to {}",
                claim.cluster_id, p.cluster_id
            );
            return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
        }
    }

    // The proof.
    let nonce = generate_token();
    if let Err(e) = write_msg(&mut c.tx, &Msg::Challenge { nonce: nonce.clone() }).await {
        return e.to_string();
    }
    let (chain, signature) = match tokio::time::timeout(HANDSHAKE, read_msg(&mut c.rx)).await {
        Ok(Ok(Some(Msg::Proof {
            cert_chain_pem,
            signature,
        }))) => (cert_chain_pem, signature),
        other => return format!("expected a proof, got {other:?}"),
    };
    let own: Arc<dyn SwarmOps> = match through(session.clone()) {
        Ok(o) => o,
        Err(e) => return e.to_string(),
    };
    let root = match &pinned {
        Some(p) => p.trust_root.clone(),
        None => match call(own.swarm_info()).await {
            Ok(si) if si.cluster_id == claim.cluster_id => si.trust_root,
            Ok(si) => {
                let reason = format!(
                    "the node reports swarm {} but claims {}",
                    si.cluster_id, claim.cluster_id
                );
                return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
            }
            Err(e) => {
                let reason = format!("cannot read the swarm's root CA: {e}");
                return refuse(s, env, c, session, &info, reason, ByeReason::NotYet).await;
            }
        },
    };
    let msg = swarm_proof::proof_message(&nonce, &claim);
    if let Err(e) = swarm_proof::verify(&root, &chain, &claim, &msg, &signature, now_unix()) {
        return refuse(s, env, c, session, &info, e.to_string(), ByeReason::Incompatible).await;
    }
    if pinned.is_none() {
        match s
            .store
            .pin_swarm_cluster(env.id, &claim.cluster_id, &root, now_unix())
            .await
        {
            // Another manager pinned first; it must be the same swarm.
            Ok(p) if p.cluster_id != claim.cluster_id || p.trust_root != root => {
                let reason = format!("the environment is pinned to swarm {}", p.cluster_id);
                return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
            }
            Ok(_) => audit(s, None, "swarm.pin", "environment", &env.name, "success").await,
            Err(e) => return e.to_string(),
        }
    }

    // Membership: a removed node's certificate stays valid until it expires.
    let checker: Arc<dyn SwarmOps> = match checker.map(|sess| through(sess)) {
        Some(Ok(o)) => o,
        Some(Err(e)) => return e.to_string(),
        None => own,
    };
    match call(checker.node(&claim.node_id)).await {
        Ok(Some(n)) if (n.role == NodeRole::Manager) == claim.manager => {}
        Ok(Some(n)) => {
            let role = match n.role {
                NodeRole::Manager => "manager",
                NodeRole::Worker => "worker",
            };
            let reason = format!("node {} is a {role} in this swarm", claim.node_id);
            return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
        }
        Ok(None) => {
            let reason = format!("node {} is not a member of this swarm", claim.node_id);
            return refuse(s, env, c, session, &info, reason, ByeReason::Incompatible).await;
        }
        Err(e) => {
            let reason = format!("cannot check swarm membership: {e}");
            return refuse(s, env, c, session, &info, reason, ByeReason::NotYet).await;
        }
    }

    let welcome = Msg::Welcome {
        env_name: env.name.clone(),
        server_version: env!("CARGO_PKG_VERSION").into(),
        agent_key: None,
    };
    if let Err(e) = write_msg(&mut c.tx, &welcome).await {
        return e.to_string();
    }
    let node = claim.node_id.clone();
    let (bye_tx, mut bye_rx) = mpsc::channel(1);
    let id = s
        .agents
        .insert_node(env.id, &node, &key_hash, session.clone(), info.clone(), bye_tx);
    // As for Docker agents: a rotation or delete during the handshake found
    // no session to revoke.
    if s.store
        .swarm_by_key(&key_hash, now_unix())
        .await
        .ok()
        .flatten()
        .map(|(e, _)| e)
        != Some(env.id)
    {
        s.agents.remove_node(env.id, &node, id, "key revoked");
        say_bye(c, session, ByeReason::Revoked).await;
        return "key revoked during the handshake".into();
    }
    let record = |last_error: Option<String>| SwarmAgentRecord {
        node_id: node.clone(),
        hostname: Some(info.host.hostname.clone()),
        manager: claim.manager,
        agent_version: Some(info.agent_version.clone()),
        last_seen_at: Some(now_unix()),
        last_error,
    };
    let _ = s.store.record_swarm_agent(env.id, &record(None)).await;
    let _ = s
        .store
        .record_agent_seen(env.id, now_unix(), Some(&info.agent_version), None, None)
        .await;
    let target = format!("{}/{}", env.name, info.host.hostname);
    audit(s, None, "agent.connect", "environment", &target, "success").await;
    publish_status(s, env.id).await;

    let reason = control_loop(s, env.id, c, session, &mut bye_rx).await;

    s.agents.remove_node(env.id, &node, id, &reason);
    let _ = s.store.record_swarm_agent(env.id, &record(None)).await;
    if disconnect_audit_due(s, env.id, &node) {
        audit(s, None, "agent.disconnect", "environment", &target, "success").await;
    }
    reason
}
