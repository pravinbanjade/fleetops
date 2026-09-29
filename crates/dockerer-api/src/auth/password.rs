use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use dockerer_core::{Error, Result};
use std::sync::OnceLock;

#[cfg(test)]
static IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static MAX_SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Upper bound on concurrent argon2 runs (19 MiB each), so a flood of
/// login attempts cannot exhaust memory.
fn permits() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 8)
}

async fn run_limited<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, tokio::task::JoinError> {
    static GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _permit = GATE
        .get_or_init(|| tokio::sync::Semaphore::new(permits()))
        .acquire()
        .await
        .expect("argon2 semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering::SeqCst;
            let n = IN_FLIGHT.fetch_add(1, SeqCst) + 1;
            MAX_SEEN.fetch_max(n, SeqCst);
        }
        let out = f();
        #[cfg(test)]
        IN_FLIGHT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        out
    })
    .await
}

/// argon2id with the crate's default (OWASP-recommended) parameters.
/// Runs on the blocking pool because hashing takes tens of milliseconds.
pub async fn hash_password(password: String) -> Result<String> {
    run_limited(move || hash_blocking(&password))
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
    run_limited(move || verify_blocking(&password, &hash))
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
    let _ = run_limited(move || {
        let dummy =
            DUMMY.get_or_init(|| hash_blocking("dockerer-dummy-password").unwrap_or_default());
        verify_blocking(&password, dummy)
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_hashing_is_bounded() {
        let h = hash_password("pw-pw-pw-pw".into()).await.unwrap();
        let tasks: Vec<_> = (0..32)
            .map(|_| tokio::spawn(verify_password("x".into(), h.clone())))
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        let max = MAX_SEEN.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            max >= 1 && max <= permits(),
            "max {max}, permits {}",
            permits()
        );
    }

    #[tokio::test]
    async fn hash_then_verify() {
        let h = hash_password("s3cret-pass".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("s3cret-pass".into(), h.clone()).await);
        assert!(!verify_password("wrong".into(), h).await);
        assert!(!verify_password("x".into(), "not-a-hash".into()).await);
    }
}
