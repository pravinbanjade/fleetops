//! GitHub (OAuth2, not OIDC): who signed in comes from the REST API with
//! their access token; members of an allowed organisation only.

use super::{
    db::Connection,
    flow::Runtime,
    model::ConnectionSettings,
    oidc::claim_str,
    provision::{Claims, Refusal},
};
use serde_json::Value;

/// The web base (authorize, token) for a connection.
pub fn web_base(rt: &Runtime, c: &Connection) -> String {
    match &c.settings {
        ConnectionSettings::Github {
            base_url: Some(b), ..
        } => b.clone(),
        _ => rt.endpoints.github_web.clone(),
    }
}

/// The REST API base: `<base>/api/v3` on GitHub Enterprise Server.
pub fn api_base(rt: &Runtime, c: &Connection) -> String {
    match &c.settings {
        ConnectionSettings::Github {
            base_url: Some(b), ..
        } => format!("{b}/api/v3"),
        _ => rt.endpoints.github_api.clone(),
    }
}

async fn get_all(rt: &Runtime, url: &str, token: &str) -> fleetops_core::Result<Vec<Value>> {
    let mut out = vec![];
    for page in 1..=10 {
        let sep = if url.contains('?') { '&' } else { '?' };
        let items: Vec<Value> = rt
            .oidc
            .get_json(&format!("{url}{sep}per_page=100&page={page}"), Some(token))
            .await?;
        let n = items.len();
        out.extend(items);
        if n < 100 {
            break;
        }
    }
    Ok(out)
}

pub async fn identify(
    rt: &Runtime,
    c: &Connection,
    token: &str,
) -> Result<Claims, (Refusal, String)> {
    let ConnectionSettings::Github { orgs, .. } = &c.settings else {
        unreachable!()
    };
    let invalid = |e: fleetops_core::Error| (Refusal::Invalid, e.to_string());
    let api = api_base(rt, c);
    let user: Value = rt
        .oidc
        .get_json(&format!("{api}/user"), Some(token))
        .await
        .map_err(invalid)?;
    let id = user
        .get("id")
        .and_then(Value::as_i64)
        .ok_or((Refusal::Invalid, "no GitHub user id".into()))?;
    let allowed: Vec<String> = orgs.iter().map(|o| o.to_lowercase()).collect();
    let member_of: Vec<String> = get_all(rt, &format!("{api}/user/orgs"), token)
        .await
        .map_err(invalid)?
        .iter()
        .filter_map(|o| claim_str(o, "login"))
        .map(|o| o.to_lowercase())
        .collect();
    if !member_of.iter().any(|o| allowed.contains(o)) {
        return Err((Refusal::NotAllowed, "not in an allowed organisation".into()));
    }
    let emails = get_all(rt, &format!("{api}/user/emails"), token)
        .await
        .unwrap_or_default();
    let email = emails
        .iter()
        .find(|e| {
            e.get("primary") == Some(&Value::Bool(true))
                && e.get("verified") == Some(&Value::Bool(true))
        })
        .and_then(|e| claim_str(e, "email"));
    let groups = get_all(rt, &format!("{api}/user/teams"), token)
        .await
        .map_err(invalid)?
        .iter()
        .filter_map(|t| {
            let org = t
                .get("organization")
                .and_then(|o| claim_str(o, "login"))?
                .to_lowercase();
            let slug = claim_str(t, "slug")?.to_lowercase();
            allowed.contains(&org).then(|| format!("{org}/{slug}"))
        })
        .collect();
    Ok(Claims {
        subject: id.to_string(),
        preferred_username: None,
        email,
        login: claim_str(&user, "login"),
        groups,
    })
}
