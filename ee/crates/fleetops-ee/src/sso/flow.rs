//! The sign-in redirect (SSO spec, "The redirect login flow"):
//! `start` sends the browser to the provider with PKCE, `state` and
//! `nonce`; `callback` checks them, verifies who signed in, provisions the
//! user and starts a session. Failures land on `/login?sso_error=<code>`.

use super::{
    db::{self, db_err, Connection},
    model::ConnectionSettings,
    oidc::{claim_str, string_list, Oidc},
    provision::{self, Claims, Refusal},
    routes::callback_url,
};
use axum::{
    extract::{Query, State},
    http::{
        header::{COOKIE, LOCATION, SET_COOKIE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    Extension,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fleetops_api::{
    auth::{extract::ClientMeta, start_session},
    extract::ApiPath,
    AppState,
};
use fleetops_core::{now_unix, Error, Result};
use rand::RngCore;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use utoipa::IntoParams;

const BINDING_COOKIE: &str = "fleetops_sso";
const COOKIE_PATH: &str = "/api/v1/ee/sso";
/// The detail of a refusal provisioning already audited.
const AUDITED: &str = "audited";
/// How long a redirect may take before its state expires.
const STATE_SECS: i64 = 600;

/// Where each provider lives; tests point these at a mock.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub microsoft_login: String,
    pub graph: String,
    pub google_issuer: String,
    pub google_token: String,
    pub google_directory: String,
    pub github_web: String,
    pub github_api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            microsoft_login: "https://login.microsoftonline.com".into(),
            graph: "https://graph.microsoft.com".into(),
            google_issuer: "https://accounts.google.com".into(),
            google_token: "https://oauth2.googleapis.com/token".into(),
            google_directory: "https://admin.googleapis.com".into(),
            github_web: "https://github.com".into(),
            github_api: "https://api.github.com".into(),
        }
    }
}

/// Shared by the sign-in routes.
pub struct Runtime {
    pub oidc: Oidc,
    pub endpoints: Endpoints,
    /// Google service-account tokens, by client email.
    pub google_tokens: std::sync::Mutex<std::collections::HashMap<String, (i64, String)>>,
}

impl Runtime {
    pub fn new(endpoints: Endpoints) -> Self {
        Self {
            oidc: Oidc::new(),
            endpoints,
            google_tokens: Default::default(),
        }
    }
}

pub type Rt = Extension<Arc<Runtime>>;

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn sha256_b64(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}

/// A same-origin path, or nothing: never `//host` or a full URL.
pub fn safe_return_to(p: Option<&str>) -> Option<String> {
    let p = p?.trim();
    let ok = p.starts_with('/')
        && !p.starts_with("//")
        && !p.starts_with("/\\")
        && !p.chars().any(|c| c.is_control());
    ok.then(|| p.to_string())
}

fn redirect(to: &str) -> Response {
    let mut r = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(to) {
        r.headers_mut().insert(LOCATION, v);
    }
    r
}

fn fail(code: Refusal) -> Response {
    let mut r = redirect(&format!("/login?sso_error={}", code.code()));
    r.headers_mut().append(SET_COOKIE, clear_binding());
    r
}

fn binding_cookie(value: &str, tls: bool) -> HeaderValue {
    let secure = if tls { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{BINDING_COOKIE}={value}; HttpOnly; SameSite=Lax; Path={COOKIE_PATH}; Max-Age={STATE_SECS}{secure}"
    ))
    .expect("cookie")
}

fn clear_binding() -> HeaderValue {
    HeaderValue::from_static(
        "fleetops_sso=; HttpOnly; SameSite=Lax; Path=/api/v1/ee/sso; Max-Age=0",
    )
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// Where the browser goes to sign in, and the scopes to ask for.
async fn authorize_endpoint(rt: &Runtime, c: &Connection) -> Result<(String, String)> {
    Ok(match &c.settings {
        ConnectionSettings::Github { .. } => (
            format!("{}/login/oauth/authorize", super::github::web_base(rt, c)),
            "read:user user:email read:org".into(),
        ),
        ConnectionSettings::Oidc { scopes, .. } => (
            rt.oidc
                .discover(&issuer(rt, c))
                .await?
                .authorization_endpoint,
            scopes.join(" "),
        ),
        ConnectionSettings::Microsoft { .. } | ConnectionSettings::Google { .. } => (
            rt.oidc
                .discover(&issuer(rt, c))
                .await?
                .authorization_endpoint,
            "openid profile email".into(),
        ),
    })
}

/// The OIDC issuer a connection discovers from.
pub fn issuer(rt: &Runtime, c: &Connection) -> String {
    match &c.settings {
        ConnectionSettings::Oidc { issuer, .. } => issuer.clone(),
        ConnectionSettings::Microsoft { tenant, .. } => {
            format!("{}/{tenant}/v2.0", rt.endpoints.microsoft_login)
        }
        ConnectionSettings::Google { .. } => rt.endpoints.google_issuer.clone(),
        ConnectionSettings::Github { .. } => String::new(),
    }
}

fn client_id(c: &Connection) -> &str {
    match &c.settings {
        ConnectionSettings::Microsoft { client_id, .. }
        | ConnectionSettings::Google { client_id, .. }
        | ConnectionSettings::Github { client_id, .. }
        | ConnectionSettings::Oidc { client_id, .. } => client_id,
    }
}

#[derive(Deserialize, IntoParams)]
pub struct StartQuery {
    /// A path on this server to land on after signing in.
    pub return_to: Option<String>,
}

#[utoipa::path(get, path = "/ee/sso/{id}/start", operation_id = "startSso", tag = "enterprise",
    summary = "Begin signing in through a connection (a browser redirect)", security(()),
    params(("id" = i64, Path, description = "Connection id"), StartQuery),
    responses((status = 302, description = "To the identity provider, or back to the login page with sso_error")))]
