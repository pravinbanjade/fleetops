//! fleetops-agent: dials out to a FleetOps server and serves its Docker
//! socket through the tunnel (sub-project 2).

pub mod backoff;
pub mod config;
pub mod creds;
mod dial;
mod session;

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
    creds::probe_writable(&config.data).with_context(|| {
        format!(
            "the data directory {} must be writable",
            config.data.display()
        )
    })?;
    let target = fleetops_tunnel::Target::parse(&config.docker)?;
    let tls = Arc::new(dial::tls_config(config.ca.as_deref())?);
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
    // Set once the server rejects the stored key: a key rotated in the UI
    // stays in the data volume, and the new join command must still work.
    let mut key_rejected = false;
    loop {
        let key = creds::load_key(&config.data)?.filter(|_| !key_rejected);
        let bearer = match (&key, &config.join_token) {
            (Some(k), _) => k.clone(),
            (None, Some(j)) => j.clone(),
            (None, None) => {
                return Err(anyhow!(
                    "no agent key in {} and no --join-token",
                    config.data.display()
                ))
            }
        };
        let started = Instant::now();
        match dial::dial(&config.server, &bearer, tls.clone()).await {
            Ok(ws) => match session::run(
                ws,
                target.clone(),
                &config.docker,
                &config.data,
                config.server_silence,
            )
            .await
            {
                session::Ended::Fatal(e) => return Err(e),
                session::Ended::Dropped => tracing::warn!("disconnected from {}", config.server),
            },
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
