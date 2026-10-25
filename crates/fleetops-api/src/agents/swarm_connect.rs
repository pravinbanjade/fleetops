//! The Swarm half of the connect handler (swarm spec §2): challenge the
//! agent, check its node certificate against the swarm's root CA (proposed
//! by the first manager, trusted by an admin), check membership through a
//! manager, then register the session under its node.

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

/// Refuses the agent: says bye, remembers why, and audits it. The reason
/// is kept per node only once the agent has `proven` it runs on that node;
/// before that the node id is only a claim, so it goes on the environment.
/// `NotYet` is routine while a swarm starts, so it is neither audited nor
/// stored as an error.
#[allow(clippy::too_many_arguments)]
async fn refuse(
    s: &AppState,
    env: &EnvironmentRecord,
    c: &mut Control,
    session: &Session,
    info: &AgentInfo,
    reason: String,
    bye: ByeReason,
    proven: bool,
) -> String {
    let node = info
        .host
        .swarm
        .as_ref()
        .filter(|_| proven)
        .map(|n| n.node_id.as_str());
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

/// A registered known manager's session other than `except`'s: the only
/// sessions whose word on membership counts.
fn known_manager_session(
    s: &AppState,
    env_id: i64,
    known: Option<&[String]>,
    except: &str,
) -> Option<Session> {
    let known = known?;
    s.agents
        .nodes(env_id)
        .into_iter()
        .find(|(n, l)| {
            n != except
                && known.contains(n)
                && l.info.host.swarm.as_ref().is_some_and(|c| c.manager)
        })
        .map(|(_, l)| l.session)
}

/// Records the swarm's managers as `ops` reports them, and revokes any
/// registered manager session whose node is a manager no longer.
async fn refresh_known_managers(s: &AppState, env_id: i64, ops: &dyn SwarmOps) {
    let Ok(nodes) = call(ops.nodes()).await else {
        return;
    };
    let managers: Vec<String> = nodes
        .into_iter()
        .filter(|n| n.role == NodeRole::Manager)
        .map(|n| n.id)
        .collect();
    if s.store.set_known_managers(env_id, &managers).await.is_err() {
        return;
    }
    for (node, live) in s.agents.nodes(env_id) {
        let claims_manager = live.info.host.swarm.as_ref().is_some_and(|c| c.manager);
        if claims_manager && !managers.contains(&node) {
            s.agents.revoke_node(env_id, &node);
        }
    }
}

/// How often a connected manager refreshes the known managers, so a node
/// removed or demoted while FleetOps runs drops off without anyone
/// reconnecting.
fn refresh_every(s: &AppState) -> std::time::Duration {
    s.config.agent_ping_interval * 4
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
    // Refuses with `reason`; `proven` once the proof has passed.
    macro_rules! no {
        ($reason:expr, $bye:ident, $proven:expr) => {
            return refuse(s, env, c, session, &info, $reason, ByeReason::$bye, $proven).await
        };
    }
    let claim: SwarmClaim = match (&info.host.serves, &info.host.swarm) {
        (Serves::Swarm, Some(claim)) => claim.clone(),
        (serves, _) => no!(
            format!(
                "agent serves {}; this environment expects swarm",
                serves.as_str()
            ),
            Incompatible,
            false
        ),
    };
    let Some(through) = s.swarm_through.clone() else {
        no!(
            "this server cannot reach Swarm agents".into(),
            Incompatible,
            false
        )
    };
    let pinned = match s.store.get_swarm_cluster(env.id).await {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    let confirmed = pinned.as_ref().is_some_and(|p| p.confirmed_at.is_some());
    let known = pinned.as_ref().and_then(|p| p.known_managers.clone());
    let checker = known_manager_session(s, env.id, known.as_deref(), &claim.node_id);

    // Workers wait for a manager: it proposes the swarm, an admin trusts
    // it, and a manager checks membership.
    if !claim.manager && (!confirmed || checker.is_none()) {
        no!(
            "waiting for a manager's agent to connect".into(),
            NotYet,
            false
        )
    }
    if let Some(p) = &pinned {
        if p.cluster_id != claim.cluster_id {
            no!(
                format!(
                    "this agent belongs to swarm {}; the environment is pinned to {}",
                    claim.cluster_id, p.cluster_id
                ),
                Incompatible,
                false
            )
        }
    }

    // The proof.
    let nonce = generate_token();
    let challenge = Msg::Challenge {
        nonce: nonce.clone(),
    };
    if let Err(e) = write_msg(&mut c.tx, &challenge).await {
        return e.to_string();
    }
    let (chain, signature) = match tokio::time::timeout(HANDSHAKE, read_msg(&mut c.rx)).await {
        Ok(Ok(Some(Msg::Proof {
            cert_chain_pem,
            signature,
        }))) => (cert_chain_pem, signature),
        Ok(Ok(Some(Msg::CannotProve { reason }))) => no!(
            format!("node {} cannot prove its identity: {reason}", claim.node_id),
            Incompatible,
            false
        ),
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
            Ok(si) => no!(
                format!(
                    "the node reports swarm {} but claims {}",
                    si.cluster_id, claim.cluster_id
                ),
                Incompatible,
                false
            ),
            // A worker's engine can't read the swarm: its claim is false.
            Err(e) if e.contains("not a swarm manager") => no!(
                format!(
                    "node {} claims to be a manager, but its engine is not one",
                    claim.node_id
                ),
                Incompatible,
                false
            ),
            Err(e) => no!(
                format!("cannot read the swarm's root CA: {e}"),
                NotYet,
                false
            ),
        },
    };
    let msg = swarm_proof::proof_message(&nonce, &claim);
    if let Err(e) = swarm_proof::verify(&root, &chain, &claim, &msg, &signature, now_unix()) {
        no!(e.to_string(), Incompatible, false)
    }

    // A proven manager proposes the swarm; an admin must trust it.
    if pinned.is_none() {
        match s
            .store
            .pin_swarm_cluster(
                env.id,
                &claim.cluster_id,
                &root,
                &claim.node_id,
                &info.host.hostname,
                now_unix(),
            )
            .await
        {
            // Another manager proposed first; it must be the same swarm.
            Ok(p) if p.cluster_id != claim.cluster_id || p.trust_root != root => no!(
                format!("the environment is pinned to swarm {}", p.cluster_id),
                Incompatible,
                true
            ),
            Ok(_) => audit(s, None, "swarm.pin", "environment", &env.name, "success").await,
            Err(e) => return e.to_string(),
        }
    }
    if !confirmed {
        let fp = swarm_proof::fingerprint(&root).unwrap_or_default();
        no!(
            format!(
                "waiting for an admin to trust swarm {} (root CA {fp})",
                claim.cluster_id
            ),
            NotYet,
            true
        )
    }

    // Membership: a removed node's certificate stays valid until it
    // expires, so a known manager must vouch for the node. A manager checks
    // itself only when it is known (or right after confirmation, when none
    // is known yet) and no other known manager is connected.
    let self_check = checker.is_none();
    if self_check && known.as_ref().is_some_and(|k| !k.contains(&claim.node_id)) {
        no!(
            format!(
                "waiting for a known manager to confirm new manager {}",
                claim.node_id
            ),
            NotYet,
            true
        )
    }
    // Right after an admin trusted the swarm no manager is known yet: only
    // the manager that proposed it may vouch for itself.
    let proposer = pinned.as_ref().and_then(|p| p.pinned_node.as_deref());
    if self_check && known.is_none() && proposer != Some(claim.node_id.as_str()) {
        no!(
            format!(
                "waiting for the manager that proposed this swarm ({}) to connect",
                pinned
                    .as_ref()
                    .and_then(|p| p.pinned_by.as_deref())
                    .unwrap_or("unknown")
            ),
            NotYet,
            true
        )
    }
    let checker: Arc<dyn SwarmOps> = match checker.map(|sess| through(sess)) {
        Some(Ok(o)) => o,
        Some(Err(e)) => return e.to_string(),
        None => own.clone(),
    };
    match call(checker.node(&claim.node_id)).await {
        Ok(Some(n)) if (n.role == NodeRole::Manager) == claim.manager => {}
        Ok(Some(n)) => {
            let role = match n.role {
                NodeRole::Manager => "manager",
                NodeRole::Worker => "worker",
            };
            no!(
                format!("node {} is a {role} in this swarm", claim.node_id),
                Incompatible,
                true
            )
        }
        Ok(None) => no!(
            format!("node {} is not a member of this swarm", claim.node_id),
            Incompatible,
            true
        ),
        Err(e) => no!(format!("cannot check swarm membership: {e}"), NotYet, true),
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
    let id = s.agents.insert_node(
        env.id,
        &node,
        &key_hash,
        session.clone(),
        info.clone(),
        bye_tx,
    );
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
    // Likewise a re-trust: the root the proof was checked against must
    // still be the trusted one.
    let still = s.store.get_swarm_cluster(env.id).await.ok().flatten();
    if !still.is_some_and(|p| {
        p.confirmed_at.is_some() && p.cluster_id == claim.cluster_id && p.trust_root == root
    }) {
        s.agents.remove_node(env.id, &node, id, "swarm re-trusted");
        say_bye(c, session, ByeReason::Revoked).await;
        return "the swarm was re-trusted during the handshake".into();
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
    // A manager vouched for (or a known one checking itself) refreshes the
    // known managers.
    if claim.manager {
        refresh_known_managers(s, env.id, checker.as_ref()).await;
    }
    publish_status(s, env.id).await;

    // A manager keeps the known managers current while it stays connected.
    let refresher = claim.manager.then(|| {
        let (s, env_id, ops, every) = (s.clone(), env.id, own.clone(), refresh_every(s));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                refresh_known_managers(&s, env_id, ops.as_ref()).await;
            }
        })
    });
    let reason = control_loop(s, env.id, c, session, &mut bye_rx, Some(&key_hash)).await;
    if let Some(r) = refresher {
        r.abort();
    }

    // A session another one replaced leaves the row to the newer one.
    if s.agents.remove_node(env.id, &node, id, &reason) {
        let _ = s.store.record_swarm_agent(env.id, &record(None)).await;
    }
    if disconnect_audit_due(s, env.id, &node) {
        audit(
            s,
            None,
            "agent.disconnect",
            "environment",
            &target,
            "success",
        )
        .await;
    }
    reason
}
