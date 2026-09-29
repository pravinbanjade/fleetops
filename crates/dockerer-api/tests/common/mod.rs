#![allow(dead_code)]

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use dockerer_api::{ApiConfig, AppState};
use dockerer_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower::ServiceExt;

pub const ADMIN: &str = "admin";
pub const ADMIN_PW: &str = "correct-horse";

pub struct TestApp {
    pub router: Router,
    pub store: Store,
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
    let router = dockerer_api::app(AppState::new(store.clone(), config), Router::new());
    TestApp { router, store }
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
