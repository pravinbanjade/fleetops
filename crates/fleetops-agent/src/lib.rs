//! fleetops-agent: dials out to a FleetOps server and serves its Docker
//! socket through the tunnel (sub-project 2).

pub mod backoff;
pub mod config;
pub mod creds;
mod dial;
pub mod kube_proxy;
mod session;
pub mod swarm;

pub use config::Config;

use anyhow::{anyhow, Context};
use backoff::Backoff;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// A session that stayed up this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);

pub async fn run(config: Config) -> anyhow::Result<()> {
    let (inbound, desc, proxy) = if config.kube {
        let up = match &config.kube_upstream {
            Some(u) => u.clone(),
            None => kube_proxy::KubeUpstream::in_cluster()?,
        };
        let proxy = kube_proxy::KubeProxy::new(up)?;
        proxy.spawn_refresh(kube_proxy::TOKEN_REFRESH);
        (
            session::Inbound::Kube(proxy.clone()),
            "kubernetes".to_string(),
            Some(proxy),
        )
    } else {
        let target = fleetops_tunnel::Target::parse(&config.docker)?;
        (
            session::Inbound::Docker(target),
            config.docker.clone(),
            None,
        )
    };
    let keys = match (&config.key_secret, proxy) {
        (Some(name), Some(proxy)) => {
            let ns = std::env::var("POD_NAMESPACE")
                .context("POD_NAMESPACE must be set to keep the key in a Secret")?;
            creds::Keys::Secret(creds::SecretKeys::new(proxy, ns, name.clone()))
        }
        (Some(_), None) => return Err(anyhow!("--key-secret needs --kube")),
        (None, _) => match &config.key_file {
            Some(file) => creds::Keys::ReadOnly(file.clone()),
            None => creds::Keys::File(config.data.clone()),
        },
    };
    keys.probe()
        .await
        .with_context(|| format!("{} must be writable", keys.describe()))?;
    let tls = Arc::new(dial::tls_config(
        config.ca.as_deref(),
        config.ca_pem.as_deref(),
    )?);
    let docker_target = fleetops_tunnel::Target::parse(&config.docker)?;
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
    // Set once the server rejects the stored key: a key rotated in the UI
    // stays in the data volume, and the new join command must still work.
    let mut key_rejected = false;
    loop {
        let key = keys.load().await?.filter(|_| !key_rejected);
        let bearer = match (&key, &config.join_token) {
            (Some(k), _) => k.clone(),
            (None, Some(j)) => j.clone(),
            (None, None) => {
                return Err(anyhow!(
                    "no agent key in {} and no --join-token",
                    keys.describe()
                ))
            }
        };
        let started = Instant::now();
        // Re-read every time: the node may have been promoted or demoted.
        let node = if config.swarm {
            match swarm::identity(&docker_target).await {
                Ok(claim) => Some(session::SwarmNode {
                    claim,
                    certs: config.swarm_certs.clone(),
                }),
                Err(e @ swarm::IdentityError::NotInSwarm(_)) => return Err(e.into()),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read this node's swarm identity");
                    tokio::time::sleep(backoff.next_delay()).await;
                    continue;
                }
            }
        } else {
            None
        };
        match dial::dial(&config.server, &bearer, tls.clone()).await {
            Ok(ws) => match session::run(
                ws,
                inbound.clone(),
                &desc,
                &keys,
                config.server_silence,
                node.as_ref(),
            )
            .await
            {
                session::Ended::Fatal(e) => return Err(e),
                session::Ended::Dropped => tracing::warn!("disconnected from {}", config.server),
            },
            Err(dial::DialError::Refused(401)) if config.key_file.is_some() => {
                let file = config.key_file.as_deref().expect("checked");
                return Err(anyhow!(
                    "the server rejected the key in {}; update the fleetops-agent-key secret \
                     (see the environment's Rotate page)",
                    file.display()
                ));
            }
            Err(dial::DialError::Refused(401)) if key.is_some() => {
                if config.join_token.is_none() {
                    return Err(anyhow!(
                        "the server rejected this agent's key (revoked or environment deleted); \
                         re-enroll with a new join command"
                    ));
                }
                tracing::info!("stored key rejected; enrolling with the join token");
                key_rejected = true;
                continue;
            }
            Err(dial::DialError::Refused(code)) => {
                tracing::warn!(code, "server refused the connection")
            }
            Err(dial::DialError::Other(e)) => {
                tracing::warn!(error = %e, "cannot reach {}", config.server)
            }
        }
        if started.elapsed() >= STABLE {
            backoff.reset();
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}
