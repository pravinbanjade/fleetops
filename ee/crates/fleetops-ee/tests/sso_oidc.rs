mod common;

use axum::http::StatusCode;
use common::{idp::*, *};
use fleetops_api::ApiConfig;
use fleetops_core::now_unix;
use serde_json::{json, Value};
use std::collections::HashMap;

const DAY: i64 = 86_400;

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

pub struct Flow {
    pub ee: Ee,
    pub admin: Session,
    pub idp: Idp,
    pub conn: i64,
}

/// What `start` handed the browser.
pub struct Started {
    pub state: String,
    pub nonce: String,
    pub query: HashMap<String, String>,
    /// `fleetops_sso=<binding>`.
    pub binding: String,
}

impl Flow {
    async fn new(session_hours: i64) -> Flow {
        let idp = Idp::start("").await;
        let k = key();
        let ee = ee_app_with(
            &k,
            None,
            ApiConfig {
                public_url: Some(PUBLIC_URL.into()),
                ..ApiConfig::default()
            },
        )
        .await;
        let admin = ee.admin().await;
        ee.install(&admin, &license(&k, now_unix() + DAY, None, &[])).await;
        let r = ee
            .send(
                "POST",
                "/api/v1/ee/sso/connections",
                body(json!({
                    "name": "Acme",
                    "settings": {"kind": "oidc", "issuer": idp.uri(), "client_id": CLIENT_ID},
                    "secrets": {"client_secret": CLIENT_SECRET},
                    "enabled": true,
                    "session_hours": session_hours
                })),
                Some(&admin),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        let conn = r.json()["id"].as_i64().unwrap();
        Flow { ee, admin, idp, conn }
    }

    async fn start(&self, return_to: &str) -> Started {
        let r = self
            .ee
            .send(
                "GET",
                &format!("/api/v1/ee/sso/{}/start?return_to={return_to}", self.conn),
                None,
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::FOUND, "{}", r.text);
        let loc = r.headers["location"].to_str().unwrap();
        assert!(loc.starts_with(&format!("{}/authorize?", self.idp.uri())), "{loc}");
        let query: HashMap<String, String> = url::Url::parse(loc)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let binding = r.headers["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        Started {
            state: query["state"].clone(),
            nonce: query["nonce"].clone(),
            query,
            binding,
        }
    }

    async fn callback(&self, state: &str, cookie: Option<&str>, extra: &str) -> Resp {
        let s = Session {
            cookie: cookie.unwrap_or("").to_string(),
            csrf: String::new(),
        };
        self.ee
            .send(
                "GET",
                &format!("/api/v1/ee/sso/{}/callback?code=c1&state={state}{extra}", self.conn),
                None,
                cookie.map(|_| &s),
            )
            .await
    }

    /// A full sign-in whose ID token carries `claims`.
    async fn sign_in(&self, claims: Value) -> Resp {
        let st = self.start("/envs").await;
        let t = self.idp.id_token(&self.idp.uri(), &st.nonce, claims);
        self.idp.answer_token(Some(t)).await;
        self.callback(&st.state, Some(&st.binding), "").await
    }

    async fn users(&self) -> usize {
        self.ee.store.list_users().await.unwrap().len()
    }
}

fn location(r: &Resp) -> &str {
    r.headers["location"].to_str().unwrap()
}

#[tokio::test]
async fn start_sends_pkce_state_and_nonce() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    assert_eq!(st.query["code_challenge_method"], "S256");
    assert_eq!(st.query["response_type"], "code");
    assert_eq!(st.query["client_id"], CLIENT_ID);
    assert_eq!(
        st.query["redirect_uri"],
        format!("{PUBLIC_URL}/api/v1/ee/sso/{}/callback", f.conn)
    );
    assert!(st.binding.starts_with("fleetops_sso="));
}

#[tokio::test]
async fn full_login_sets_session_and_redirects() {
    let f = Flow::new(12).await;
    let r = f.sign_in(json!({"groups": ["ops"]})).await;
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.text);
    assert_eq!(location(&r), "/envs");
    let cookies: Vec<&str> = r.headers.get_all("set-cookie").iter().map(|v| v.to_str().unwrap()).collect();
    assert!(cookies.iter().any(|c| c.contains("Max-Age=43200")), "{cookies:?}");
    assert_eq!(f.users().await, 2);
}