pub async fn start(
    State(s): State<AppState>,
    Extension(rt): Rt,
    meta: ClientMeta,
    ApiPath(id): ApiPath<i64>,
    Query(q): Query<StartQuery>,
) -> Response {
    match start_inner(&s, &rt, &meta, id, q).await {
        Ok(r) => r,
        Err((code, detail)) => {
            tracing::info!(connection = id, detail, "single sign-on start failed");
            fail(code)
        }
    }
}

async fn start_inner(
    s: &AppState,
    rt: &Runtime,
    meta: &ClientMeta,
    id: i64,
    q: StartQuery,
) -> std::result::Result<Response, (Refusal, String)> {
    let invalid = |e: Error| (Refusal::Invalid, e.to_string());
    let c = db::get(s.store.pool(), id).await.map_err(invalid)?;
    if !c.enabled {
        return Err((Refusal::Disabled, "connection disabled".into()));
    }
    let callback = callback_url(s, id).ok_or((Refusal::Invalid, "no public URL".into()))?;
    let (auth, scopes) = authorize_endpoint(rt, &c).await.map_err(invalid)?;
    let now = now_unix();
    let (state, nonce, verifier, binding) = (
        random_token(),
        random_token(),
        random_token(),
        random_token(),
    );
    let pool = s.store.pool();
    sqlx::query("DELETE FROM ee_sso_states WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| invalid(db_err(e)))?;
    sqlx::query(
        "INSERT INTO ee_sso_states (state, connection_id, nonce, pkce_verifier, binding_hash, return_to, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&state)
    .bind(id)
    .bind(&nonce)
    .bind(&verifier)
    .bind(sha256_b64(&binding))
    .bind(safe_return_to(q.return_to.as_deref()))
    .bind(now + STATE_SECS)
    .execute(pool)
    .await
    .map_err(|e| invalid(db_err(e)))?;
    let mut url = url::Url::parse(&auth).map_err(|e| (Refusal::Invalid, e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id(&c))
        .append_pair("redirect_uri", &callback)
        .append_pair("scope", &scopes)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &sha256_b64(&verifier))
        .append_pair("code_challenge_method", "S256");
    let mut r = redirect(url.as_str());
    r.headers_mut()
        .insert(SET_COOKIE, binding_cookie(&binding, meta.tls));
    Ok(r)
}

#[derive(Deserialize, IntoParams)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(sqlx::FromRow)]
struct StateRow {
    connection_id: i64,
    nonce: String,
    pkce_verifier: String,
    binding_hash: String,
    return_to: Option<String>,
    expires_at: i64,
}

#[utoipa::path(get, path = "/ee/sso/{id}/callback", operation_id = "ssoCallback", tag = "enterprise",
    summary = "Where the identity provider sends the browser back", security(()),
    params(("id" = i64, Path, description = "Connection id"), CallbackQuery),
    responses((status = 302, description = "Signed in, or back to the login page with sso_error")))]
pub async fn callback(
    State(s): State<AppState>,
    Extension(rt): Rt,
    meta: ClientMeta,
    headers: HeaderMap,
    ApiPath(id): ApiPath<i64>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let c = match db::get(s.store.pool(), id).await {
        Ok(c) => c,
        Err(_) => return fail(Refusal::Invalid),
    };
    match callback_inner(&s, &rt, &meta, &headers, &c, q).await {
        Ok(r) => r,
        Err((code, detail)) => {
            // Provisioning audits its own refusals.
            if detail != AUDITED {
                provision::refused(&s, &c, code, &detail).await;
            }
            fail(code)
        }
    }
}

