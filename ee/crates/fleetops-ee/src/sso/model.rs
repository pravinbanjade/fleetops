//! What a connection is configured with, and the checks on it.

use fleetops_core::{Error, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const NAME_MAX: usize = 64;
pub const SESSION_HOURS_DEFAULT: i64 = 12;
pub const SESSION_HOURS_MAX: i64 = 720;

/// A connection's non-secret settings, by kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ConnectionSettings {
    /// Microsoft Entra ID, one tenant.
    Microsoft { tenant: String, client_id: String },
    Google {
        client_id: String,
        /// Workspace domains allowed to sign in; empty allows any.
        #[serde(default)]
        hosted_domains: Vec<String>,
        /// The Workspace admin the service account acts as to read groups.
        #[serde(default)]
        admin_email: Option<String>,
    },
    Github {
        client_id: String,
        /// Members of at least one of these organisations may sign in.
        orgs: Vec<String>,
        /// GitHub Enterprise Server, e.g. `https://github.acme.io`.
        #[serde(default)]
        base_url: Option<String>,
    },
    /// Any OpenID Connect provider, found by discovery.
    Oidc {
        issuer: String,
        client_id: String,
        #[serde(default = "default_scopes")]
        scopes: Vec<String>,
        #[serde(default = "default_groups_claim")]
        groups_claim: String,
    },
}

fn default_scopes() -> Vec<String> {
    ["openid", "profile", "email"].map(String::from).to_vec()
}

fn default_groups_claim() -> String {
    "groups".into()
}

/// Secrets as an admin sends them; an absent field keeps the stored one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Secrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Google: the service account's JSON key, to read groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_json: Option<String>,
}

impl Secrets {
    /// `self` with every field `new` sets replaced.
    pub fn merged(mut self, new: Secrets) -> Secrets {
        if new.client_secret.is_some() {
            self.client_secret = new.client_secret;
        }
        if new.service_account_json.is_some() {
            self.service_account_json = new.service_account_json;
        }
        self
    }

    /// The names of the secrets that are set, for the API.
    pub fn names(&self) -> Vec<String> {
        let mut v = vec![];
        if self.client_secret.is_some() {
            v.push("client_secret".into());
        }
        if self.service_account_json.is_some() {
            v.push("service_account_json".into());
        }
        v
    }
}

impl ConnectionSettings {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Microsoft { .. } => "microsoft",
            Self::Google { .. } => "google",
            Self::Github { .. } => "github",
            Self::Oidc { .. } => "oidc",
        }
    }

    /// Where the provider's user ids come from: they only identify a user
    /// within it, so it can't change once users signed in.
    pub fn identity_source(&self) -> Option<&str> {
        match self {
            Self::Microsoft { tenant, .. } => Some(tenant),
            Self::Oidc { issuer, .. } => Some(issuer),
            Self::Github { base_url, .. } => base_url.as_deref(),
            Self::Google { .. } => None,
        }
    }

    /// Parses settings an admin sent, with a clear message for LDAP.
    pub fn parse(v: serde_json::Value) -> Result<Self> {
        if v.get("kind").and_then(|k| k.as_str()) == Some("ldap") {
            return Err(Error::Invalid("ldap arrives in a later release".into()));
        }
        serde_json::from_value(v).map_err(|e| Error::Invalid(format!("settings: {e}")))
    }

    /// Trims every field and checks it.
    pub fn validated(self) -> Result<Self> {
        let client = |id: String| -> Result<String> {
            let id = id.trim().to_string();
            if id.is_empty() || id.len() > 512 {
                return Err(Error::Invalid("client_id must be 1-512 characters".into()));
            }
            Ok(id)
        };
        Ok(match self {
            Self::Microsoft { tenant, client_id } => {
                let tenant = tenant.trim().to_ascii_lowercase();
                if matches!(tenant.as_str(), "common" | "organizations" | "consumers") {
                    return Err(Error::Invalid(
                        "tenant must be your own tenant ID or domain, not a multi-tenant alias"
                            .into(),
                    ));
                }
                if !is_guid(&tenant) && !is_domain(&tenant) {
                    return Err(Error::Invalid(
                        "tenant must be a tenant ID (GUID) or a verified domain".into(),
                    ));
                }
                Self::Microsoft {
                    tenant,
                    client_id: client(client_id)?,
                }
            }
            Self::Google {
                client_id,
                hosted_domains,
                admin_email,
            } => {
                let hosted_domains = hosted_domains
                    .iter()
                    .map(|d| d.trim().to_ascii_lowercase())
                    .filter(|d| !d.is_empty())
                    .collect::<Vec<_>>();
                if hosted_domains.is_empty() {
                    return Err(Error::Invalid(
                        "list at least one Workspace domain that may sign in".into(),
                    ));
                }
                if let Some(d) = hosted_domains.iter().find(|d| !is_domain(d)) {
                    return Err(Error::Invalid(format!("'{d}' is not a domain")));
                }
                let admin_email = admin_email
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty());
                if let Some(e) = &admin_email {
                    if !e.contains('@') {
                        return Err(Error::Invalid(
                            "admin_email must be an email address".into(),
                        ));
                    }
                }
                Self::Google {
                    client_id: client(client_id)?,
                    hosted_domains,
                    admin_email,
                }
            }
            Self::Github {
                client_id,
                orgs,
                base_url,
            } => {
                let orgs = orgs
                    .iter()
                    .map(|o| o.trim().to_string())
                    .filter(|o| !o.is_empty())
                    .collect::<Vec<_>>();
                if orgs.is_empty() {
                    return Err(Error::Invalid(
                        "list at least one GitHub organisation".into(),
                    ));
                }
                let base_url = base_url
                    .map(|u| u.trim().trim_end_matches('/').to_string())
                    .filter(|u| !u.is_empty());
                if let Some(u) = &base_url {
                    check_url(u, "base_url")?;
                }
                Self::Github {
                    client_id: client(client_id)?,
                    orgs,
                    base_url,
                }
            }
            Self::Oidc {
                issuer,
                client_id,
                scopes,
                groups_claim,
            } => {
                let issuer = issuer.trim().trim_end_matches('/').to_string();
                check_url(&issuer, "issuer")?;
                let mut scopes = scopes
                    .iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                if !scopes.iter().any(|s| s == "openid") {
                    scopes.insert(0, "openid".into());
                }
                let groups_claim = groups_claim.trim().to_string();
                if groups_claim.is_empty() {
                    return Err(Error::Invalid("groups_claim must not be empty".into()));
                }
                Self::Oidc {
                    issuer,
                    client_id: client(client_id)?,
                    scopes,
                    groups_claim,
                }
            }
        })
    }
}

