//! The kind fixtures written by `scripts/kind-up.sh`, and clients for them.
#![allow(dead_code)]

use fleetops_agent::kube_proxy::{KubeProxy, KubeUpstream};
use std::{path::PathBuf, sync::Arc};

pub const NS: &str = "fleetops-it";

pub fn dir() -> PathBuf {
    let _ = rustls::crypto::ring::default_provider().install_default();
    PathBuf::from(std::env::var("FLEETOPS_KUBE_IT").expect("run through `make test-kube`"))
}

pub fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name))
        .unwrap()
        .trim()
        .to_string()
}

/// A kube-rs client through an in-process tunnel whose agent side is the
/// real kube proxy, using the token file `token`.
pub fn tunnel_client_with(token: &str) -> kube::Client {
    let d = dir();
    let up = KubeUpstream {
        base: read("server").parse().unwrap(),
        ca: d.join("ca.crt"),
        token: d.join(token),
    };
    let proxy = KubeProxy::new(up).unwrap();
    let session = fleetops_tunnel::testing::loopback_with(move |stream| {
        let proxy = proxy.clone();
        tokio::spawn(async move {
            let _ = proxy.serve(stream).await;
        });
    });
    fleetops_kube::tunnel::client(Arc::new(session))
}

pub fn tunnel_client() -> kube::Client {
    tunnel_client_with("token")
}

use fleetops_kube::KubeClient;

/// The client the suite runs against: through the tunnel when
/// FLEETOPS_IT_TUNNEL is set, else straight from kind's kubeconfig.
pub fn ops() -> KubeClient {
    if std::env::var_os("FLEETOPS_IT_TUNNEL").is_some() {
        return KubeClient::new(tunnel_client());
    }
    let p = fleetops_kube::kubeconfig::prepare(&read("kubeconfig"), None).unwrap();
    let config = fleetops_kube::kubeconfig::config(&p.yaml).unwrap();
    KubeClient::new(kube::Client::try_from(config).unwrap())
}

/// Polls `f` for up to 30 s.
pub async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..60 {
        if f().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("timed out waiting for {what}");
}
