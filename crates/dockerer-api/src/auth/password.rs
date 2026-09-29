use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use dockerer_core::{Error, Result};
use std::sync::OnceLock;

/// argon2id with the crate's default (OWASP-recommended) parameters.
/// Runs on the blocking pool because hashing takes tens of milliseconds.
pub async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_blocking(&password))
        .await
        .map_err(|e| Error::Internal(format!("hash task failed: {e}")))?
}

fn hash_blocking(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("password hashing failed: {e}")))
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_blocking(&password, &hash))
        .await
        .unwrap_or(false)
}

fn verify_blocking(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Spends the same time as a real verification so response timing does not
/// reveal whether a username exists.
pub async fn burn_verify(password: String) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let _ = tokio::task::spawn_blocking(move || {
        let dummy =
            DUMMY.get_or_init(|| hash_blocking("dockerer-dummy-password").unwrap_or_default());
        verify_blocking(&password, dummy)
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_then_verify() {
        let h = hash_password("s3cret-pass".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("s3cret-pass".into(), h.clone()).await);
        assert!(!verify_password("wrong".into(), h).await);
        assert!(!verify_password("x".into(), "not-a-hash".into()).await);
    }
}