/// Checks a secret set before it is sealed.
pub fn validate_secrets(s: &Secrets) -> Result<()> {
    if let Some(json) = &s.service_account_json {
        let v: serde_json::Value = serde_json::from_str(json)
            .map_err(|_| Error::Invalid("service_account_json is not JSON".into()))?;
        for field in ["client_email", "private_key"] {
            if v.get(field).and_then(|f| f.as_str()).is_none() {
                return Err(Error::Invalid(format!(
                    "service_account_json has no {field}; use the service account's JSON key"
                )));
            }
        }
    }
    Ok(())
}

/// The trimmed name, or why it is not allowed.
pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(Error::Invalid(format!(
            "connection name must be 1-{NAME_MAX} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
    {
        return Err(Error::Invalid(
            "connection name may only contain letters, digits, spaces, '_', '-' and '.'".into(),
        ));
    }
    Ok(name.to_string())
}

pub fn validate_session_hours(h: i64) -> Result<i64> {
    if !(1..=SESSION_HOURS_MAX).contains(&h) {
        return Err(Error::Invalid(format!(
            "session_hours must be 1-{SESSION_HOURS_MAX}"
        )));
    }
    Ok(h)
}

fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_domain(s: &str) -> bool {
    s.len() <= 253
        && s.contains('.')
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// `https://`, or `http://` on the loopback interface (tests, local IdPs).
fn check_url(u: &str, field: &str) -> Result<()> {
    let parsed = url::Url::parse(u).map_err(|_| Error::Invalid(format!("{field} is not a URL")))?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if local => Ok(()),
        _ => Err(Error::Invalid(format!("{field} must be an https:// URL"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn microsoft_refuses_multi_tenant_aliases() {
        for t in ["common", "Organizations", "consumers"] {
            let s = ConnectionSettings::Microsoft {
                tenant: t.into(),
                client_id: "x".into(),
            };
            assert!(s.validated().is_err(), "{t}");
        }
        let ok = ConnectionSettings::Microsoft {
            tenant: "72F988BF-86F1-41AF-91AB-2D7CD011DB47".into(),
            client_id: "x".into(),
        };
        assert!(ok.validated().is_ok());
        let domain = ConnectionSettings::Microsoft {
            tenant: "acme.onmicrosoft.com".into(),
            client_id: "x".into(),
        };
        assert!(domain.validated().is_ok());
    }

    #[test]
    fn ldap_is_refused_with_a_clear_message() {
        let e = ConnectionSettings::parse(json!({"kind": "ldap"})).unwrap_err();
        assert!(e.to_string().contains("later release"), "{e}");
    }

    #[test]
    fn oidc_needs_https_except_on_loopback() {
        let s = |issuer: &str| ConnectionSettings::Oidc {
            issuer: issuer.into(),
            client_id: "x".into(),
            scopes: vec![],
            groups_claim: "groups".into(),
        };
        assert!(s("http://idp.acme.io").validated().is_err());
        assert!(s("http://127.0.0.1:8080/realms/x").validated().is_ok());
        match s("https://idp.acme.io/").validated().unwrap() {
            ConnectionSettings::Oidc { issuer, scopes, .. } => {
                assert_eq!(issuer, "https://idp.acme.io");
                assert_eq!(scopes, vec!["openid"]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn github_needs_an_org() {
        let s = ConnectionSettings::Github {
            client_id: "x".into(),
            orgs: vec![" ".into()],
            base_url: None,
        };
        assert!(s.validated().is_err());
    }

    #[test]
    fn service_account_json_needs_email_and_key() {
        let bad = Secrets {
            service_account_json: Some("{}".into()),
            ..Secrets::default()
        };
        assert!(validate_secrets(&bad).is_err());
        let good = Secrets {
            service_account_json: Some(
                json!({"client_email": "a@b.iam", "private_key": "k"}).to_string(),
            ),
            ..Secrets::default()
        };
        assert!(validate_secrets(&good).is_ok());
    }
}
