#![allow(dead_code)]

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use ed25519_dalek::SigningKey;
use fleetops_api::{ApiConfig, AppState};
use fleetops_core::{
    env::{EnvFactory, EnvSpec, Environment},
    Error, Result,
};
use fleetops_ee::{
    license::{sign, Payload, VERSION},
    EeConfig,
};
use fleetops_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tower::ServiceExt;

pub const PW: &str = "correct-horse";

struct NoEnvs;
impl EnvFactory for NoEnvs {
    fn build(&self, _: &EnvSpec) -> Result<Arc<dyn Environment>> {
        Err(Error::Invalid("no environments in Enterprise tests".into()))
    }
}

pub struct Ee {
    pub router: Router,
    pub store: Store,
    pub state: AppState,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

#[derive(Clone)]
pub struct Session {
    cookie: String,
    csrf: String,
}

pub fn key() -> SigningKey {
    SigningKey::generate(&mut rand::rngs::OsRng)
}

/// A license for "Acme" expiring at `expires_at`.
pub fn license(
    key: &SigningKey,
    expires_at: i64,
    max_users: Option<u32>,
    features: &[&str],
) -> String {
    license_for_nodes(key, expires_at, max_users, None, features)
}

/// [`license`] with a node limit.
pub fn license_for_nodes(
    key: &SigningKey,
    expires_at: i64,
    max_users: Option<u32>,
    max_nodes: Option<u32>,
    features: &[&str],
) -> String {
    sign(
        &Payload {
            v: VERSION,
            id: "lic-1".into(),
            customer: "Acme".into(),
            issued_at: 0,
            expires_at,
            max_users,
            max_nodes,
            features: features.iter().map(|s| s.to_string()).collect(),
        },
        key,
    )
}

pub async fn ee_app(key: &SigningKey, file: Option<PathBuf>) -> Ee {
    ee_app_with(key, file, ApiConfig::default()).await
}

/// The URL tests give `FLEETOPS_PUBLIC_URL`.
pub const PUBLIC_URL: &str = "https://fleet.test";

/// [`ee_app`] with `config`.
pub async fn ee_app_with(key: &SigningKey, file: Option<PathBuf>, config: ApiConfig) -> Ee {
    let store = Store::in_memory().await.unwrap();
    let parts = fleetops_ee::init_with_keys(
        &store,
        EeConfig { license_file: file },
        vec![key.verifying_key()],
    )
    .await
    .unwrap();
    let state = AppState::new(store.clone(), config, Arc::new(NoEnvs))
        .with_entitlements(parts.entitlements)
        .with_extra_grants(parts.grants)
        .with_custom_roles(parts.roles)
        .with_external_auth(parts.external_auth);
    Ee {
        router: fleetops_api::app_with(state.clone(), Router::new(), parts.routes),
        store,
        state,
    }
}

impl Ee {
    pub async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<(&str, String)>,
        s: Option<&Session>,
    ) -> Resp {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(s) = s {
            b = b
                .header("cookie", &s.cookie)
                .header("x-csrf-token", &s.csrf);
        }
        let body = match body {
            Some((ct, text)) => {
                b = b.header("content-type", ct);
                Body::from(text)
            }
            None => Body::empty(),
        };
        let mut r = b.body(body).unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        let res = self.router.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Resp {
            status,
            headers,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    pub fn session(r: &Resp) -> Session {
        let cookie = r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        Session {
            cookie,
            csrf: r.json()["csrf_token"].as_str().unwrap().to_string(),
        }
    }

    fn json_body(v: Value) -> Option<(&'static str, String)> {
        Some(("application/json", v.to_string()))
    }

    pub async fn admin(&self) -> Session {
        let r = self
            .send(
                "POST",
                "/api/v1/auth/setup",
                Self::json_body(json!({"username": "admin", "password": PW})),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        Self::session(&r)
    }

    pub async fn user(&self, admin: &Session, name: &str) -> Resp {
        self.send(
            "POST",
            "/api/v1/users",
            Self::json_body(json!({"username": name, "password": PW, "role": "user"})),
            Some(admin),
        )
        .await
    }

    pub async fn login(&self, name: &str) -> Session {
        let r = self
            .send(
                "POST",
                "/api/v1/auth/login",
                Self::json_body(json!({"username": name, "password": PW})),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        Self::session(&r)
    }

    pub async fn install(&self, admin: &Session, text: &str) -> Resp {
        self.send(
            "PUT",
            "/api/v1/ee/license",
            Some(("text/plain", text.to_string())),
            Some(admin),
        )
        .await
    }

    pub async fn actions(&self) -> Vec<String> {
        self.store
            .list_audit(500, None)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }
}
