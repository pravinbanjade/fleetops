//! SSH git sources (stack git SSH spec §1, §4): URLs, and fetches over an
//! in-process SSH client that runs `git-upload-pack` on the server.

/// Where an SSH URL points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshTarget {
    pub user: String,
    pub host: String,
    pub port: u16,
    /// As the server gets it: `/srv/r.git` for `ssh://h/srv/r.git`,
    /// `acme/r.git` (relative to the user's home) for `h:acme/r.git`.
    pub path: String,
}

const DEFAULT_USER: &str = "git";

/// `ssh://…`, or git's scp-like `[user@]host:path`: no `://`, and no `/`
/// before the first `:`.
pub fn is_ssh_url(url: &str) -> bool {
    if url.starts_with("ssh://") {
        return true;
    }
    if url.contains("://") {
        return false;
    }
    match url.split_once(':') {
        Some((before, _)) => !before.contains('/'),
        None => false,
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

pub(crate) fn ssh_target(url: &str) -> Result<SshTarget, String> {
    const INVALID: &str = "the ssh URL is not valid";
    let (user, host, port, path) = if url.starts_with("ssh://") {
        let u = url::Url::parse(url).map_err(|_| INVALID.to_string())?;
        if u.password().is_some() {
            return Err("an ssh URL must not hold a password; use an SSH key".into());
        }
        if u.query().is_some() || u.fragment().is_some() {
            return Err("the URL must not have a query or fragment".into());
        }
        let host = u.host_str().unwrap_or("").to_string();
        let user = match u.username() {
            "" => DEFAULT_USER.to_string(),
            user => user.to_string(),
        };
        (user, host, u.port().unwrap_or(22), u.path().to_string())
    } else {
        let (front, path) = url.split_once(':').ok_or_else(|| INVALID.to_string())?;
        let (user, host) = match front.split_once('@') {
            Some((u, h)) => (u.to_string(), h.to_string()),
            None => (DEFAULT_USER.to_string(), front.to_string()),
        };
        (user, host, 22, path.to_string())
    };
    if host.is_empty() {
        return Err("the URL has no host".into());
    }
    let host_ok = valid_name(&host) || (host.starts_with('[') && host.ends_with(']'));
    if !host_ok || !valid_name(&user) {
        return Err(INVALID.into());
    }
    if path.trim_start_matches('/').is_empty() || path.starts_with(':') {
        return Err("the URL has no repository path".into());
    }
    Ok(SshTarget {
        user,
        host,
        port,
        path,
    })
}

/// The command the server runs, with `path` as one shell word.
pub(crate) fn upload_pack_command(path: &str) -> String {
    format!("git-upload-pack '{}'", path.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        let t = ssh_target("git@github.com:acme/ops.git").unwrap();
        assert_eq!(
            (t.user.as_str(), t.host.as_str(), t.port, t.path.as_str()),
            ("git", "github.com", 22, "acme/ops.git")
        );
        let t = ssh_target("ssh://deploy@h:2222/srv/r.git").unwrap();
        assert_eq!(
            (t.user.as_str(), t.host.as_str(), t.port, t.path.as_str()),
            ("deploy", "h", 2222, "/srv/r.git")
        );
        assert_eq!(ssh_target("h:r").unwrap().user, "git");
        assert_eq!(ssh_target("ssh://h/r").unwrap().user, "git");
        assert!(is_ssh_url("h:r") && is_ssh_url("ssh://h/r"));
        assert!(!is_ssh_url("https://h/r") && !is_ssh_url("a/b:c"));
    }

    #[test]
    fn upload_pack_command_quotes_the_path() {
        assert_eq!(
            upload_pack_command("acme/ops.git"),
            "git-upload-pack 'acme/ops.git'"
        );
        assert_eq!(
            upload_pack_command("it's here/r"),
            r"git-upload-pack 'it'\''s here/r'"
        );
    }
}
