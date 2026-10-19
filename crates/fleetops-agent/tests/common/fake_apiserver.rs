//! A TLS apiserver stand-in: answers /version and /api/v1/nodes, keeps
//! Secrets in memory, upgrades `.../exec` to an echo pipe, and records the
//! path and Authorization of every request.
#![allow(dead_code)]

use bytes::Bytes;
use fleetops_agent::kube_proxy::KubeUpstream;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct Seen {
    pub requests: Vec<(String, String)>,
    pub secrets: HashMap<String, Value>,
}

pub struct FakeApi {
    pub dir: tempfile::TempDir,
    pub upstream: KubeUpstream,
    pub seen: Arc<Mutex<Seen>>,
}

impl FakeApi {
    pub fn set_token(&self, t: &str) {
        std::fs::write(self.dir.path().join("token"), t).unwrap();
    }
    pub fn auth_for(&self, path: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, a)| a.clone())
            .collect()
    }
}

fn json_resp(status: StatusCode, v: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(v.to_string())))
        .unwrap()
}

async fn handle(seen: Arc<Mutex<Seen>>, mut req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_string();
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    seen.lock()
        .unwrap()
        .requests
        .push((path.clone(), auth.clone()));
    if path.ends_with("/exec") && req.headers().contains_key("upgrade") {
        let up = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            if let Ok(up) = up.await {
                let (mut r, mut w) = tokio::io::split(TokioIo::new(up));
                let _ = tokio::io::copy(&mut r, &mut w).await;
            }
        });
        return Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header("connection", "upgrade")
            .header("upgrade", "echo")
            .body(Full::new(Bytes::new()))
            .unwrap();
    }
    if path == "/version" {
        return json_resp(
            StatusCode::OK,
            json!({"major": "1", "minor": "32", "gitVersion": "v1.32.0",
            "gitCommit": "", "gitTreeState": "", "buildDate": "", "goVersion": "", "compiler": "", "platform": "linux/amd64"}),
        );
    }
    if path == "/api/v1/nodes" {
        return json_resp(
            StatusCode::OK,
            json!({"kind": "NodeList", "apiVersion": "v1", "metadata": {}, "items": []}),
        );
    }
    if let Some(rest) = path.strip_prefix("/api/v1/namespaces/fleetops/secrets") {
        let method = req.method().clone();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let mut s = seen.lock().unwrap();
        let name = rest.trim_start_matches('/').to_string();
        return match (method.as_str(), name.is_empty()) {
            ("GET", false) => match s.secrets.get(&name) {
                Some(v) => json_resp(StatusCode::OK, v.clone()),
                None => json_resp(
                    StatusCode::NOT_FOUND,
                    json!({"kind": "Status", "code": 404}),
                ),
            },
            ("POST", true) => {
                let v: Value = serde_json::from_slice(&body).unwrap();
                let n = v["metadata"]["name"].as_str().unwrap().to_string();
                s.secrets.insert(n, v.clone());
                json_resp(StatusCode::CREATED, v)
            }
            ("PATCH", false) => match s.secrets.get_mut(&name) {
                Some(v) => {
                    let patch: Value = serde_json::from_slice(&body).unwrap();
                    v["data"] = patch["data"].clone();
                    json_resp(StatusCode::OK, v.clone())
                }
                None => json_resp(
                    StatusCode::NOT_FOUND,
                    json!({"kind": "Status", "code": 404}),
                ),
            },
            _ => json_resp(StatusCode::METHOD_NOT_ALLOWED, json!({})),
        };
    }
    json_resp(StatusCode::OK, json!({"path": path, "authorization": auth}))
}

pub async fn start() -> FakeApi {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let cert =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    std::fs::write(dir.path().join("ca.crt"), cert.cert.pem()).unwrap();
    std::fs::write(dir.path().join("token"), "good-token").unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, seen) = (acceptor.clone(), s2.clone());
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let svc = hyper::service::service_fn(move |req| {
                    let seen = seen.clone();
                    async move { Ok::<_, Infallible>(handle(seen, req).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), svc)
                    .with_upgrades()
                    .await;
            });
        }
    });
    let upstream = KubeUpstream {
        base: format!("https://127.0.0.1:{port}").parse().unwrap(),
        ca: dir.path().join("ca.crt"),
        token: dir.path().join("token"),
    };
    FakeApi {
        dir,
        upstream,
        seen,
    }
}
