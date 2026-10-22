//! SSH git sources (stack git SSH spec §1, §4): URLs, and fetches over an
//! in-process SSH client that runs `git-upload-pack` on the server.

use crate::host_keys::{check_host_key, host_key_text};
use crate::{Fail, Source};
use russh::client::{Handle, Msg};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelStream;
use std::sync::{Arc, Mutex};
use tokio::io::{ReadHalf, WriteHalf};
use tokio_util::io::SyncIoBridge;

/// The host key a fetch saw, shared with the caller, which pins it when the
/// stack has none yet. Set whenever a handshake reaches the host key check,
/// whether or not the fetch then succeeds.
#[derive(Clone, Default, Debug)]
pub struct SeenHostKey(Arc<Mutex<Option<String>>>);

impl SeenHostKey {
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set(&self, key: String) {
        *self.0.lock().unwrap() = Some(key);
    }
}

struct Client {
    host: String,
    pinned: Option<String>,
    seen: SeenHostKey,
    /// Why the host key was refused.
    refused: Arc<Mutex<Option<String>>>,
}

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            *self.refused.lock().unwrap() = Some(format!(
                "{} offers a host certificate, which is not supported",
                self.host
            ));
            return Ok(false);
        };
        let text = host_key_text(key);
        self.seen.set(text.clone());
        match check_host_key(&self.host, self.pinned.as_deref(), &text) {
            Ok(()) => Ok(true),
            Err(m) => {
                *self.refused.lock().unwrap() = Some(m);
                Ok(false)
            }
        }
    }
}

type Stream = ChannelStream<Msg>;

/// What gitoxide reads and writes: `git-upload-pack` on the server.
pub(crate) type SshConnection = gix_transport::client::git::blocking_io::Connection<
    SyncIoBridge<ReadHalf<Stream>>,
    SyncIoBridge<WriteHalf<Stream>>,
>;

/// An SSH session running `git-upload-pack` for one fetch.
pub(crate) struct SshSession {
    handle: Handle<Client>,
    stream: Stream,
    path: String,
    url: String,
}

/// Connects, checks the host key, authenticates with the source's key and
/// starts `git-upload-pack`.
pub(crate) async fn connect(src: &Source) -> Result<SshSession, Fail> {
    let target = ssh_target(&src.url).map_err(Fail::Invalid)?;
    let net = |e: russh::Error| Fail::Network(e.to_string());
    let refused = Arc::new(Mutex::new(None));
    let client = Client {
        host: target.host.clone(),
        pinned: src.host_key.clone(),
        seen: src.seen_host_key.clone(),
        refused: refused.clone(),
    };
    let host = target.host.trim_start_matches('[').trim_end_matches(']');
    let config = Arc::new(russh::client::Config::default());
    let mut handle = match russh::client::connect(config, (host, target.port), client).await {
        Ok(h) => h,
        Err(e) => {
            return Err(match refused.lock().unwrap().take() {
                Some(m) => Fail::HostKey(m),
                None => net(e),
            })
        }
    };
    let Some(text) = src.ssh_key.as_deref().filter(|k| !k.is_empty()) else {
        return Err(Fail::NeedsKey);
    };
    let key = PrivateKey::from_openssh(text.trim())
        .map_err(|_| Fail::Invalid("the stored SSH key cannot be read".into()))?;
    let auth = handle
        .authenticate_publickey(
            &target.user,
            PrivateKeyWithHashAlg::new(Arc::new(key), None),
        )
        .await
        .map_err(net)?;
    if !auth.success() {
        return Err(Fail::RefusedKey);
    }
    let channel = handle.channel_open_session().await.map_err(net)?;
    // Servers that do not accept the variable speak protocol v1.
    let _ = channel.set_env(false, "GIT_PROTOCOL", "version=2").await;
    channel
        .exec(true, upload_pack_command(&target.path))
        .await
        .map_err(net)?;
    Ok(SshSession {
        handle,
        stream: channel.into_stream(),
        path: target.path,
        url: src.url.clone(),
    })
}

impl SshSession {
    /// The blocking git connection over the channel, and what ends the
    /// session. Called inside the runtime; the connection is then used on a
    /// blocking thread.
    pub(crate) fn split(self) -> (SshConnection, Closer) {
        let (r, w) = tokio::io::split(self.stream);
        let conn = gix_transport::client::git::blocking_io::Connection::new(
            SyncIoBridge::new(r),
            SyncIoBridge::new(w),
            gix_transport::Protocol::V2,
            self.path,
            None::<(String, Option<u16>)>,
            gix_transport::client::git::ConnectMode::Process,
            false,
        )
        .custom_url(Some(self.url.into()));
        (conn, Closer(self.handle))
    }
}

/// Ends the session, so a blocked read on the channel ends.
pub(crate) struct Closer(Handle<Client>);

impl Closer {
    pub(crate) async fn close(&self) {
        let _ = self
            .0
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
    }
}

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
