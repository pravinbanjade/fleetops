//! Swarm mode (swarm spec §2): the agent learns its node from `docker info`
//! and proves it to the server with the node's Swarm certificate and key.

use anyhow::{anyhow, Context};
use bytes::Bytes;
use fleetops_tunnel::{control::SwarmClaim, swarm_proof, Target};
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use std::path::Path;

pub const NODE_CERT: &str = "swarm-node.crt";
pub const NODE_KEY: &str = "swarm-node.key";

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// Retrying cannot help.
    #[error("this node is not part of an active swarm (Swarm.LocalNodeState is '{0}')")]
    NotInSwarm(String),
    #[error("cannot read docker info: {0:#}")]
    Docker(anyhow::Error),
}

async fn get(target: &Target, path: &str) -> anyhow::Result<serde_json::Value> {
    let req = hyper::Request::get(path)
        .header(hyper::header::HOST, "docker")
        .body(Empty::<Bytes>::new())?;
    let resp = match target {
        Target::Unix(p) => {
            let io = TokioIo::new(tokio::net::UnixStream::connect(p).await?);
            let (mut tx, conn) = hyper::client::conn::http1::handshake(io).await?;
            tokio::spawn(conn);
            tx.send_request(req).await?
        }
        Target::Tcp(addr) => {
            let io = TokioIo::new(tokio::net::TcpStream::connect(addr).await?);
            let (mut tx, conn) = hyper::client::conn::http1::handshake(io).await?;
            tokio::spawn(conn);
            tx.send_request(req).await?
        }
    };
    let status = resp.status();
    let body = resp.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        return Err(anyhow!(
            "GET {path}: HTTP {status}: {}",
            String::from_utf8_lossy(&body)
        ));
    }
    Ok(serde_json::from_slice(&body)?)
}

/// This node's Swarm identity, from `GET /info` on the local engine.
pub async fn identity(docker: &Target) -> Result<SwarmClaim, IdentityError> {
    let info = get(docker, "/info").await.map_err(IdentityError::Docker)?;
    let swarm = &info["Swarm"];
    let state = swarm["LocalNodeState"].as_str().unwrap_or_default();
    if state != "active" {
        return Err(IdentityError::NotInSwarm(state.to_string()));
    }
    Ok(SwarmClaim {
        node_id: swarm["NodeID"].as_str().unwrap_or_default().to_string(),
        cluster_id: swarm["Cluster"]["ID"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        manager: swarm["ControlAvailable"].as_bool().unwrap_or(false),
    })
}

/// The node's certificate chain and a signature of the server's nonce.
/// Read fresh each time: Swarm renews node certificates on its own.
pub fn prove(certs: &Path, nonce: &str, claim: &SwarmClaim) -> anyhow::Result<(String, String)> {
    let chain = std::fs::read_to_string(certs.join(NODE_CERT))
        .with_context(|| format!("reading {}", certs.join(NODE_CERT).display()))?;
    let key = std::fs::read_to_string(certs.join(NODE_KEY))
        .with_context(|| format!("reading {}", certs.join(NODE_KEY).display()))?;
    let signature = swarm_proof::sign(&key, &swarm_proof::proof_message(nonce, claim))?;
    Ok((chain, signature))
}
