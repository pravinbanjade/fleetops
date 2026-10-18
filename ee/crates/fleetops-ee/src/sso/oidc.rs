//! The OpenID Connect pieces: discovery, keys, the code exchange and the ID
//! token check. Nothing from a token is trusted until `verify_id_token`
//! has checked its signature, issuer, audience, expiry and nonce.

use fleetops_core::{Error, Result};
use jsonwebtoken::{jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// How long a discovery document or key set is reused.
const CACHE_FOR: Duration = Duration::from_secs(3600);
/// Clock skew allowed on `exp` and `iat`.
const LEEWAY_SECS: u64 = 60;

#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
}

/// Outbound HTTP for sign-in, with discovery and key caches.
pub struct Oidc {
    pub http: reqwest::Client,
    discovery: Mutex<HashMap<String, (Instant, Discovery)>>,
    keys: Mutex<HashMap<String, (Instant, JwkSet)>>,
}

impl Default for Oidc {
    fn default() -> Self {
        Self::new()
    }
}

fn upstream(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Upstream {
        status: 502,
        message: format!("{what}: {e}"),
    }
}

impl Oidc {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("fleetops")
                .build()
                .expect("HTTP client"),
            discovery: Mutex::default(),
            keys: Mutex::default(),
        }
    }

    /// GETs `url` as JSON, failing on any non-2xx status.
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<T> {
        let mut req = self.http.get(url).header("accept", "application/json");
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.map_err(|e| upstream(url, e))?;
        if !res.status().is_success() {
            return Err(upstream(url, res.status()));
        }
        res.json().await.map_err(|e| upstream(url, e))
    }

    /// The provider's discovery document for `issuer`.
    pub async fn discover(&self, issuer: &str) -> Result<Discovery> {
        if let Some((at, d)) = self.discovery.lock().unwrap().get(issuer) {
            if at.elapsed() < CACHE_FOR {
                return Ok(d.clone());
            }
        }
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let d: Discovery = self.get_json(&url, None).await?;
        self.discovery
            .lock()
            .unwrap()
            .insert(issuer.to_string(), (Instant::now(), d.clone()));
        Ok(d)
    }

    /// Drops cached documents, after a connection's settings change.
    pub fn forget(&self) {
        self.discovery.lock().unwrap().clear();
        self.keys.lock().unwrap().clear();
    }

    async fn jwks(&self, uri: &str, fresh: bool) -> Result<JwkSet> {
        if !fresh {
            if let Some((at, k)) = self.keys.lock().unwrap().get(uri) {
                if at.elapsed() < CACHE_FOR {
                    return Ok(k.clone());
                }
            }
        }
        let k: JwkSet = self.get_json(uri, None).await?;
        self.keys
            .lock()
            .unwrap()
            .insert(uri.to_string(), (Instant::now(), k.clone()));
        Ok(k)
    }

    /// Exchanges an authorization code (with its PKCE verifier) for tokens.
    pub async fn exchange(
        &self,
        token_endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<Tokens> {
        let res = self
            .http
            .post(token_endpoint)
            .header("accept", "application/json")
            .form(form)
            .send()
            .await
            .map_err(|e| upstream("token endpoint", e))?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(upstream("token endpoint", format!("{status}: {text}")));
        }
        res.json().await.map_err(|e| upstream("token endpoint", e))
    }

    /// The ID token's claims, once its signature (by the provider's keys),
    /// issuer, audience, expiry and nonce all check out.
    pub async fn verify_id_token(
        &self,
        token: &str,
        d: &Discovery,
        client_id: &str,
        nonce: &str,
    ) -> Result<Value> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| Error::Unauthorized(format!("ID token header: {e}")))?;
        if !matches!(
            header.alg,
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512
                | Algorithm::ES256
                | Algorithm::ES384
        ) {
            return Err(Error::Unauthorized(format!(
                "ID token algorithm {:?} is not allowed",
                header.alg
            )));
        }
        let kid = header.kid.clone();
        let find = |set: &JwkSet| match &kid {
            Some(k) => set.find(k).cloned(),
            None if set.keys.len() == 1 => set.keys.first().cloned(),
            None => None,
        };
        let jwk = match find(&self.jwks(&d.jwks_uri, false).await?) {
            Some(j) => j,
            // The provider may have rotated its keys.
            None => find(&self.jwks(&d.jwks_uri, true).await?)
                .ok_or_else(|| Error::Unauthorized("ID token signed by an unknown key".into()))?,
        };
        let key = DecodingKey::from_jwk(&jwk)
            .map_err(|e| Error::Unauthorized(format!("provider key: {e}")))?;
        let mut v = Validation::new(header.alg);
        v.set_issuer(&[&d.issuer]);
        v.set_audience(&[client_id]);
        v.leeway = LEEWAY_SECS;
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data = jsonwebtoken::decode::<Value>(token, &key, &v)
            .map_err(|e| Error::Unauthorized(format!("ID token: {e}")))?;
        if data.claims.get("nonce").and_then(Value::as_str) != Some(nonce) {
            return Err(Error::Unauthorized("ID token nonce does not match".into()));
        }
        Ok(data.claims)
    }
}

/// A claim that is a string or a list of strings, as a list.
pub fn string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

pub fn claim_str(c: &Value, name: &str) -> Option<String> {
    c.get(name)
        .and_then(Value::as_str)
        .map(String::from)
        .filter(|s| !s.is_empty())
}
