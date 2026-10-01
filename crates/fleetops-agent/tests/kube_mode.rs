mod common {
    pub mod fake_apiserver;
}

use bytes::Bytes;
use common::fake_apiserver;
use fleetops_agent::creds::{Keys, SecretKeys};
use fleetops_agent::kube_proxy::KubeProxy;
use fleetops_tunnel::http::TunnelConnector;
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode};
use hyper_util::{
    client::legacy::Client,
    rt::{TokioExecutor, TokioIo},
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A plain-HTTP client whose connections are tunnel streams served by `proxy`.
fn through(proxy: Arc<KubeProxy>) -> Client<TunnelConnector, Empty<Bytes>> {
    let session = fleetops_tunnel::testing::loopback_with(move |s| {
        let p = proxy.clone();
        tokio::spawn(async move {
            let _ = p.serve(s).await;
        });
    });
    Client::builder(TokioExecutor::new()).build(TunnelConnector::new(Arc::new(session)))
}

async fn get(
    c: &Client<TunnelConnector, Empty<Bytes>>,
    path: &str,
    auth: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::get(format!("http://kube-agent{path}"));
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    let r = c.request(b.body(Empty::new()).unwrap()).await.unwrap();
    let status = r.status();
    let body = r.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn the_agents_token_replaces_whatever_the_server_sent() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let (status, body) = get(&c, "/api/v1/pods", Some("Bearer evil")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authorization"], "Bearer good-token");
}

#[tokio::test]
async fn paths_outside_the_api_never_reach_the_apiserver() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let (status, body) = get(&c, "/metrics", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("forwards only Kubernetes API paths"));
    assert!(api.auth_for("/metrics").is_empty());
}

#[tokio::test]
async fn an_upgrade_is_spliced_end_to_end() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let req = Request::get("http://kube-agent/api/v1/namespaces/a/pods/p/exec")
        .header("connection", "upgrade")
        .header("upgrade", "echo")
        .body(Empty::new())
        .unwrap();
    let resp = c.request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut io = TokioIo::new(hyper::upgrade::on(resp).await.unwrap());
    io.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    io.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
}

#[tokio::test]
async fn a_rotated_token_file_is_picked_up() {
    let api = fake_apiserver::start().await;
    let proxy = KubeProxy::new(api.upstream.clone()).unwrap();
    let c = through(proxy.clone());
    api.set_token("newer-token");
    proxy.refresh_token().unwrap();
    let (_, body) = get(&c, "/api/v1/pods", None).await;
    assert_eq!(body["authorization"], "Bearer newer-token");
}

#[tokio::test]
async fn the_secret_key_store_creates_then_updates_its_secret() {
    let api = fake_apiserver::start().await;
    let keys = Keys::Secret(SecretKeys::new(
        KubeProxy::new(api.upstream.clone()).unwrap(),
        "fleetops".into(),
        "fleetops-agent-key".into(),
    ));
    keys.probe().await.unwrap();
    assert_eq!(keys.load().await.unwrap(), None);
    keys.save("fk_one").await.unwrap();
    assert_eq!(keys.load().await.unwrap().as_deref(), Some("fk_one"));
    keys.save("fk_two").await.unwrap();
    assert_eq!(keys.load().await.unwrap().as_deref(), Some("fk_two"));
}

#[tokio::test]
async fn a_kube_agent_brings_its_cluster_up_and_keeps_its_key_in_a_secret() {
    use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion};
    std::env::set_var("POD_NAMESPACE", "fleetops");
    let api = fake_apiserver::start().await;
    let store = fleetops_store::Store::in_memory().await.unwrap();
    let state = fleetops_server::build_state(store.clone(), fleetops_api::ApiConfig::default());
    let token = fleetops_api::agents::new_join_token();
    let now = fleetops_core::now_unix();
    store
        .create_agent_environment(
            "k8s-01",
            EnvKind::KubeAgent,
            &fleetops_api::auth_token_hash(&token),
            now + 900,
            now,
        )
        .await
        .unwrap();
    state.envs.load(&store).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = fleetops_api::app(state.clone(), axum::Router::new());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let data = tempfile::tempdir().unwrap();
    let config = fleetops_agent::Config {
        server: base,
        join_token: Some(token),
        data: data.path().to_path_buf(),
        docker: "unix:///var/run/docker.sock".into(),
        ca: None,
        log: "warn".into(),
        server_silence: std::time::Duration::from_secs(45),
        kube: true,
        key_secret: Some("fleetops-agent-key".into()),
        kube_upstream: Some(api.upstream.clone()),
    };
    tokio::spawn(fleetops_agent::run(config));

    for _ in 0..100 {
        if state.agents.connected() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(state.agents.connected(), 1);
    let env = state.envs.get("k8s-01").unwrap();
    match env.status().await {
        EnvStatus::Up {
            version: EnvVersion::Kube(v),
        } => assert_eq!(v.git_version, "v1.32.0"),
        other => panic!("{other:?}"),
    }
    assert!(api
        .auth_for("/version")
        .iter()
        .all(|a| a == "Bearer good-token"));
    let secrets = api.seen.lock().unwrap().secrets.clone();
    assert!(
        secrets.contains_key("fleetops-agent-key"),
        "the key went to the Secret"
    );
    assert!(
        std::fs::read_dir(data.path()).unwrap().next().is_none(),
        "nothing written to --data"
    );
}