#[tokio::test]
async fn session_cookie_lifetime_is_session_hours() {
    let f = Flow::new(3).await;
    let r = f.sign_in(json!({})).await;
    let cookies: Vec<&str> = r.headers.get_all("set-cookie").iter().map(|v| v.to_str().unwrap()).collect();
    assert!(cookies.iter().any(|c| c.contains("Max-Age=10800")), "{cookies:?}");
}

async fn rejected(f: &Flow, r: Resp) {
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.text);
    assert_eq!(location(&r), "/login?sso_error=invalid");
    assert_eq!(f.users().await, 1, "no user created");
    assert!(f.ee.actions().await.contains(&"sso.login".to_string()));
}

#[tokio::test]
async fn rejects_bad_signature() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({}));
    // Flip a byte of the signature.
    let mut parts: Vec<String> = t.split('.').map(String::from).collect();
    let sig = parts[2].clone();
    parts[2] = format!("{}{}", if sig.starts_with('A') { "B" } else { "A" }, &sig[1..]);
    f.idp.answer_token(Some(parts.join("."))).await;
    let r = f.callback(&st.state, Some(&st.binding), "").await;
    rejected(&f, r).await;
}

#[tokio::test]
async fn rejects_wrong_aud() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({"aud": "someone-else"}));
    f.idp.answer_token(Some(t)).await;
    rejected(&f, f.callback(&st.state, Some(&st.binding), "").await).await;
}

#[tokio::test]
async fn rejects_wrong_iss() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token("https://evil.example", &st.nonce, json!({}));
    f.idp.answer_token(Some(t)).await;
    rejected(&f, f.callback(&st.state, Some(&st.binding), "").await).await;
}

#[tokio::test]
async fn rejects_expired() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let past = now_unix() - 3600;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({"iat": past - 300, "exp": past}));
    f.idp.answer_token(Some(t)).await;
    rejected(&f, f.callback(&st.state, Some(&st.binding), "").await).await;
}

#[tokio::test]
async fn rejects_wrong_nonce() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token(&f.idp.uri(), "not-the-nonce", json!({}));
    f.idp.answer_token(Some(t)).await;
    rejected(&f, f.callback(&st.state, Some(&st.binding), "").await).await;
}

#[tokio::test]
async fn rejects_unknown_key() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let claims = f.idp.base_claims(&f.idp.uri(), &st.nonce, json!({}));
    f.idp.answer_token(Some(sign(&claims, "rotated-away"))).await;
    rejected(&f, f.callback(&st.state, Some(&st.binding), "").await).await;
}

#[tokio::test]
async fn state_is_single_use() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({}));
    f.idp.answer_token(Some(t)).await;
    let first = f.callback(&st.state, Some(&st.binding), "").await;
    assert!(!location(&first).contains("sso_error"), "{}", location(&first));
    let again = f.callback(&st.state, Some(&st.binding), "").await;
    assert_eq!(location(&again), "/login?sso_error=invalid");
}

#[tokio::test]
async fn missing_binding_cookie_rejected() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({}));
    f.idp.answer_token(Some(t)).await;
    rejected(&f, f.callback(&st.state, None, "").await).await;
}

#[tokio::test]
async fn return_to_must_be_local() {
    let f = Flow::new(12).await;
    let st = f.start("//evil.com").await;
    let t = f.idp.id_token(&f.idp.uri(), &st.nonce, json!({}));
    f.idp.answer_token(Some(t)).await;
    let r = f.callback(&st.state, Some(&st.binding), "").await;
    assert_eq!(location(&r), "/");
}

#[tokio::test]
async fn idp_access_denied_maps_to_denied() {
    let f = Flow::new(12).await;
    let st = f.start("/").await;
    let r = f
        .callback(&st.state, Some(&st.binding), "&error=access_denied")
        .await;
    assert_eq!(location(&r), "/login?sso_error=denied");
}

#[tokio::test]
async fn disabled_connection_start_refused() {
    let f = Flow::new(12).await;
    f.ee.send(
        "PATCH",
        &format!("/api/v1/ee/sso/connections/{}", f.conn),
        body(json!({"enabled": false})),
        Some(&f.admin),
    )
    .await;
    let r = f
        .ee
        .send("GET", &format!("/api/v1/ee/sso/{}/start", f.conn), None, None)
        .await;
    assert_eq!(location(&r), "/login?sso_error=disabled");
}
