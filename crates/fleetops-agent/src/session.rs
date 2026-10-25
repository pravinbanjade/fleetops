//! One connected session: hello, welcome (save a new key first), then
//! answer pings until the server or the network ends it.

use crate::{creds::Keys, kube_proxy::KubeProxy};
use anyhow::anyhow;
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, HostInfo, Msg, Serves, SwarmClaim, PROTO_MAJOR},
    forward,
    ws::tungstenite_io,
    Role, Session, Target,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub enum Ended {
    /// The network or server went away; reconnect.
    Dropped,
    /// Retrying cannot help.
    Fatal(anyhow::Error),
}

/// Where the agent sends the server's streams.
#[derive(Clone)]
pub enum Inbound {
    Docker(Target),
    Kube(Arc<KubeProxy>),
}

/// A Swarm agent's node, and where its certificates are.
#[derive(Clone)]
pub struct SwarmNode {
    pub claim: SwarmClaim,
    pub certs: PathBuf,
}

fn host_info(docker: &str, serves: Serves, swarm: Option<SwarmClaim>) -> HostInfo {
    HostInfo {
        hostname: std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        docker: docker.into(),
        serves,
        swarm,
    }
}

pub async fn run(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    inbound: Inbound,
    target_desc: &str,
    keys: &Keys,
    silence: Duration,
    swarm: Option<&SwarmNode>,
) -> Ended {
    let serves = match (&inbound, swarm) {
        (Inbound::Docker(_), Some(_)) => Serves::Swarm,
        (Inbound::Docker(_), None) => Serves::Docker,
        (Inbound::Kube(_), _) => Serves::Kube,
    };
    let session = Session::spawn(
        tungstenite_io(ws),
        Role::Client,
        move |stream| match inbound.clone() {
            Inbound::Docker(target) => {
                tokio::spawn(async move {
                    if let Err(e) = forward(stream, &target).await {
                        tracing::debug!(error = %e, "stream to Docker ended");
                    }
                });
            }
            Inbound::Kube(proxy) => {
                tokio::spawn(async move {
                    if let Err(e) = proxy.serve(stream).await {
                        tracing::debug!(error = %e, "stream to the apiserver ended");
                    }
                });
            }
        },
    );
    let host = host_info(target_desc, serves, swarm.map(|n| n.claim.clone()));
    let ended = control(&session, host, keys, silence, swarm).await;
    session.close().await;
    ended
}

