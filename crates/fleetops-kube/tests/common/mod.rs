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
