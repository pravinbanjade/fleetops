//! Microsoft Entra ID: one tenant, groups from the token or, past 200
//! groups, from Microsoft Graph.

use super::{
    db::Connection,
    flow::Runtime,
    model::ConnectionSettings,
    oidc::{claim_str, string_list},
    provision::{Claims, Refusal},
};
use fleetops_api::{audit_detail, AppState};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Page {
    value: Vec<Value>,
    #[serde(rename = "@odata.nextLink")]
    next: Option<String>,
}

/// Every group the user is in, through Graph (`GroupMember.Read.All`).
async fn graph_groups(rt: &Runtime, access_token: &str) -> fleetops_core::Result<Vec<String>> {
    let mut url = Some(format!(
        "{}/v1.0/me/transitiveMemberOf/microsoft.graph.group?$select=id",
        rt.endpoints.graph
    ));
    let mut out = vec![];
    // Bounded: 50 pages of 100.
    for _ in 0..50 {
        let Some(u) = url.take() else { break };
        let page: Page = rt.oidc.get_json(&u, Some(access_token)).await?;
        out.extend(page.value.iter().filter_map(|g| claim_str(g, "id")));
        url = page.next;
    }
    Ok(out)
}

pub async fn identify(
    s: &AppState,
    rt: &Runtime,
    c: &Connection,
    claims: &Value,
    access_token: &str,
) -> Result<Claims, (Refusal, String)> {
    let ConnectionSettings::Microsoft { tenant, .. } = &c.settings else {
        unreachable!()
    };
    let tid = claim_str(claims, "tid").unwrap_or_default();
    let oid = claim_str(claims, "oid").ok_or((Refusal::Invalid, "no oid claim".into()))?;
    // A GUID tenant must match exactly; a domain tenant was pinned by the
    // issuer check already.
    if tenant.contains('-') && !tenant.contains('.') && !tid.eq_ignore_ascii_case(tenant) {
        return Err((Refusal::NotAllowed, format!("token from tenant {tid}")));
    }
    let overage = claims
        .get("_claim_names")
        .and_then(|n| n.get("groups"))
        .is_some();
    let groups = if overage {
        match graph_groups(rt, access_token).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(connection = c.id, error = %e, "Microsoft Graph groups failed");
                audit_detail(
                    s,
                    None,
                    "sso.login",
                    "sso_connection",
                    &c.id.to_string(),
                    "failure",
                    Some(json!({"connection": c.name, "groups_error": e.to_string()})),
                )
                .await;
                vec![]
            }
        }
    } else {
        string_list(claims.get("groups"))
    };
    Ok(Claims {
        subject: format!("{oid}:{tid}"),
        preferred_username: claim_str(claims, "preferred_username"),
        email: claim_str(claims, "email").or_else(|| claim_str(claims, "preferred_username")),
        login: None,
        groups,
    })
}