async fn control(
    session: &Session,
    host: HostInfo,
    keys: &Keys,
    silence: Duration,
    swarm: Option<&SwarmNode>,
) -> Ended {
    let Ok(stream) = session.open_stream().await else {
        return Ended::Dropped;
    };
    let (mut rx, mut tx) = tokio::io::split(stream);
    let hello = Msg::Hello {
        proto: PROTO_MAJOR,
        agent_version: env!("CARGO_PKG_VERSION").into(),
        host,
    };
    if write_msg(&mut tx, &hello).await.is_err() {
        return Ended::Dropped;
    }
    let mut reply = tokio::time::timeout(Duration::from_secs(10), read_msg(&mut rx)).await;
    // A Swarm agent proves its node before the welcome.
    if let (Some(node), Ok(Ok(Some(Msg::Challenge { nonce })))) = (swarm, &reply) {
        let (cert_chain_pem, signature) = match crate::swarm::prove(&node.certs, nonce, &node.claim)
        {
            Ok(p) => p,
            Err(e) => {
                // Said to the server too, so the admin sees why.
                let why = Msg::CannotProve {
                    reason: format!("{e:#}"),
                };
                if write_msg(&mut tx, &why).await.is_ok() {
                    // Its bye says the server has it.
                    let _ = tokio::time::timeout(Duration::from_secs(10), read_msg(&mut rx)).await;
                }
                return Ended::Fatal(e.context("cannot prove this node's identity"));
            }
        };
        let proof = Msg::Proof {
            cert_chain_pem,
            signature,
        };
        if write_msg(&mut tx, &proof).await.is_err() {
            return Ended::Dropped;
        }
        reply = tokio::time::timeout(Duration::from_secs(10), read_msg(&mut rx)).await;
    }
    match reply {
        Ok(Ok(Some(Msg::Welcome {
            env_name,
            agent_key,
            ..
        }))) => {
            if let Some(key) = agent_key {
                if let Err(e) = keys.save(&key).await {
                    return Ended::Fatal(anyhow!(
                        "enrolled, but could not save the key in {}: {e}; \
                         rotate the agent in the UI and start again",
                        keys.describe()
                    ));
                }
                tracing::info!(env = %env_name, "enrolled; key saved");
            }
            tracing::info!(env = %env_name, "connected");
        }
        Ok(Ok(Some(Msg::Bye { reason }))) => return bye(reason, swarm.is_some()),
        _ => return Ended::Dropped,
    }
    loop {
        // The server pings every 15 s. Silence longer than `silence` means
        // the connection is dead even if TCP has not noticed (a crashed
        // server, a NAT that dropped the mapping): dial again.
        let Ok(msg) = tokio::time::timeout(silence, read_msg(&mut rx)).await else {
            tracing::warn!("no word from the server for {silence:?}; reconnecting");
            return Ended::Dropped;
        };
        match msg {
            Ok(Some(Msg::Ping { n })) => {
                if write_msg(&mut tx, &Msg::Pong { n }).await.is_err() {
                    return Ended::Dropped;
                }
            }
            Ok(Some(Msg::Bye { reason })) => return bye(reason, swarm.is_some()),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return Ended::Dropped,
        }
    }
}

/// What the server's bye means here; a Swarm agent is one of many, one
/// per node, all on the environment's key.
fn bye(reason: ByeReason, swarm: bool) -> Ended {
    match (reason, swarm) {
        (ByeReason::Shutdown, _) => Ended::Dropped,
        (ByeReason::NotYet, _) => {
            tracing::info!("the server is not ready for this node yet; retrying");
            Ended::Dropped
        }
        (ByeReason::Revoked, false) => Ended::Fatal(anyhow!(
            "the server revoked this agent's key; re-enroll with a new join command"
        )),
        (ByeReason::Revoked, true) => Ended::Fatal(anyhow!(
            "the server no longer accepts this key (rotated, re-trusted or removed); \
             deploy the agent stack from the environment's key dialog again"
        )),
        (ByeReason::Replaced, false) => Ended::Fatal(anyhow!(
            "another agent connected with this key; only one may run per environment"
        )),
        (ByeReason::Replaced, true) => Ended::Fatal(anyhow!(
            "another agent connected for this node; only one may run per node"
        )),
        (ByeReason::Incompatible, false) => Ended::Fatal(anyhow!(
            "this agent's protocol version is not supported by the server; \
             use the image tag from the server's command"
        )),
        (ByeReason::Incompatible, true) => Ended::Fatal(anyhow!(
            "the server refused this node; the environment's Nodes page says why"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fatal(e: Ended) -> String {
        match e {
            Ended::Fatal(e) => e.to_string(),
            _ => panic!("not fatal"),
        }
    }

    /// A Swarm environment has an agent per node, one key for all, and
    /// refusals the Nodes page explains.
    #[test]
    fn swarm_byes_speak_of_nodes() {
        let replaced = fatal(bye(ByeReason::Replaced, true));
        assert!(replaced.contains("for this node"), "{replaced}");
        let revoked = fatal(bye(ByeReason::Revoked, true));
        assert!(revoked.contains("agent stack"), "{revoked}");
        let refused = fatal(bye(ByeReason::Incompatible, true));
        assert!(refused.contains("Nodes page"), "{refused}");
        let docker = fatal(bye(ByeReason::Replaced, false));
        assert!(docker.contains("per environment"), "{docker}");
    }
}
