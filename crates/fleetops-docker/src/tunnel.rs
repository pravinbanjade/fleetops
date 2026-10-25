//! bollard over an agent tunnel: a hyper client whose connector opens a
//! yamux stream instead of a socket. Upgrades (exec, attach) work because
//! hyper owns the connection and the stream is a plain byte pipe.

use crate::LocalDocker;
use bollard::{BollardRequest, Docker, API_DEFAULT_VERSION};
use fleetops_core::{Error, Result};
use fleetops_tunnel::{http::TunnelConnector, StreamOpener};
use hyper_util::{
    client::legacy::Client,
    rt::{TokioExecutor, TokioTimer},
};
use std::{sync::Arc, time::Duration};

/// Seconds bollard waits for response headers (its own default).
const REQUEST_TIMEOUT_SECS: u64 = 120;
/// Idle pooled streams close after this (spec §2.4).
const POOL_IDLE: Duration = Duration::from_secs(30);

impl LocalDocker {
    /// A client for the Docker endpoint at the far end of an agent tunnel.
    pub fn through(opener: Arc<dyn StreamOpener>) -> Result<Self> {
        let raw = Some(crate::raw::tunnel(opener.clone()));
        let client = Arc::new(
            Client::builder(TokioExecutor::new())
                .pool_idle_timeout(POOL_IDLE)
                .pool_timer(TokioTimer::new())
                .build(TunnelConnector::new(opener)),
        );
        let client = Docker::connect_with_custom_transport(
            move |req: BollardRequest| {
                let client = client.clone();
                async move {
                    client
                        .request(req)
                        .await
                        .map_err(bollard::errors::Error::from)
                }
            },
            Some("http://agent"),
            REQUEST_TIMEOUT_SECS,
            API_DEFAULT_VERSION,
        )
        .map_err(|e| Error::EnvUnavailable(format!("agent transport: {e}")))?;
        Ok(Self { client, raw })
    }
}

#[cfg(test)]
mod tests {
    use crate::LocalDocker;
    use fleetops_core::docker::SystemOps;
    use fleetops_tunnel::{testing::loopback, Target};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-endpoint Docker stand-in: answers GET /version on a unix socket.
    fn fake_daemon(path: std::path::PathBuf) {
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            loop {
                let (mut c, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = c.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    assert!(req.contains("/version"), "{req}");
                    let body =
                        r#"{"Version":"27.0.0","ApiVersion":"1.47","Os":"linux","Arch":"arm64"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    c.write_all(resp.as_bytes()).await.unwrap();
                });
            }
        });
    }

    #[tokio::test]
    async fn bollard_requests_travel_through_the_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("docker.sock");
        fake_daemon(sock.clone());
        let docker = LocalDocker::through(Arc::new(loopback(Target::Unix(sock)))).unwrap();
        let v = docker.version().await.unwrap();
        assert_eq!(v.version, "27.0.0");
        assert_eq!(v.arch, "arm64");
    }

    #[tokio::test]
    async fn a_closed_tunnel_is_env_unavailable() {
        let session = loopback(Target::Unix("/nonexistent.sock".into()));
        session.close().await;
        let docker = LocalDocker::through(Arc::new(session)).unwrap();
        assert!(matches!(
            docker.version().await,
            Err(fleetops_core::Error::EnvUnavailable(_))
        ));
    }
}
