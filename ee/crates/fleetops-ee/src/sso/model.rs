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
    /// An LDAP directory, signed into on the password form.
    Ldap {
        preset: LdapPreset,
        /// `ldap://` or `ldaps://`.
        url: String,
        #[serde(default)]
        starttls: bool,
        #[serde(default)]
        skip_tls_verify: bool,
        /// PEM certificates to trust instead of the system's.
        #[serde(default)]
        ca_cert: Option<String>,
        /// The service account; none searches anonymously.
        #[serde(default)]
        bind_dn: Option<String>,
        user_base_dn: String,
        /// Empty takes the preset's.
        #[serde(default)]
        user_filter: String,
        #[serde(default)]
        username_attr: String,
        #[serde(default = "default_email_attr")]
        email_attr: String,
        /// Where groups are searched; none reads no groups.
        #[serde(default)]
        group_base_dn: Option<String>,
        #[serde(default)]
        group_filter: String,
    },
}

/// Which directory an LDAP connection talks to; sets the default filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LdapPreset {
    ActiveDirectory,
    Openldap,
    Custom,
}

/// The preset's user filter, username attribute and group filter; empty
/// for `Custom`.
pub fn ldap_preset_defaults(p: LdapPreset) -> (&'static str, &'static str, &'static str) {
    match p {
        LdapPreset::ActiveDirectory => (
            "(&(objectClass=user)(sAMAccountName={username}))",
            "sAMAccountName",
            "(member:1.2.840.113556.1.4.1941:={user_dn})",
        ),
        LdapPreset::Openldap => (
            "(&(objectClass=inetOrgPerson)(uid={username}))",
            "uid",
            "(|(&(objectClass=groupOfNames)(member={user_dn}))(&(objectClass=posixGroup)(memberUid={username})))",
        ),
        LdapPreset::Custom => ("", "", ""),
    }
}

fn default_email_attr() -> String {
    "mail".into()
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
    /// LDAP: the service account's password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_password: Option<String>,
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
        if new.bind_password.is_some() {
            self.bind_password = new.bind_password;
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
        if self.bind_password.is_some() {
            v.push("bind_password".into());
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
            Self::Ldap { .. } => "ldap",
        }
    }

    /// Whether sign-in goes through a browser redirect (everything but LDAP).
    pub fn is_redirect(&self) -> bool {
        !matches!(self, Self::Ldap { .. })
    }

    /// Where the provider's user ids come from: they only identify a user
    /// within it, so it can't change once users signed in.
    pub fn identity_source(&self) -> Option<&str> {
        match self {
            Self::Microsoft { tenant, .. } => Some(tenant),
            Self::Oidc { issuer, .. } => Some(issuer),
            Self::Github { base_url, .. } => base_url.as_deref(),
            // LDAP subjects are directory UUIDs; the URL may change (failover,
            // a new certificate name).
            Self::Google { .. } | Self::Ldap { .. } => None,
        }
    }

    /// Parses settings an admin sent.
    pub fn parse(v: serde_json::Value) -> Result<Self> {
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
            Self::Ldap {
                preset,
                url,
                starttls,
                skip_tls_verify,
                ca_cert,
                bind_dn,
                user_base_dn,
                user_filter,
                username_attr,
                email_attr,
                group_base_dn,
                group_filter,
            } => {
                let opt =
                    |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
                let (d_user, d_attr, d_group) = ldap_preset_defaults(preset);
                let or = |v: String, d: &str| {
                    let v = v.trim().to_string();
                    if v.is_empty() {
                        d.to_string()
                    } else {
                        v
                    }
                };
                let url = url.trim().to_string();
                let parsed =
                    url::Url::parse(&url).map_err(|_| Error::Invalid("url is not a URL".into()))?;
                match parsed.scheme() {
                    "ldap" => {}
                    "ldaps" if starttls => {
                        return Err(Error::Invalid(
                            "StartTLS needs ldap://; ldaps:// is already encrypted".into(),
                        ))
                    }
                    "ldaps" => {}
                    _ => {
                        return Err(Error::Invalid(
                            "url must start with ldap:// or ldaps://".into(),
                        ))
                    }
                }
                if parsed.host_str().unwrap_or("").is_empty() {
                    return Err(Error::Invalid("url has no host".into()));
                }
                let ca_cert = opt(ca_cert);
                if let Some(pem) = &ca_cert {
                    if ldap_ca_certs(pem).is_empty() {
                        return Err(Error::Invalid(
                            "ca_cert must hold PEM certificates (-----BEGIN CERTIFICATE-----)"
                                .into(),
                        ));
                    }
                }
                let user_base_dn = user_base_dn.trim().to_string();
                if user_base_dn.is_empty() {
                    return Err(Error::Invalid("user_base_dn must not be empty".into()));
                }
                let user_filter = or(user_filter, d_user);
                if user_filter.is_empty() {
                    return Err(Error::Invalid("user_filter must not be empty".into()));
                }
                if !user_filter.contains("{username}") {
                    return Err(Error::Invalid("user_filter must contain {username}".into()));
                }
                let username_attr = or(username_attr, d_attr);
                if username_attr.is_empty() {
                    return Err(Error::Invalid("username_attr must not be empty".into()));
                }
                let email_attr = or(email_attr, "mail");
                let group_base_dn = opt(group_base_dn);
                let group_filter = or(group_filter, d_group);
                if group_base_dn.is_some() {
                    if group_filter.is_empty() {
                        return Err(Error::Invalid(
                            "group_filter must not be empty when group_base_dn is set".into(),
                        ));
                    }
                    if !group_filter.contains("{user_dn}") && !group_filter.contains("{username}") {
                        return Err(Error::Invalid(
                            "group_filter must contain {user_dn} or {username}".into(),
                        ));
                    }
                }
                Self::Ldap {
                    preset,
                    url,
                    starttls,
                    skip_tls_verify,
                    ca_cert,
                    bind_dn: opt(bind_dn),
                    user_base_dn,
                    user_filter,
                    username_attr,
                    email_attr,
                    group_base_dn,
                    group_filter,
                }
            }
        })
    }
}

