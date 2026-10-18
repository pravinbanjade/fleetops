//! The server's secret key and the AES-256-GCM box sealed with it
//! (spec §3.1). Sealed text is `v1:` + base64(nonce ‖ ciphertext ‖ tag);
//! the caller's AAD (the environment id) binds it to its row.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use fleetops_core::{Error, Result};
use rand::RngCore;
use std::{io::Write, path::Path};

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct SecretKey(pub(crate) [u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl SecretKey {
    pub fn random() -> Self {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        Self(k)
    }

    /// The key as `FLEETOPS_SECRET_KEY` takes it; backups carry it this way.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    pub fn from_base64(text: &str) -> Result<Self> {
        let bad = || Error::Invalid("FLEETOPS_SECRET_KEY must be 32 bytes, base64-encoded".into());
        let bytes = STANDARD.decode(text.trim()).map_err(|_| bad())?;
        let k: [u8; 32] = bytes.try_into().map_err(|_| bad())?;
        Ok(Self(k))
    }

    /// Reads the key file, or creates it (mode 0600) when it is missing.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        let io = |e: std::io::Error| Error::Internal(format!("secret key {}: {e}", path.display()));
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((Self::from_base64(&text)?, false)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::random();
                let mut opts = std::fs::OpenOptions::new();
                opts.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600);
                }
                let mut f = opts.open(path).map_err(io)?;
                writeln!(f, "{}", key.to_base64()).map_err(io)?;
                f.sync_all().map_err(io)?;
                Ok((key, true))
            }
            Err(e) => Err(io(e)),
        }
    }
}

pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn new(key: &SecretKey) -> Self {
        Self {
            cipher: Aes256Gcm::new(&key.0.into()),
        }
    }

    pub fn seal(&self, aad: &[u8], plain: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
            .expect("AES-GCM encryption does not fail for in-memory input");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        format!("{PREFIX}{}", STANDARD.encode(out))
    }

    pub fn open(&self, aad: &[u8], sealed: &str) -> Result<Vec<u8>> {
        let fail =
            || Error::Invalid("credential cannot be decrypted; re-enter the kubeconfig".into());
        let body = sealed.strip_prefix(PREFIX).ok_or_else(fail)?;
        let bytes = STANDARD.decode(body).map_err(|_| fail())?;
        if bytes.len() < NONCE_LEN {
            return Err(fail());
        }
        let (nonce, ct) = bytes.split_at(NONCE_LEN);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| fail())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_text_opens_with_the_same_key_and_aad() {
        let b = SecretBox::new(&SecretKey::random());
        let sealed = b.seal(b"7", b"token: abc");
        assert!(sealed.starts_with("v1:"), "{sealed}");
        assert!(!sealed.contains("abc"));
        assert_eq!(b.open(b"7", &sealed).unwrap(), b"token: abc");
    }

    #[test]
    fn another_key_another_aad_or_a_tampered_text_fails() {
        let b = SecretBox::new(&SecretKey::random());
        let sealed = b.seal(b"7", b"x");
        assert!(SecretBox::new(&SecretKey::random())
            .open(b"7", &sealed)
            .is_err());
        assert!(b.open(b"8", &sealed).is_err());
        let mut t = sealed.clone().into_bytes();
        let last = t.len() - 2;
        t[last] = if t[last] == b'A' { b'B' } else { b'A' };
        assert!(b.open(b"7", std::str::from_utf8(&t).unwrap()).is_err());
        assert!(b.open(b"7", "v2:abc").is_err());
    }

    #[test]
    fn a_key_file_is_created_owner_only_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        let (a, created) = SecretKey::load_or_create(&path).unwrap();
        assert!(created);
        let (b, created) = SecretKey::load_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(a.0, b.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_key_must_be_32_bytes_of_base64() {
        assert!(SecretKey::from_base64("c2hvcnQ=").is_err());
        assert!(SecretKey::from_base64("not base64!").is_err());
        let k = SecretKey::random();
        use base64::Engine;
        let text = base64::engine::general_purpose::STANDARD.encode(k.0);
        assert_eq!(SecretKey::from_base64(&text).unwrap().0, k.0);
    }
}
