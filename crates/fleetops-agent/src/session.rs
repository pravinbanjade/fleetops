//! One connected session: hello, welcome (save a new key first), then
//! answer pings until the server or the network ends it.

use crate::creds;
use anyhow::anyhow;
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, HostInfo, Msg, PROTO_MAJOR},
    forward,
    ws::tungstenite_io,
    Role, Session, Target,
};
use std::{path::Path, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub enum Ended {
    /// The network or server went away; reconnect.
    Dropped,
    /// Retrying cannot help.
    Fatal(anyhow::Error),
}

fn host_info(docker: &str) -> HostInfo {
    HostInfo {
        hostname: std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        docker: docker.into(),
    }
}

pub async fn run(
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    target: Target,
    docker_url: &str,
    data: &Path,
) -> Ended {
    let session = Session::spawn(tungstenite_io(ws), Role::Client, move |stream| {
        let target = target.clone();
        tokio::spawn(async move {
            if let Err(e) = forward(stream, &target).await {
                tracing::debug!(error = %e, "stream to Docker ended");
            }
        });
    });
    let ended = control(&session, docker_url, data).await;
    session.close().await;
    ended
}

async fn control(session: &Session, docker_url: &str, data: &Path) -> Ended {
    let Ok(stream) = session.open_stream().await else {
        return Ended::Dropped;
    };
    let (mut rx, mut tx) = tokio::io::split(stream);
    let hello = Msg::Hello {
        proto: PROTO_MAJOR,
        agent_version: env!("CARGO_PKG_VERSION").into(),
        host: host_info(docker_url),
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
                if let Err(e) = creds::save_key(data, &key) {
                    return Ended::Fatal(anyhow!(
                        "enrolled, but could not save the key in {}: {e}; \
                         rotate the agent in the UI and start again",
                        data.display()
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
        match read_msg(&mut rx).await {
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
        ByeReason::Shutdown => Ended::Dropped,
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
