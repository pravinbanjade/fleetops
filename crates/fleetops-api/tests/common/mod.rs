#![allow(dead_code)]

pub mod agent;
pub mod contract;
pub mod fake_docker;
pub mod fake_kube;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use fake_docker::{FakeDocker, FakeFactory};
use fleetops_api::{ApiConfig, AppState};
use fleetops_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc};

/// A token kubeconfig for a cluster nobody listens on.
pub const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\ncurrent-context: dev\nclusters:\n- name: dev\n  cluster: {server: 'https://127.0.0.1:1'}\ncontexts:\n- name: dev\n  context: {cluster: dev, user: dev}\n- name: other\n  context: {cluster: dev, user: dev}\nusers:\n- name: dev\n  user: {token: very-secret-token}\n";
use tower::ServiceExt;

pub const ADMIN: &str = "admin";
pub const ADMIN_PW: &str = "correct-horse";

pub struct TestApp {
    pub router: Router,
    pub store: Store,
    pub state: AppState,
    pub fake: Arc<FakeDocker>,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub json: Value,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

pub async fn app() -> TestApp {
    app_with(ApiConfig::default()).await
}

pub async fn app_with(config: ApiConfig) -> TestApp {
    let store = Store::in_memory().await.unwrap();
    let fake = FakeDocker::new();
    let state = AppState::new(store.clone(), config, Arc::new(FakeFactory(fake.clone())));
    let router = fleetops_api::app(state.clone(), Router::new());
    TestApp {
        router,
        store,
        state,
        fake,
    }
}

pub fn req(
    method: &str,
    uri: &str,
    body: Option<Value>,
    session: Option<&Session>,
) -> Request<Body> {
    req_from([127, 0, 0, 1], method, uri, body, session)
}

pub fn req_from(
    ip: [u8; 4],
    method: &str,
    uri: &str,
    body: Option<Value>,
    session: Option<&Session>,
) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(s) = session {
        b = b
            .header("cookie", &s.cookie)
            .header("x-csrf-token", &s.csrf);
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut r = b.body(body).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 40000))));
    r
}

pub fn session_from(r: &Resp) -> Session {
    let set = r
        .headers
        .get("set-cookie")
        .expect("set-cookie")
        .to_str()
        .unwrap();
    Session {
        cookie: set.split(';').next().unwrap().to_string(),
        csrf: r.json["csrf_token"]
            .as_str()
            .expect("csrf_token")
            .to_string(),
    }
}

impl TestApp {
    /// Sends `r` and checks the exchange against the OpenAPI document
    /// (see `contract`), failing the test on any mismatch.
    pub async fn send(&self, r: Request<Body>) -> Resp {
        let (parts, body) = r.into_parts();
        let sent = body.collect().await.unwrap().to_bytes();
        let request: Option<Value> = parts
            .headers
            .get("content-type")
            .filter(|ct| ct.as_bytes().starts_with(b"application/json"))
            .and_then(|_| serde_json::from_slice(&sent).ok());
        let (method, path) = (parts.method.clone(), parts.uri.path().to_string());
        let r = Request::from_parts(parts, Body::from(sent));
        let res = self.router.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        let exchange = contract::Exchange {
            method: method.as_str(),
            path: &path,
            request: request.as_ref(),
            status: status.as_u16(),
            content_type: headers.get("content-type").and_then(|v| v.to_str().ok()),
            body: &json,
        };
        if let Err(e) = contract::check(&exchange) {
            panic!("OpenAPI contract: {e}");
        }
        Resp {
            status,
            headers,
            json,
        }
    }

    pub async fn setup_admin(&self) -> Session {
        let r = self
            .send(req(
                "POST",
                "/api/v1/auth/setup",
                Some(json!({"username": ADMIN, "password": ADMIN_PW})),
                None,
            ))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
        session_from(&r)
    }

    pub async fn login(&self, username: &str, password: &str) -> Resp {
        self.send(req(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": username, "password": password})),
            None,
        ))
        .await
    }

