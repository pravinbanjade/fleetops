//! Deploy keys for SSH git sources (stack git SSH spec §2): ed25519 keys
//! FleetOps generates, or unencrypted ed25519 / ECDSA keys users paste.

use rand::TryRng;
use russh::keys::ssh_key::{private::Ed25519Keypair, private::KeypairData, LineEnding};
use russh::keys::{Algorithm, PrivateKey};

/// A deploy key: the private key as OpenSSH text, and its public line.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyPair {
    pub private: String,
    /// `ssh-ed25519 AAAA… comment`, as added to a git host.
    pub public: String,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair")
            .field("private", &"***")
            .field("public", &self.public)
            .finish()
    }
}

fn pair(key: &PrivateKey) -> KeyPair {
    KeyPair {
        private: key
            .to_openssh(LineEnding::LF)
            .expect("a decoded key encodes")
            .to_string(),
        public: key
            .public_key()
            .to_openssh()
            .expect("a decoded key encodes"),
    }
}

/// A new ed25519 key with `comment`.
pub fn generate_key(comment: &str) -> KeyPair {
    let mut seed = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut seed)
        .expect("the OS random source");
    let data = KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed));
    pair(&PrivateKey::new(data, comment).expect("an ed25519 key is valid"))
}

/// A pasted private key: unencrypted OpenSSH ed25519 or ECDSA.
pub fn parse_key(text: &str) -> Result<KeyPair, String> {
    let key = PrivateKey::from_openssh(text.trim())
        .map_err(|_| "that is not an OpenSSH private key".to_string())?;
    if key.is_encrypted() {
        return Err("passphrase-protected keys are not supported".into());
    }
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } => Ok(pair(&key)),
        Algorithm::Rsa { .. } => Err("RSA keys are not supported: use ed25519".into()),
        a => Err(format!(
            "{} keys are not supported: use ed25519",
            a.as_str()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_round_trips() {
        let k = generate_key("fleetops:shop");
        assert!(k.public.starts_with("ssh-ed25519 AAAA"), "{}", k.public);
        assert!(k.public.ends_with(" fleetops:shop"));
        assert!(k.private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert_eq!(parse_key(&k.private).unwrap().public, k.public);
        assert!(!format!("{k:?}").contains("PRIVATE"));
        assert_ne!(generate_key("x").private, generate_key("x").private);
    }

    #[test]
    fn pasted_keys() {
        let ecdsa = include_str!("../tests/fixtures/keys/ecdsa");
        let k = parse_key(ecdsa).unwrap();
        assert!(k.public.starts_with("ecdsa-sha2-nistp256 "), "{}", k.public);
        assert!(k.public.ends_with(" ecdsa-test"));
        assert!(parse_key(include_str!("../tests/fixtures/keys/ed25519"))
            .unwrap()
            .public
            .starts_with("ssh-ed25519 "));
        assert_eq!(
            parse_key(include_str!("../tests/fixtures/keys/rsa")).unwrap_err(),
            "RSA keys are not supported: use ed25519"
        );
        assert_eq!(
            parse_key(include_str!("../tests/fixtures/keys/ed25519_pass")).unwrap_err(),
            "passphrase-protected keys are not supported"
        );
        assert_eq!(
            parse_key("nonsense").unwrap_err(),
            "that is not an OpenSSH private key"
        );
    }
}
