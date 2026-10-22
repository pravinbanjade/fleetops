//! SSH host keys (stack git SSH spec §3): the published keys of the big git
//! hosts, and trust on first use for every other host.

use russh::keys::{HashAlg, PublicKey};

/// Host → the published keys FleetOps accepts for it, checked against
/// docs.github.com, docs.gitlab.com and support.atlassian.com.
const KNOWN: &[(&str, &[&str])] = &[
    (
        "github.com",
        &[
            "ssh-ed25519 SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU",
            "ecdsa-sha2-nistp256 SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM",
        ],
    ),
    (
        "gitlab.com",
        &[
            "ssh-ed25519 SHA256:eUXGGm1YGsMAS7vkcx6JOJdOGHPem5gQp4taiCfCLB8",
            "ecdsa-sha2-nistp256 SHA256:HbW3g8zUjNSksFbqTiUWPWg2Bq1x8xdGUrliXFzSnUw",
        ],
    ),
    (
        "bitbucket.org",
        &[
            "ssh-ed25519 SHA256:ybgmFkzwOSotHTHLJgHO0QN8L0xErw6vd0VhFA9m3SM",
            "ecdsa-sha2-nistp256 SHA256:FC73VB6C4OQLSCrjEayhMp9UMxS97caD/Yyi2bhW/J0",
        ],
    ),
];

/// `<algorithm> SHA256:<fingerprint>`, as stored and shown.
pub fn host_key_text(key: &PublicKey) -> String {
    format!(
        "{} {}",
        key.algorithm().as_str(),
        key.fingerprint(HashAlg::Sha256)
    )
}

fn fingerprint(text: &str) -> &str {
    text.split_once(' ').map_or(text, |(_, f)| f)
}

/// Whether `seen` may be trusted for `host`: a known host's published key,
/// else the pinned key, else anything (trust on first use).
pub(crate) fn check_host_key(host: &str, pinned: Option<&str>, seen: &str) -> Result<(), String> {
    let now = fingerprint(seen);
    if let Some((_, keys)) = KNOWN.iter().find(|(h, _)| h.eq_ignore_ascii_case(host)) {
        return if keys.contains(&seen) {
            Ok(())
        } else {
            Err(format!(
                "the host key of {host} does not match its published key (now {now})"
            ))
        };
    }
    match pinned {
        Some(p) if p != seen => Err(format!(
            "the host key of {host} changed (now {now}); if that is expected, accept the new key in the Git tab"
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let gh = "ssh-ed25519 SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU";
        assert!(check_host_key("github.com", None, gh).is_ok());
        // A pin never overrides the published key.
        assert!(check_host_key("github.com", Some("ssh-ed25519 SHA256:x"), gh).is_ok());
        assert_eq!(
            check_host_key("github.com", Some("ssh-ed25519 SHA256:x"), "ssh-ed25519 SHA256:x")
                .unwrap_err(),
            "the host key of github.com does not match its published key (now SHA256:x)"
        );
        assert!(check_host_key("git.acme", None, "ssh-ed25519 SHA256:a").is_ok());
        assert!(check_host_key(
            "git.acme",
            Some("ssh-ed25519 SHA256:a"),
            "ssh-ed25519 SHA256:a"
        )
        .is_ok());
        assert_eq!(
            check_host_key(
                "git.acme",
                Some("ssh-ed25519 SHA256:a"),
                "ssh-ed25519 SHA256:b"
            )
            .unwrap_err(),
            "the host key of git.acme changed (now SHA256:b); if that is expected, accept the new key in the Git tab"
        );
        let k = crate::generate_key("x");
        let pk = russh::keys::PublicKey::from_openssh(&k.public).unwrap();
        let text = host_key_text(&pk);
        assert!(text.starts_with("ssh-ed25519 SHA256:"), "{text}");
        assert!(!text.ends_with('='), "{text}");
    }
}