    /// Registers environment `name` (backed by `self.fake`) as `admin`.
    /// A kube-config environment backed by `self.fake.kube`.
    pub async fn add_kube_env(&self, admin: &Session, name: &str) {
        let r = self
            .send(req("POST", "/api/v1/envs", Some(serde_json::json!({
                "name": name, "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true})), Some(admin)))
            .await;
        assert_eq!(r.status, 201, "{:?}", r.json);
    }

    pub async fn add_env(&self, admin: &Session, name: &str) {
        let r = self
            .send(req(
                "POST",
                "/api/v1/envs",
                Some(json!({"name": name, "kind": "docker-local", "url": "unix:///fake.sock"})),
                Some(admin),
            ))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    }

    /// An admin session with environment "local" registered.
    pub async fn with_local(&self) -> Session {
        let admin = self.setup_admin().await;
        self.add_env(&admin, "local").await;
        admin
    }

    /// Grants `username` a role on `env` straight through the store.
    pub async fn grant(&self, username: &str, env: &str, role: &str, namespaces: Option<&[&str]>) {
        let (user, _) = self
            .store
            .find_credentials(username)
            .await
            .unwrap()
            .unwrap();
        let env = self
            .store
            .list_environments()
            .await
            .unwrap()
            .into_iter()
            .find(|e| e.name == env)
            .unwrap();
        let ns: Option<std::collections::BTreeSet<String>> =
            namespaces.map(|n| n.iter().map(|s| s.to_string()).collect());
        self.store
            .set_binding(user.id, env.id, role.parse().unwrap(), ns.as_ref(), 0)
            .await
            .unwrap();
    }

    pub async fn create_user(
        &self,
        admin: &Session,
        username: &str,
        password: &str,
        role: &str,
    ) -> Resp {
        self.send(req(
            "POST",
            "/api/v1/users",
            Some(json!({"username": username, "password": password, "role": role})),
            Some(admin),
        ))
        .await
    }
}

// ---- websocket ---------------------------------------------------------------

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
pub use tokio_tungstenite::tungstenite::Message as WsMessage;

pub type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// How long a websocket test waits for any one frame.
pub const WS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

impl TestApp {
    /// Serves the app on an ephemeral port; websockets need a real socket.
    /// Every audit row's action, newest first.
    pub async fn audit_actions(&self) -> Vec<String> {
        self.store
            .list_audit(1000, None)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }

    pub async fn serve(&self) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = self.router.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        addr
    }
}

/// Opens `/api/v1/ws` as `session`, with an optional Origin header.
pub async fn ws_connect(
    addr: SocketAddr,
    session: Option<&Session>,
    origin: Option<&str>,
) -> Result<WsStream, tokio_tungstenite::tungstenite::Error> {
    let mut r = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .unwrap();
    if let Some(s) = session {
        r.headers_mut().insert("cookie", s.cookie.parse().unwrap());
    }
    if let Some(o) = origin {
        r.headers_mut().insert("origin", o.parse().unwrap());
    }
    tokio_tungstenite::connect_async(r).await.map(|(ws, _)| ws)
}

pub struct Ws(pub WsStream);

impl Ws {
    pub async fn open(addr: SocketAddr, session: &Session) -> Ws {
        Ws(ws_connect(addr, Some(session), None)
            .await
            .expect("ws connect"))
    }

    pub async fn send(&mut self, v: Value) {
        self.0
            .send(WsMessage::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn send_text(&mut self, t: &str) {
        self.0
            .send(WsMessage::Text(t.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn send_binary(&mut self, b: Vec<u8>) {
        self.0.send(WsMessage::Binary(b.into())).await.unwrap();
    }

    /// The next frame, failing the test after `WS_WAIT`. None once closed.
    pub async fn frame(&mut self) -> Option<WsMessage> {
        loop {
            match tokio::time::timeout(WS_WAIT, self.0.next())
                .await
                .expect("websocket frame within WS_WAIT")
            {
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(m)) => return Some(m),
            }
        }
    }

    /// The next JSON frame, skipping binary frames.
    pub async fn json(&mut self) -> Value {
        loop {
            match self.frame().await.expect("socket closed") {
                WsMessage::Text(t) => return serde_json::from_str(&t).unwrap(),
                _ => continue,
            }
        }
    }

    /// JSON frames until one matches `pred`, which is returned.
    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let v = self.json().await;
            if pred(&v) {
                return v;
            }
        }
    }

    /// Asserts nothing arrives for `ms` milliseconds.
    pub async fn quiet(&mut self, ms: u64) {
        let until = tokio::time::Instant::now() + std::time::Duration::from_millis(ms);
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(until, self.0.next()).await {
            // Reading answers pings; they are not messages.
            if !matches!(m, WsMessage::Ping(_) | WsMessage::Pong(_)) {
                panic!("expected silence, got {m:?}");
            }
        }
    }
}

/// A `sub` frame for environment `local`.
pub fn ws_sub(n: u32, topic: &str, id: Option<&str>, opts: Value) -> Value {
    json!({"op": "sub", "sub": n, "topic": topic, "env": "local", "id": id, "opts": opts})
}

/// `{"sub": n, "type": t}`?
pub fn is(v: &Value, sub: u32, t: &str) -> bool {
    v["sub"] == sub && v["type"] == t
}

/// Polls `cond` for up to WS_WAIT.
pub async fn eventually(what: &str, cond: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(start.elapsed() < WS_WAIT, "timed out waiting for {what}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Websocket-friendly config: fast reconnects and session checks.
pub fn ws_config() -> ApiConfig {
    ApiConfig {
        ws_backoff: std::time::Duration::from_millis(10),
        ws_session_check: std::time::Duration::from_millis(50),
        kube_watch_grace: std::time::Duration::from_millis(50),
        ..ApiConfig::default()
    }
}

/// A renderer that records what it was asked and answers a fixed chart.
#[derive(Debug, Default)]
pub struct FakeRenderer {
    pub calls: std::sync::Mutex<Vec<(String, serde_json::Value, bool)>>,
}

#[async_trait::async_trait]
impl fleetops_kube::helm::render::ChartRenderer for FakeRenderer {
    async fn render(
        &self,
        r: &fleetops_kube::helm::render::RenderInput,
    ) -> fleetops_core::Result<fleetops_core::helm::Rendered> {
        self.calls
            .lock()
            .unwrap()
            .push((r.name.clone(), r.values.clone(), r.upgrade));
        Ok(fleetops_core::helm::Rendered {
            manifest: "---\n# Source: demo/templates/cm.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: demo}\n".into(),
            hooks: vec![],
            notes: "Hello".into(),
            chart: serde_json::json!({"metadata": {"name": "demo", "version": "0.2.0"}}),
            meta: fleetops_core::helm::ChartMeta { name: "demo".into(), version: "0.2.0".into(), ..Default::default() },
            config: r.values.clone(),
            crds: vec![],
        })
    }
    async fn show(
        &self,
        _c: &fleetops_core::helm::ChartRef,
    ) -> fleetops_core::Result<fleetops_core::helm::ChartShow> {
        Ok(fleetops_core::helm::ChartShow {
            meta: Default::default(),
            values: "replicas: 1\n".into(),
        })
    }
    fn insecure_repos(&self) -> bool {
        false
    }
}