async fn callback_inner(
    s: &AppState,
    rt: &Runtime,
    meta: &ClientMeta,
    headers: &HeaderMap,
    c: &Connection,
    q: CallbackQuery,
) -> std::result::Result<Response, (Refusal, String)> {
    let pool = s.store.pool();
    let state = q.state.ok_or((Refusal::Invalid, "no state".into()))?;
    // Single use: the row goes before anything else is checked.
    let row: Option<StateRow> = sqlx::query_as(
        "DELETE FROM ee_sso_states WHERE state = ?
         RETURNING connection_id, nonce, pkce_verifier, binding_hash, return_to, expires_at",
    )
    .bind(&state)
    .fetch_optional(pool)
    .await
    .map_err(|e| (Refusal::Invalid, e.to_string()))?;
    let row = row.ok_or((Refusal::Invalid, "unknown or used state".into()))?;
    let binding =
        cookie(headers, BINDING_COOKIE).ok_or((Refusal::Invalid, "no binding cookie".into()))?;
    if sha256_b64(&binding) != row.binding_hash {
        return Err((Refusal::Invalid, "binding cookie does not match".into()));
    }
    if row.expires_at < now_unix() {
        return Err((Refusal::Invalid, "state expired".into()));
    }
    if row.connection_id != c.id {
        return Err((Refusal::Invalid, "state is for another connection".into()));
    }
    if !c.enabled {
        return Err((Refusal::Disabled, "connection disabled".into()));
    }
    if let Some(e) = q.error {
        let code = if e == "access_denied" {
            Refusal::Denied
        } else {
            Refusal::Invalid
        };
        return Err((code, format!("provider error: {e}")));
    }
    let code = q.code.ok_or((Refusal::Invalid, "no code".into()))?;
    let claims = identify(s, rt, c, &code, &row.nonce, &row.pkce_verifier).await?;
    let user = match provision::login(s, c, claims).await {
        Ok(Ok(u)) => u,
        Ok(Err(refusal)) => return Err((refusal, AUDITED.into())),
        Err(e) => return Err((Refusal::Invalid, e.to_string())),
    };
    let session = start_session(s, user, meta, StatusCode::OK, c.session_hours * 3600)
        .await
        .map_err(|e| (Refusal::Invalid, format!("{e:?}")))?;
    let mut r = redirect(row.return_to.as_deref().unwrap_or("/"));
    if let Some(v) = session.headers().get(SET_COOKIE) {
        r.headers_mut().append(SET_COOKIE, v.clone());
    }
    r.headers_mut().append(SET_COOKIE, clear_binding());
    Ok(r)
}

/// Exchanges the code and works out who signed in.
async fn identify(
    s: &AppState,
    rt: &Runtime,
    c: &Connection,
    code: &str,
    nonce: &str,
    verifier: &str,
) -> std::result::Result<Claims, (Refusal, String)> {
    let invalid = |e: Error| (Refusal::Invalid, e.to_string());
    let secrets = db::secrets(&s.store, c.id).await.map_err(invalid)?;
    let secret = secrets
        .client_secret
        .clone()
        .ok_or((Refusal::Invalid, "no client secret saved".into()))?;
    let callback = callback_url(s, c.id).ok_or((Refusal::Invalid, "no public URL".into()))?;
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", callback.as_str()),
        ("client_id", client_id(c)),
        ("client_secret", secret.as_str()),
        ("code_verifier", verifier),
    ];
    if let ConnectionSettings::Github { .. } = &c.settings {
        let token_url = format!(
            "{}/login/oauth/access_token",
            super::github::web_base(rt, c)
        );
        let tokens = rt.oidc.exchange(&token_url, &form).await.map_err(invalid)?;
        return super::github::identify(rt, c, &tokens.access_token).await;
    }
    let d = rt.oidc.discover(&issuer(rt, c)).await.map_err(invalid)?;
    let tokens = rt
        .oidc
        .exchange(&d.token_endpoint, &form)
        .await
        .map_err(invalid)?;
    let id_token = tokens
        .id_token
        .ok_or((Refusal::Invalid, "the provider sent no ID token".into()))?;
    let claims = rt
        .oidc
        .verify_id_token(&id_token, &d, client_id(c), nonce)
        .await
        .map_err(invalid)?;
    match &c.settings {
        ConnectionSettings::Oidc { groups_claim, .. } => Ok(oidc_claims(&claims, groups_claim)),
        ConnectionSettings::Microsoft { .. } => {
            super::microsoft::identify(s, rt, c, &claims, &tokens.access_token).await
        }
        ConnectionSettings::Google { .. } => {
            super::google::identify(s, rt, c, &claims, &secrets).await
        }
        ConnectionSettings::Github { .. } => unreachable!(),
    }
}

/// Claims from a generic OIDC ID token.
pub fn oidc_claims(c: &Value, groups_claim: &str) -> Claims {
    Claims {
        subject: claim_str(c, "sub").unwrap_or_default(),
        preferred_username: claim_str(c, "preferred_username"),
        email: claim_str(c, "email"),
        login: None,
        groups: string_list(c.get(groups_claim)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_must_be_a_local_path() {
        assert_eq!(safe_return_to(Some("/envs/1")), Some("/envs/1".into()));
        assert_eq!(safe_return_to(Some("//evil.com")), None);
        assert_eq!(safe_return_to(Some("/\\evil.com")), None);
        assert_eq!(safe_return_to(Some("https://evil.com")), None);
        assert_eq!(safe_return_to(None), None);
    }

    #[test]
    fn pkce_challenge_is_s256() {
        // RFC 7636 appendix B.
        assert_eq!(
            sha256_b64("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
