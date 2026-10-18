//! Google Workspace: a verified email in an allowed domain; groups through
//! the Admin SDK, as a service account acting for a Workspace admin.

use super::{
    db::Connection,
    flow::Runtime,
    model::{ConnectionSettings, Secrets},
    oidc::claim_str,
    provision::{Claims, Refusal},
};
use fleetops_api::{audit_detail, AppState};
use fleetops_core::{now_unix, Error, Result};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Deserialize;
use serde_json::{json, Value};

const SCOPE: &str = "https://www.googleapis.com/auth/admin.directory.group.readonly";

#[derive(Deserialize)]
struct Key {
    client_email: String,
    private_key: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct GroupsPage {
    #[serde(default)]
    groups: Vec<Value>,
    #[serde(rename = "nextPageToken")]
    next: Option<String>,
}

/// An access token for the service account acting as `admin`, cached until
/// five minutes before it expires.
async fn service_token(rt: &Runtime, key_json: &str, admin: &str) -> Result<String> {
    let key: Key = serde_json::from_str(key_json)
        .map_err(|e| Error::Invalid(format!("service account key: {e}")))?;
    let cache_key = format!("{}|{admin}", key.client_email);
    let now = now_unix();
    if let Some((exp, t)) = rt.google_tokens.lock().unwrap().get(&cache_key) {
        if *exp - 300 > now {
            return Ok(t.clone());
        }
    }
    let claims = json!({
        "iss": key.client_email,
        "sub": admin,
        "scope": SCOPE,
        "aud": rt.endpoints.google_token,
        "iat": now,
        "exp": now + 3600,
    });
    let signing = EncodingKey::from_rsa_pem(key.private_key.as_bytes())
        .map_err(|e| Error::Invalid(format!("service account private key: {e}")))?;
    let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &signing)
        .map_err(|e| Error::Internal(format!("signing the assertion: {e}")))?;
    let res = rt
        .oidc
        .http
        .post(&rt.endpoints.google_token)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await
        .map_err(|e| Error::Upstream { status: 502, message: format!("Google token: {e}") })?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(Error::Upstream {
            status: 502,
            message: format!("Google token: {status}: {text}"),
        });
    }
    let t: TokenResponse = res.json().await.map_err(|e| Error::Upstream {
        status: 502,
        message: format!("Google token: {e}"),
    })?;
    rt.google_tokens
        .lock()
        .unwrap()
        .insert(cache_key, (now + t.expires_in, t.access_token.clone()));
    Ok(t.access_token)
}

/// The email addresses of the groups `email` is a direct member of.
pub async fn groups(rt: &Runtime, key_json: &str, admin: &str, email: &str) -> Result<Vec<String>> {
    let token = service_token(rt, key_json, admin).await?;
    let mut out = vec![];
    let mut page_token: Option<String> = None;
    for _ in 0..50 {
        let mut url = url::Url::parse(&format!(
            "{}/admin/directory/v1/groups",
            rt.endpoints.google_directory
        ))
        .map_err(|e| Error::Internal(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("userKey", email)
            .append_pair("maxResults", "200");
        if let Some(p) = &page_token {
            url.query_pairs_mut().append_pair("pageToken", p);
        }
        let page: GroupsPage = rt.oidc.get_json(url.as_str(), Some(&token)).await?;
        out.extend(page.groups.iter().filter_map(|g| claim_str(g, "email")));
        match page.next {
            Some(n) => page_token = Some(n),
            None => break,
        }
    }
    Ok(out)
}

pub async fn identify(
    s: &AppState,
    rt: &Runtime,
    c: &Connection,
    claims: &Value,
    secrets: &Secrets,
) -> std::result::Result<Claims, (Refusal, String)> {
    let ConnectionSettings::Google { hosted_domains, admin_email, .. } = &c.settings else {
        unreachable!()
    };
    if claims.get("email_verified").and_then(Value::as_bool) != Some(true) {
        return Err((Refusal::NotAllowed, "email not verified".into()));
    }
    let email = claim_str(claims, "email").ok_or((Refusal::Invalid, "no email".into()))?;
    let hd = claim_str(claims, "hd").unwrap_or_default().to_ascii_lowercase();
    if !hosted_domains.is_empty() && !hosted_domains.contains(&hd) {
        return Err((Refusal::NotAllowed, format!("domain '{hd}' not allowed")));
    }
    let groups = match (&secrets.service_account_json, admin_email) {
        (Some(key), Some(admin)) => match groups(rt, key, admin, &email).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(connection = c.id, error = %e, "Google groups failed");
                audit_detail(s, None, "sso.login", "sso_connection", &c.id.to_string(), "failure",
                    Some(json!({"connection": c.name, "groups_error": e.to_string()}))).await;
                vec![]
            }
        },
        _ => vec![],
    };
    Ok(Claims {
        subject: claim_str(claims, "sub").unwrap_or_default(),
        preferred_username: None,
        email: Some(email),
        login: None,
        groups,
    })
}
