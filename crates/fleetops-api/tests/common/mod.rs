#![allow(dead_code)]

pub mod fake_docker;

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
    pub async fn send(&self, r: Request<Body>) -> Resp {
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
