//! The public keys licenses are checked against.

use crate::license::verifying_key_from_hex;
use ed25519_dalek::VerifyingKey;

/// The production key, plus the committed test key when built with
/// `test-license-key` (tests and the EE e2e suite only).
pub fn trusted() -> Vec<VerifyingKey> {
    #[allow(unused_mut)]
    let mut keys = vec![verifying_key_from_hex(include_str!("../keys/license.pub"))
        .expect("embedded production key")];
    #[cfg(feature = "test-license-key")]
    keys.push(verifying_key_from_hex(include_str!("../keys/test.pub")).expect("embedded test key"));
    keys
}

#[cfg(test)]
mod tests {
    use crate::license::{sign, signing_key_from_hex, verify, Payload, VERSION};

    fn signed_by_test_key() -> String {
        let key = signing_key_from_hex(include_str!("../testdata/test.key")).unwrap();
        let p = Payload {
            v: VERSION,
            id: "t".into(),
            customer: "T".into(),
            issued_at: 0,
            expires_at: 1,
            max_users: None,
            features: vec![],
        };
        sign(&p, &key)
    }

    #[cfg(feature = "test-license-key")]
    #[test]
    fn test_key_is_trusted_with_the_feature() {
        assert!(verify(&signed_by_test_key(), &super::trusted()).is_ok());
    }

    #[cfg(not(feature = "test-license-key"))]
    #[test]
    fn test_key_is_not_trusted_by_default() {
        assert_eq!(
            verify(&signed_by_test_key(), &super::trusted()),
            Err(crate::license::LicenseError::Signature)
        );
    }
}
