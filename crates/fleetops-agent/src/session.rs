//! One connected session: hello, welcome (save a new key first), then
//! answer pings until the server or the network ends it.

use crate::{creds::Keys, kube_proxy::KubeProxy};
use anyhow::anyhow;
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, HostInfo, Msg, Serves, PROTO_MAJOR},
    forward,
    ws::tungstenite_io,
    Role, Session, Target,
};
use std::{sync::Arc, time::Duration};
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

fn host_info(docker: &str, serves: Serves) -> HostInfo {
    HostInfo {
        hostname: std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        docker: docker.into(),
        serves,
        swarm: None,
    }
}

pub async fn run(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    inbound: Inbound,
    target_desc: &str,
    keys: &Keys,
    silence: Duration,
) -> Ended {
    let serves = match &inbound {
        Inbound::Docker(_) => Serves::Docker,
        Inbound::Kube(_) => Serves::Kube,
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
    let ended = control(&session, host_info(target_desc, serves), keys, silence).await;
    session.close().await;
    ended
}

async fn control(session: &Session, host: HostInfo, keys: &Keys, silence: Duration) -> Ended {
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
    match tokio::time::timeout(Duration::from_secs(10), read_msg(&mut rx)).await {
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
        Ok(Ok(Some(Msg::Bye { reason }))) => return bye(reason),
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
            Ok(Some(Msg::Bye { reason })) => return bye(reason),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return Ended::Dropped,
        }
    }
}

fn bye(reason: ByeReason) -> Ended {
    match reason {
        ByeReason::Shutdown | ByeReason::NotYet => Ended::Dropped,
        ByeReason::Revoked => Ended::Fatal(anyhow!(
            "the server revoked this agent's key; re-enroll with a new join command"
        )),
        ByeReason::Replaced => Ended::Fatal(anyhow!(
            "another agent connected with this key; only one may run per environment"
        )),
        ByeReason::Incompatible => Ended::Fatal(anyhow!(
            "this agent's protocol version is not supported by the server; \
             use the image tag from the server's command"
        )),
    }
}