/// The certificates in a PEM bundle; empty when there are none.
pub fn ldap_ca_certs(pem: &str) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
        .filter_map(|c| c.ok())
        .collect()
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

    fn ldap(preset: &str) -> serde_json::Value {
        json!({"kind": "ldap", "preset": preset, "url": "ldap://dc.acme.io",
               "user_base_dn": "dc=acme,dc=io"})
    }

    fn ldap_err(v: serde_json::Value) -> String {
        ConnectionSettings::parse(v)
            .and_then(|s| s.validated())
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn ldap_fills_preset_defaults() {
        let open = ConnectionSettings::parse(ldap("openldap"))
            .unwrap()
            .validated()
            .unwrap();
        match open {
            ConnectionSettings::Ldap {
                user_filter,
                username_attr,
                group_filter,
                email_attr,
                ..
            } => {
                assert_eq!(
                    user_filter,
                    "(&(objectClass=inetOrgPerson)(uid={username}))"
                );
                assert_eq!(username_attr, "uid");
                assert_eq!(group_filter, "(|(&(objectClass=groupOfNames)(member={user_dn}))(&(objectClass=posixGroup)(memberUid={username})))");
                assert_eq!(email_attr, "mail");
            }
            _ => unreachable!(),
        }
        let ad = ConnectionSettings::parse(ldap("active_directory"))
            .unwrap()
            .validated()
            .unwrap();
        match ad {
            ConnectionSettings::Ldap {
                user_filter,
                username_attr,
                group_filter,
                ..
            } => {
                assert_eq!(
                    user_filter,
                    "(&(objectClass=user)(sAMAccountName={username}))"
                );
                assert_eq!(username_attr, "sAMAccountName");
                assert_eq!(group_filter, "(member:1.2.840.113556.1.4.1941:={user_dn})");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn ldap_custom_needs_filters() {
        assert!(ldap_err(ldap("custom")).contains("user_filter"));
        let mut v = ldap("custom");
        v["user_filter"] = json!("(uid={username})");
        v["username_attr"] = json!("uid");
        v["group_base_dn"] = json!("ou=groups,dc=acme,dc=io");
        assert!(ldap_err(v).contains("group_filter"));
    }

    #[test]
    fn ldap_user_filter_needs_placeholder() {
        let mut v = ldap("openldap");
        v["user_filter"] = json!("(uid=bob)");
        assert!(ldap_err(v).contains("{username}"));
        let mut v = ldap("openldap");
        v["group_base_dn"] = json!("ou=groups,dc=acme,dc=io");
        v["group_filter"] = json!("(cn=ops)");
        assert!(ldap_err(v).contains("{user_dn}"));
    }

    #[test]
    fn ldap_url_scheme() {
        let mut v = ldap("openldap");
        v["url"] = json!("http://dc.acme.io");
        assert!(ldap_err(v).contains("ldap://"));
        let mut v = ldap("openldap");
        v["url"] = json!("ldaps://dc.acme.io");
        assert!(ConnectionSettings::parse(v.clone())
            .unwrap()
            .validated()
            .is_ok());
        v["starttls"] = json!(true);
        assert!(ldap_err(v).contains("StartTLS"));
    }

    #[test]
    fn ldap_ca_cert_must_be_pem() {
        let mut v = ldap("openldap");
        v["ca_cert"] = json!("nope");
        assert!(ldap_err(v).contains("PEM"));
        let mut v = ldap("openldap");
        v["ca_cert"] = json!(TEST_CA);
        assert!(ConnectionSettings::parse(v).unwrap().validated().is_ok());
    }

    #[test]
    fn ldap_base_dn_required() {
        let mut v = ldap("openldap");
        v["user_base_dn"] = json!("  ");
        assert!(ldap_err(v).contains("user_base_dn"));
    }

    #[test]
    fn bind_password_is_a_secret() {
        let s = Secrets::default().merged(Secrets {
            bind_password: Some("pw".into()),
            ..Secrets::default()
        });
        assert_eq!(s.bind_password.as_deref(), Some("pw"));
        assert_eq!(s.names(), vec!["bind_password"]);
    }

    /// A self-signed certificate, only parsed.
    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBWDCB/qADAgECAhQQ5RP7JKWn7NDA7XTD/6N7tGfSmDAKBggqhkjOPQQDAjAS
MRAwDgYDVQQDDAd0ZXN0LWNhMB4XDTI2MTAxODA4MjYyNFoXDTM2MTAxNTA4MjYy
NFowEjEQMA4GA1UEAwwHdGVzdC1jYTBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IA
BJEdBYb3O2SIY1Q1nuU5rISO54FOFdzgVwVwwX846elmrTyXx8ZjpW1PGvtkNe9a
GeQp3f600LmnLEujI23Tab6jMjAwMB0GA1UdDgQWBBT8HPMDhUd1Ta8bvIw6hGSy
7qHZ8zAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0kAMEYCIQD/YEyMB+eb
PlAblh0xJAueigN3uh1WTbSzJK377rmpagIhAJ2k+605oWL6MYNSvID6uUJRLVKg
Z2xKO39S+JUEk2hA
-----END CERTIFICATE-----";

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
