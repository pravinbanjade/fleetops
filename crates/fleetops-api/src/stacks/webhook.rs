//! Push webhooks from git hosts (stacks spec §8): which one sent it, whether
//! it is signed with the stack's secret, and which branch it is about.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    Gitea,
    GitLab,
    Secret,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::Gitea => "gitea",
            Provider::GitLab => "gitlab",
            Provider::Secret => "secret",
        }
    }
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn hmac_matches(secret: &str, body: &[u8], hex_sig: &str) -> bool {
    let Ok(sig) = hex::decode(hex_sig.trim()) else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

fn same(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Checks the request against `secret`. The first method present decides;
/// a wrong value never falls back to a later one.
pub fn verify(
    headers: &HeaderMap,
    query_secret: Option<&str>,
    body: &[u8],
    secret: &str,
) -> Result<Provider, &'static str> {
    if let Some(sig) = header(headers, "x-hub-signature-256") {
        let ok = sig
            .strip_prefix("sha256=")
            .is_some_and(|hex| hmac_matches(secret, body, hex));
        return ok.then_some(Provider::GitHub).ok_or("bad signature");
    }
    if let Some(sig) =
        header(headers, "x-gitea-signature").or_else(|| header(headers, "x-forgejo-signature"))
    {
        return hmac_matches(secret, body, sig)
            .then_some(Provider::Gitea)
            .ok_or("bad signature");
    }
    if let Some(token) = header(headers, "x-gitlab-token") {
        return same(token, secret)
            .then_some(Provider::GitLab)
            .ok_or("bad token");
    }
    if let Some(q) = query_secret {
        return same(q, secret)
            .then_some(Provider::Secret)
            .ok_or("bad token");
    }
    Err("no signature")
}

/// The branch a push event names, if the body says.
pub fn pushed_branch(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("ref")?
        .as_str()?
        .strip_prefix("refs/heads/")
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "hook-secret";
    const BODY: &[u8] = br#"{"ref":"refs/heads/main"}"#;

    fn sig(body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn github_valid() {
        let h = headers(&[("x-hub-signature-256", format!("sha256={}", sig(BODY)))]);
        assert_eq!(verify(&h, None, BODY, SECRET), Ok(Provider::GitHub));
    }

    #[test]
    fn github_bad() {
        let h = headers(&[("x-hub-signature-256", format!("sha256={}", "00".repeat(32)))]);
        assert_eq!(verify(&h, None, BODY, SECRET), Err("bad signature"));
    }

    #[test]
    fn github_missing_prefix() {
        let h = headers(&[("x-hub-signature-256", sig(BODY))]);
        assert_eq!(verify(&h, None, BODY, SECRET), Err("bad signature"));
    }

    #[test]
    fn gitea_valid() {
        let h = headers(&[("x-gitea-signature", sig(BODY))]);
        assert_eq!(verify(&h, None, BODY, SECRET), Ok(Provider::Gitea));
    }

    #[test]
    fn forgejo_header_valid() {
        let h = headers(&[("x-forgejo-signature", sig(BODY))]);
        assert_eq!(verify(&h, None, BODY, SECRET), Ok(Provider::Gitea));
    }

    #[test]
    fn gitlab_valid() {
        let h = headers(&[("x-gitlab-token", SECRET.into())]);
        assert_eq!(verify(&h, None, BODY, SECRET), Ok(Provider::GitLab));
    }

    #[test]
    fn gitlab_bad() {
        let h = headers(&[("x-gitlab-token", "nope".into())]);
        assert_eq!(verify(&h, None, BODY, SECRET), Err("bad token"));
    }

    #[test]
    fn query_secret_valid() {
        assert_eq!(
            verify(&HeaderMap::new(), Some(SECRET), BODY, SECRET),
            Ok(Provider::Secret)
        );
        assert_eq!(
            verify(&HeaderMap::new(), Some("nope"), BODY, SECRET),
            Err("bad token")
        );
    }

    #[test]
    fn none_present() {
        assert_eq!(
            verify(&HeaderMap::new(), None, BODY, SECRET),
            Err("no signature")
        );
    }

    #[test]
    fn wrong_github_signature_does_not_fall_back_to_query() {
        let h = headers(&[("x-hub-signature-256", "sha256=00".into())]);
        assert_eq!(verify(&h, Some(SECRET), BODY, SECRET), Err("bad signature"));
    }

    #[test]
    fn tampered_body_fails() {
        let h = headers(&[("x-hub-signature-256", format!("sha256={}", sig(BODY)))]);
        assert_eq!(
            verify(&h, None, br#"{"ref":"refs/heads/evil"}"#, SECRET),
            Err("bad signature")
        );
    }

    #[test]
    fn pushed_branch_parsing() {
        assert_eq!(pushed_branch(BODY).as_deref(), Some("main"));
        assert_eq!(
            pushed_branch(br#"{"ref":"refs/heads/feature/x"}"#).as_deref(),
            Some("feature/x")
        );
        assert_eq!(pushed_branch(br#"{"ref":"refs/tags/v1"}"#), None);
        assert_eq!(pushed_branch(b"not json"), None);
        assert_eq!(pushed_branch(br#"{"zen":"ping"}"#), None);
    }
}
