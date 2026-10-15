//! The license file (editions spec §4.1): one line,
//! `base64url(payload_json) "." base64url(signature)`, where the signature
//! is Ed25519 over the payload bytes. Keys are 64 hex characters.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use fleetops_core::edition::Feature;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How long EE features keep working after `expires_at`.
pub const GRACE_SECS: i64 = 14 * 86_400;
/// The payload version this build understands.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub v: u32,
    pub id: String,
    pub customer: String,
    pub issued_at: i64,
    pub expires_at: i64,
    /// `None`: unlimited.
    pub max_users: Option<u32>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    pub payload: Payload,
    /// The payload's features this build knows; unknown names are ignored.
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenseError {
    #[error("not a FleetOps license")]
    Format,
    #[error("signature does not verify")]
    Signature,
    #[error("unsupported license version {0}")]
    Version(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = LicenseStatus)]
pub enum Status {
    None,
    Invalid,
    Valid,
    Grace,
    Expired,
}

impl Status {
    /// Whether EE features are on.
    pub fn active(self) -> bool {
        matches!(self, Status::Valid | Status::Grace)
    }
}

impl License {
    pub fn status_at(&self, now: i64) -> Status {
        let exp = self.payload.expires_at;
        if now < exp {
            Status::Valid
        } else if now < exp + GRACE_SECS {
            Status::Grace
        } else {
            Status::Expired
        }
    }
}

pub fn sign(payload: &Payload, key: &SigningKey) -> String {
    let bytes = serde_json::to_vec(payload).expect("payload serialises");
    let sig = key.sign(&bytes);
    format!("{}.{}", B64.encode(&bytes), B64.encode(sig.to_bytes()))
}

/// Checks the signature against any of `keys`, then the version, then
/// parses the payload. Whitespace anywhere (a key wrapped by an email
/// client) and a byte-order mark are ignored: neither base64url nor the
/// separator uses them.
pub fn verify(text: &str, keys: &[VerifyingKey]) -> Result<License, LicenseError> {
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{feff}')
        .collect();
    let (p, s) = compact.split_once('.').ok_or(LicenseError::Format)?;
    let bytes = B64.decode(p).map_err(|_| LicenseError::Format)?;
    let sig: [u8; 64] = B64
        .decode(s)
        .map_err(|_| LicenseError::Format)?
        .try_into()
        .map_err(|_| LicenseError::Format)?;
    let sig = Signature::from_bytes(&sig);
    if !keys.iter().any(|k| k.verify(&bytes, &sig).is_ok()) {
        return Err(LicenseError::Signature);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| LicenseError::Format)?;
    match value.get("v").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(VERSION) => {}
        Some(v) => return Err(LicenseError::Version(u32::try_from(v).unwrap_or(u32::MAX))),
        None => return Err(LicenseError::Format),
    }
    let payload: Payload = serde_json::from_value(value).map_err(|_| LicenseError::Format)?;
    let features = payload
        .features
        .iter()
        .filter_map(|f| Feature::parse(f))
        .collect();
    Ok(License { payload, features })
}

fn key_bytes(hex_text: &str) -> Result<[u8; 32], LicenseError> {
    hex::decode(hex_text.trim())
        .map_err(|_| LicenseError::Format)?
        .try_into()
        .map_err(|_| LicenseError::Format)
}

pub fn verifying_key_from_hex(hex_text: &str) -> Result<VerifyingKey, LicenseError> {
    VerifyingKey::from_bytes(&key_bytes(hex_text)?).map_err(|_| LicenseError::Format)
}

pub fn signing_key_from_hex(hex_text: &str) -> Result<SigningKey, LicenseError> {
    Ok(SigningKey::from_bytes(&key_bytes(hex_text)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn payload(expires_at: i64) -> Payload {
        Payload {
            v: VERSION,
            id: "lic-1".into(),
            customer: "Acme".into(),
            issued_at: 0,
            expires_at,
            max_users: Some(10),
            features: vec!["audit_export".into(), "from_the_future".into()],
        }
    }

    #[test]
    fn signed_license_verifies_and_ignores_unknown_features() {
        let key = SigningKey::generate(&mut OsRng);
        let text = sign(&payload(100), &key);
        let lic = verify(&format!("\n  {text}\n"), &[key.verifying_key()]).unwrap();
        assert_eq!(lic.payload.customer, "Acme");
        assert_eq!(lic.features, vec![Feature::AuditExport]);
    }

    #[test]
    fn bom_and_line_wrapped_pastes_verify() {
        let key = SigningKey::generate(&mut OsRng);
        let text = sign(&payload(100), &key);
        let wrapped: String = text
            .as_bytes()
            .chunks(60)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("\r\n");
        assert!(verify(&format!("\u{feff}{wrapped}\n"), &[key.verifying_key()]).is_ok());
    }

    #[test]
    fn wrong_key_and_tampering_fail() {
        let key = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let text = sign(&payload(100), &key);
        assert_eq!(
            verify(&text, &[other.verifying_key()]),
            Err(LicenseError::Signature)
        );
        let (p, s) = text.split_once('.').unwrap();
        let forged = B64.encode(
            serde_json::to_vec(&Payload {
                max_users: None,
                ..payload(100)
            })
            .unwrap(),
        );
        assert_eq!(
            verify(&format!("{forged}.{s}"), &[key.verifying_key()]),
            Err(LicenseError::Signature)
        );
        assert_eq!(
            verify(&format!("{p}.AAAA"), &[key.verifying_key()]),
            Err(LicenseError::Format)
        );
        assert_eq!(
            verify("not a license", &[key.verifying_key()]),
            Err(LicenseError::Format)
        );
    }

    #[test]
    fn unknown_version_is_rejected() {
        let key = SigningKey::generate(&mut OsRng);
        let text = sign(
            &Payload {
                v: 2,
                ..payload(100)
            },
            &key,
        );
        assert_eq!(
            verify(&text, &[key.verifying_key()]),
            Err(LicenseError::Version(2))
        );
    }

    #[test]
    fn status_at_each_boundary() {
        let key = SigningKey::generate(&mut OsRng);
        let lic = verify(&sign(&payload(1_000), &key), &[key.verifying_key()]).unwrap();
        assert_eq!(lic.status_at(999), Status::Valid);
        assert_eq!(lic.status_at(1_000), Status::Grace);
        assert_eq!(lic.status_at(1_000 + GRACE_SECS - 1), Status::Grace);
        assert_eq!(lic.status_at(1_000 + GRACE_SECS), Status::Expired);
        assert!(Status::Grace.active() && Status::Valid.active());
        assert!(!Status::Expired.active() && !Status::None.active() && !Status::Invalid.active());
    }

    #[test]
    fn keys_round_trip_through_hex() {
        let key = SigningKey::generate(&mut OsRng);
        let seed = hex::encode(key.to_bytes());
        let public = hex::encode(key.verifying_key().to_bytes());
        assert_eq!(
            signing_key_from_hex(&format!("{seed}\n"))
                .unwrap()
                .to_bytes(),
            key.to_bytes()
        );
        assert_eq!(
            verifying_key_from_hex(&public).unwrap(),
            key.verifying_key()
        );
        assert_eq!(verifying_key_from_hex("zz"), Err(LicenseError::Format));
    }
}
