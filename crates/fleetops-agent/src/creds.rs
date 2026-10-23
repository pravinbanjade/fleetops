//! The agent's long-lived key, in `<data>/agent.key`, readable by its owner only.

use crate::kube_proxy::KubeProxy;
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::{Method, StatusCode};
use std::{fs, io, path::Path};
use std::{path::PathBuf, sync::Arc};

pub const KEY_FILE: &str = "agent.key";

pub fn load_key(dir: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(dir.join(KEY_FILE)) {
        Ok(s) => Ok(Some(s.trim().to_string()).filter(|k| !k.is_empty())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Written to a temporary file and renamed, so a crash never leaves half a key.
pub fn save_key(dir: &Path, key: &str) -> io::Result<()> {
    let tmp = dir.join(format!("{KEY_FILE}.tmp"));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        io::Write::write_all(&mut f, key.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dir.join(KEY_FILE))
}

/// Fails if the key could not be saved, so a join token is never spent
/// by an agent that would lose the key it gets for it.
pub fn probe_writable(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(".write-probe");
    fs::write(&probe, b"ok")?;
    fs::remove_file(probe)
}

/// Where the agent keeps its key: a file in --data, a Secret, or a
/// read-only file (a Swarm agent's Docker secret).
pub enum Keys {
    File(PathBuf),
    Secret(SecretKeys),
    ReadOnly(PathBuf),
}

impl Keys {
    pub async fn load(&self) -> io::Result<Option<String>> {
        match self {
            Keys::File(dir) => load_key(dir),
            Keys::Secret(s) => s.load().await,
            Keys::ReadOnly(file) => match fs::read_to_string(file) {
                Ok(k) => Ok(Some(k.trim().to_string()).filter(|k| !k.is_empty())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    pub async fn save(&self, key: &str) -> io::Result<()> {
        match self {
            Keys::File(dir) => save_key(dir, key),
            Keys::Secret(s) => s.save(key).await,
            Keys::ReadOnly(file) => Err(io::Error::other(format!(
                "{} is read-only",
                file.display()
            ))),
        }
    }

    /// Fails unless a key could be saved, so a join token is never spent
    /// by an agent that would lose the key it gets for it.
    pub async fn probe(&self) -> io::Result<()> {
        match self {
            Keys::File(dir) => probe_writable(dir),
            Keys::Secret(s) => s.probe().await,
            Keys::ReadOnly(file) => fs::metadata(file).map(|_| ()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Keys::File(dir) => format!("the data directory {}", dir.display()),
            Keys::Secret(s) => format!("the Secret {}/{}", s.namespace, s.name),
            Keys::ReadOnly(file) => format!("the key file {}", file.display()),
        }
    }
}

/// The key in Secret `name`, field `key`, through the apiserver.
pub struct SecretKeys {
    proxy: Arc<KubeProxy>,
    pub(crate) namespace: String,
    pub(crate) name: String,
}

const FIELD: &str = "key";

impl SecretKeys {
    pub fn new(proxy: Arc<KubeProxy>, namespace: String, name: String) -> Self {
        Self {
            proxy,
            namespace,
            name,
        }
    }

    fn collection(&self) -> String {
        format!("/api/v1/namespaces/{}/secrets", self.namespace)
    }

    fn path(&self) -> String {
        format!("{}/{}", self.collection(), self.name)
    }

    fn fail(&self, what: &str, status: StatusCode, body: &[u8]) -> io::Error {
        io::Error::other(format!(
            "{what} Secret {}/{}: HTTP {status}: {}",
            self.namespace,
            self.name,
            String::from_utf8_lossy(body)
        ))
    }

    pub async fn load(&self) -> io::Result<Option<String>> {
        let (status, body) = self
            .proxy
            .api(Method::GET, &self.path(), None, vec![])
            .await?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::OK => {
                let v: serde_json::Value =
                    serde_json::from_slice(&body).map_err(io::Error::other)?;
                let Some(b64) = v["data"][FIELD].as_str() else {
                    return Ok(None);
                };
                let bytes = STANDARD.decode(b64).map_err(io::Error::other)?;
                Ok(Some(String::from_utf8_lossy(&bytes).trim().to_string())
                    .filter(|k| !k.is_empty()))
            }
            s => Err(self.fail("reading", s, &body)),
        }
    }

    pub async fn save(&self, key: &str) -> io::Result<()> {
        let data = serde_json::json!({ "data": { FIELD: STANDARD.encode(key) } });
        let (status, body) = self
            .proxy
            .api(
                Method::PATCH,
                &self.path(),
                Some("application/merge-patch+json"),
                data.to_string().into_bytes(),
            )
            .await?;
        match status {
            StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => {
                let secret = serde_json::json!({
                    "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
                    "metadata": { "name": self.name },
                    "data": { FIELD: STANDARD.encode(key) }
                });
                let (status, body) = self
                    .proxy
                    .api(
                        Method::POST,
                        &self.collection(),
                        Some("application/json"),
                        secret.to_string().into_bytes(),
                    )
                    .await?;
                if status == StatusCode::CREATED || status == StatusCode::OK {
                    Ok(())
                } else {
                    Err(self.fail("creating", status, &body))
                }
            }
            s => Err(self.fail("updating", s, &body)),
        }
    }

    /// The Secret must be readable (present or not): checks RBAC up front.
    pub async fn probe(&self) -> io::Result<()> {
        let (status, body) = self
            .proxy
            .api(Method::GET, &self.path(), None, vec![])
            .await?;
        match status {
            StatusCode::OK | StatusCode::NOT_FOUND => Ok(()),
            s => Err(self.fail("reading", s, &body)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_loads_back_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_key(dir.path()).unwrap(), None);
        save_key(dir.path(), "fk_abc").unwrap();
        assert_eq!(load_key(dir.path()).unwrap().as_deref(), Some("fk_abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_dir_fails_the_probe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions; the test is meaningless there.
        if std::fs::write(dir.path().join("x"), b"x").is_ok() {
            return;
        }
        assert!(probe_writable(dir.path()).is_err());
    }
}
