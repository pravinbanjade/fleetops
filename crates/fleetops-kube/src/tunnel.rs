//! kube-rs over an agent tunnel. Each HTTP connection is one yamux stream;
//! the agent adds the ServiceAccount token and TLS, so this side speaks
//! plain HTTP with no credentials (spec §4.3).

use fleetops_tunnel::{http::TunnelConnector, StreamOpener};
use hyper::Uri;
use hyper_util::{
    client::legacy::Client,
    rt::{TokioExecutor, TokioTimer},
};
use std::{sync::Arc, time::Duration};

/// Idle pooled streams close after this, as for Docker agents.
const POOL_IDLE: Duration = Duration::from_secs(30);
/// Never dialled: the connector ignores it. kube-rs needs an absolute base.
const BASE: &str = "http://kube-agent";

pub fn client(opener: Arc<dyn StreamOpener>) -> kube::Client {
    let http = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE)
        .pool_timer(TokioTimer::new())
        .build(TunnelConnector::new(opener));
    let service = tower::ServiceBuilder::new()
        .layer(kube::client::middleware::BaseUriLayer::new(
            Uri::from_static(BASE),
        ))
        .service(http);
    kube::Client::new(service, "default")
}
