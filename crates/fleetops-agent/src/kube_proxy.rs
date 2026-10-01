//! `--kube` mode: each tunnel stream carries plain HTTP/1.1 meant for the
//! kube-apiserver. The agent forwards only Kubernetes API paths, replaces
//! any Authorization header with its own ServiceAccount token, and talks
//! TLS to the apiserver. Upgrades (exec) are spliced end to end.

use anyhow::Context as _;
use bytes::Bytes;
use fleetops_tunnel::StreamHandle;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::{
    convert::Infallible,
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

/// Where every pod finds its ServiceAccount credentials.
pub const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
/// Projected tokens rotate; re-read the file this often.
pub const TOKEN_REFRESH: Duration = Duration::from_secs(60);

type Body = BoxBody<Bytes, hyper::Error>;

/// The apiserver to forward to and the credentials to use.
#[derive(Debug, Clone)]
pub struct KubeUpstream {
    /// e.g. `https://10.96.0.1:443`; no trailing slash.
    pub base: Uri,
    pub ca: PathBuf,
    pub token: PathBuf,
}

impl KubeUpstream {
    /// The apiserver as every pod sees it.
    pub fn in_cluster() -> anyhow::Result<Self> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .context("KUBERNETES_SERVICE_HOST is not set: --kube must run inside a pod")?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        Ok(Self {
            base: format!("https://{host}:{port}").parse()?,
            ca: Path::new(SA_DIR).join("ca.crt"),
            token: Path::new(SA_DIR).join("token"),
        })
    }
}

/// Forwards only what kube-rs sends: the core and group APIs and
/// `/version`. Dot segments are refused so a path can't climb out.
pub fn allowed(path: &str) -> bool {
    let api = path == "/version"
        || path == "/api"
        || path == "/apis"
        || path.starts_with("/api/")
        || path.starts_with("/apis/");
    api && !path.split('/').any(|seg| seg == ".." || seg == ".")
}

pub struct KubeProxy {
    up: KubeUpstream,
    client: Client<HttpsConnector<HttpConnector>, Body>,
    token: RwLock<String>,
}

fn read_token(path: &Path) -> io::Result<String> {
    let t = std::fs::read_to_string(path)?.trim().to_string();
    if t.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token file is empty",
        ));
    }
    Ok(t)
}

fn reply(status: StatusCode, message: &str) -> Response<Body> {
    let body = format!(
        r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":{message:?},"code":{}}}"#,
        status.as_u16()
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            Full::new(Bytes::from(body))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("static response")
}

impl KubeProxy {
    pub fn new(up: KubeUpstream) -> anyhow::Result<Arc<Self>> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(&up.ca)
            .with_context(|| format!("reading the cluster CA {}", up.ca.display()))?
        {
            roots.add(cert?)?;
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(TokioTimer::new())
            .build(https);
        let token = read_token(&up.token)
            .with_context(|| format!("reading the ServiceAccount token {}", up.token.display()))?;
        Ok(Arc::new(Self {
            up,
            client,
            token: RwLock::new(token),
        }))
    }

    /// Re-reads the token file; the old token stays in use on failure.
    pub fn refresh_token(&self) -> io::Result<()> {
        let t = read_token(&self.up.token)?;
        *self.token.write().unwrap() = t;
        Ok(())
    }

    pub fn spawn_refresh(self: &Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = me.refresh_token() {
                    tracing::warn!(error = %e, "cannot re-read the ServiceAccount token");
                }
            }
        })
    }

    /// Serves one tunnel stream: HTTP/1.1 requests, keep-alive, upgrades.
    pub async fn serve(self: Arc<Self>, stream: StreamHandle) -> Result<(), hyper::Error> {
        let svc = hyper::service::service_fn(move |req| {
            let me = self.clone();
            async move { Ok::<_, Infallible>(me.handle(req).await) }
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), svc)
            .with_upgrades()
            .await
    }

    async fn handle(&self, mut req: Request<Incoming>) -> Response<Body> {
        if !allowed(req.uri().path()) {
            return reply(
                StatusCode::FORBIDDEN,
                "fleetops-agent forwards only Kubernetes API paths",
            );
        }
        let pq = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let uri = format!("{}{pq}", self.up.base.to_string().trim_end_matches('/'));
        match uri.parse() {
            Ok(u) => *req.uri_mut() = u,
            Err(_) => return reply(StatusCode::BAD_REQUEST, "bad request path"),
        }
        let token = self.token.read().unwrap().clone();
        let headers = req.headers_mut();
        headers.remove(header::HOST);
        headers.remove(header::AUTHORIZATION);
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("token is a header value"),
        );
        let client_upgrade = req
            .headers()
            .contains_key(header::UPGRADE)
            .then(|| hyper::upgrade::on(&mut req));
        let req = req.map(|b| b.boxed());
        let mut resp = match self.client.request(req).await {
            Ok(r) => r,
            Err(e) => {
                return reply(
                    StatusCode::BAD_GATEWAY,
                    &format!("kube-apiserver unreachable: {e}"),
                )
            }
        };
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_up) = client_upgrade {
                let server_up = hyper::upgrade::on(&mut resp);
                tokio::spawn(async move {
                    match tokio::try_join!(client_up, server_up) {
                        Ok((a, b)) => {
                            let _ = tokio::io::copy_bidirectional(
                                &mut TokioIo::new(a),
                                &mut TokioIo::new(b),
                            )
                            .await;
                        }
                        Err(e) => tracing::debug!(error = %e, "upgrade splice failed"),
                    }
                });
            }
        }
        resp.map(|b| b.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn only_kubernetes_api_paths_are_forwarded() {
        for ok in [
            "/version",
            "/api",
            "/api/v1/pods",
            "/apis/apps/v1/namespaces/x/deployments",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "/",
            "/healthz",
            "/metrics",
            "/logs/x",
            "/api/../metrics",
            "/apis/./x",
            "/apix",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }
}
