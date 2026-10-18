//! Signing in against an LDAP directory (SSO spec, "LDAP login"): bind as
//! the service account, find the user, read their groups, then bind as the
//! user with the password they typed.

use super::model::{ldap_ca_certs, ConnectionSettings};
use fleetops_core::{Error, Result};
use ldap3::{
    ldap_escape, Ldap, LdapConnAsync, LdapConnSettings, Scope, SearchEntry, SearchOptions,
};
use std::{sync::Arc, time::Duration};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const OP_TIMEOUT: Duration = Duration::from_secs(10);
/// `invalidCredentials`.
const RC_INVALID_CREDENTIALS: u32 = 49;
/// `sizeLimitExceeded`: the user filter matched more than one entry.
const RC_SIZE_LIMIT: u32 = 4;

/// A directory user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub dn: String,
    /// `objectGUID`, else `entryUUID`, else the DN lowercased.
    pub subject: String,
    /// The username attribute's value (the typed name if it has none).
    pub username: String,
    pub email: Option<String>,
    /// The DNs of the groups the group filter found.
    pub groups: Vec<String>,
}

/// The parts of a connection's settings a sign-in needs.
struct Cfg<'a> {
    url: &'a str,
    starttls: bool,
    skip_tls_verify: bool,
    ca_cert: Option<&'a str>,
    bind_dn: Option<&'a str>,
    user_base_dn: &'a str,
    user_filter: &'a str,
    username_attr: &'a str,
    email_attr: &'a str,
    group_base_dn: Option<&'a str>,
    group_filter: &'a str,
}

fn cfg(settings: &ConnectionSettings) -> Result<Cfg<'_>> {
    match settings {
        ConnectionSettings::Ldap {
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
            ..
        } => Ok(Cfg {
            url,
            starttls: *starttls,
            skip_tls_verify: *skip_tls_verify,
            ca_cert: ca_cert.as_deref(),
            bind_dn: bind_dn.as_deref(),
            user_base_dn,
            user_filter,
            username_attr,
            email_attr,
            group_base_dn: group_base_dn.as_deref(),
            group_filter,
        }),
        _ => Err(Error::Invalid("not an LDAP connection".into())),
    }
}

/// `filter` with `{username}` and `{user_dn}` replaced by their RFC 4515
/// escaped values.
pub fn fill(filter: &str, username: &str, user_dn: &str) -> String {
    filter
        .replace("{username}", &ldap_escape(username))
        .replace("{user_dn}", &ldap_escape(user_dn))
}

/// An Active Directory `objectGUID` as the usual GUID string: the first
/// three fields are little-endian.
pub fn guid_string(b: &[u8]) -> Option<String> {
    if b.len() != 16 {
        return None;
    }
    let hex = |r: &[u8]| r.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let rev = |r: &[u8]| r.iter().rev().copied().collect::<Vec<u8>>();
    Some(format!(
        "{}-{}-{}-{}-{}",
        hex(&rev(&b[0..4])),
        hex(&rev(&b[4..6])),
        hex(&rev(&b[6..8])),
        hex(&b[8..10]),
        hex(&b[10..16])
    ))
}

/// The directory could not answer (502): not the same as "no".
fn upstream(message: String) -> Error {
    Error::Upstream {
        status: 502,
        message,
    }
}

fn ldap_err(e: ldap3::LdapError) -> Error {
    upstream(format!("LDAP: {e}"))
}

async fn connect(c: &Cfg<'_>) -> Result<Ldap> {
    let mut settings = LdapConnSettings::new()
        .set_conn_timeout(CONNECT_TIMEOUT)
        .set_starttls(c.starttls)
        .set_no_tls_verify(c.skip_tls_verify);
    if let (Some(pem), false) = (c.ca_cert, c.skip_tls_verify) {
        let mut roots = rustls::RootCertStore::empty();
        for cert in ldap_ca_certs(pem) {
            roots
                .add(cert)
                .map_err(|e| Error::Invalid(format!("ca_cert: {e}")))?;
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Invalid(format!("TLS: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        settings = settings.set_config(Arc::new(config));
    }
    let (conn, mut ldap) = LdapConnAsync::with_settings(settings, c.url)
        .await
        .map_err(ldap_err)?;
    tokio::spawn(async move {
        if let Err(e) = conn.drive().await {
            tracing::debug!(error = %e, "LDAP connection closed");
        }
    });
    ldap.with_timeout(OP_TIMEOUT);
    Ok(ldap)
}

/// Binds as the service account (anonymously without a bind DN).
async fn service_bind(ldap: &mut Ldap, c: &Cfg<'_>, bind_password: Option<&str>) -> Result<()> {
    let (dn, pw) = match c.bind_dn {
        Some(dn) => (
            dn,
            bind_password
                .filter(|p| !p.is_empty())
                .ok_or_else(|| Error::Invalid("no bind_password saved".into()))?,
        ),
        None => ("", ""),
    };
    ldap.with_timeout(OP_TIMEOUT)
        .simple_bind(dn, pw)
        .await
        .map_err(ldap_err)?
        .success()
        .map_err(|e| upstream(format!("LDAP service bind failed: {e}")))?;
    Ok(())
}

/// The single entry `username` names, with its groups.
async fn find(ldap: &mut Ldap, c: &Cfg<'_>, username: &str) -> Result<Option<Entry>> {
    let filter = fill(c.user_filter, username, "");
    let ldap3::SearchResult(entries, res) = ldap
        .with_search_options(SearchOptions::new().sizelimit(2))
        .with_timeout(OP_TIMEOUT)
        .search(
            c.user_base_dn,
            Scope::Subtree,
            &filter,
            vec![c.username_attr, c.email_attr, "objectGUID", "entryUUID"],
        )
        .await
        .map_err(ldap_err)?;
    if res.rc == RC_SIZE_LIMIT {
        tracing::info!(username, "LDAP user filter matched more than one entry");
        return Ok(None);
    }
    if res.rc != 0 {
        return Err(upstream(format!(
            "LDAP user search failed: {} {}",
            res.rc, res.text
        )));
    }
    if entries.len() != 1 {
        if entries.len() > 1 {
            tracing::info!(username, "LDAP user filter matched more than one entry");
        }
        return Ok(None);
    }
    let e = SearchEntry::construct(entries.into_iter().next().expect("one entry"));
    let first = |name: &str| {
        e.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.first().cloned())
    };
    let guid = e
        .bin_attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectGUID"))
        .and_then(|(_, v)| v.first())
        .and_then(|b| guid_string(b));
    let subject = guid
        .or_else(|| first("entryUUID").map(|u| u.to_ascii_lowercase()))
        .unwrap_or_else(|| e.dn.to_ascii_lowercase());
    let found_name = first(c.username_attr).unwrap_or_else(|| username.to_string());
    let groups = match c.group_base_dn {
        Some(base) => {
            let filter = fill(c.group_filter, &found_name, &e.dn);
            let ldap3::SearchResult(rows, res) = ldap
                .with_timeout(OP_TIMEOUT)
                .search(base, Scope::Subtree, &filter, vec!["1.1"])
                .await
                .map_err(ldap_err)?;
            if res.rc != 0 {
                return Err(upstream(format!(
                    "LDAP group search failed: {} {}",
                    res.rc, res.text
                )));
            }
            rows.into_iter()
                .map(|r| SearchEntry::construct(r).dn)
                .collect()
        }
        None => vec![],
    };
    Ok(Some(Entry {
        dn: e.dn.clone(),
        subject,
        username: found_name,
        email: first(c.email_attr),
        groups,
    }))
}

/// Service bind only, for the Test button.
pub async fn check(settings: &ConnectionSettings, bind_password: Option<&str>) -> Result<()> {
    let c = cfg(settings)?;
    let mut ldap = connect(&c).await?;
    let r = service_bind(&mut ldap, &c, bind_password).await;
    let _ = ldap.unbind().await;
    r
}

/// Finds `username` and their groups. `Ok(None)`: no such user, or not
/// exactly one.
pub async fn lookup(
    settings: &ConnectionSettings,
    bind_password: Option<&str>,
    username: &str,
) -> Result<Option<Entry>> {
    let c = cfg(settings)?;
    let mut ldap = connect(&c).await?;
    let r = async {
        service_bind(&mut ldap, &c, bind_password).await?;
        find(&mut ldap, &c, username).await
    }
    .await;
    let _ = ldap.unbind().await;
    r
}

/// [`lookup`], then a bind as the found entry with `password`. `Ok(None)`:
/// unknown or ambiguous user, or a wrong or empty password.
pub async fn authenticate(
    settings: &ConnectionSettings,
    bind_password: Option<&str>,
    username: &str,
    password: &str,
) -> Result<Option<Entry>> {
    // An empty password would be an unauthenticated bind, which many
    // servers accept (RFC 4513 §5.1.2).
    if password.is_empty() {
        return Ok(None);
    }
    let c = cfg(settings)?;
    let mut ldap = connect(&c).await?;
    let r = async {
        service_bind(&mut ldap, &c, bind_password).await?;
        let Some(entry) = find(&mut ldap, &c, username).await? else {
            return Ok(None);
        };
        let res = ldap
            .with_timeout(OP_TIMEOUT)
            .simple_bind(&entry.dn, password)
            .await
            .map_err(ldap_err)?;
        match res.rc {
            0 => Ok(Some(entry)),
            RC_INVALID_CREDENTIALS => Ok(None),
            rc => Err(upstream(format!(
                "LDAP user bind failed: {rc} {}",
                res.text
            ))),
        }
    }
    .await;
    let _ = ldap.unbind().await;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_escapes_metacharacters() {
        assert_eq!(
            fill("(uid={username})", "a*)(uid=*", ""),
            "(uid=a\\2a\\29\\28uid=\\2a)"
        );
        assert_eq!(
            fill("(member={user_dn})", "", "cn=x\\,y"),
            "(member=cn=x\\5c,y)"
        );
    }

    #[test]
    fn guid_string_is_ad_byte_order() {
        let b = [
            0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        assert_eq!(
            guid_string(&b).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
        assert_eq!(guid_string(&b[..15]), None);
    }
}
