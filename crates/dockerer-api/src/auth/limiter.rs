use dockerer_core::{Error, Result};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Beyond this many tracked keys, stale entries are pruned on insert so an
/// attacker spraying usernames cannot grow memory without bound; if the map
/// is still full, the oldest entry is evicted (hard cap).
pub const MAX_TRACKED_KEYS: usize = 10_000;

struct Entry {
    failures: u32,
    first: Instant,
}

/// Fixed-window attempt counter keyed by `limiter_key(username, ip)`.
pub struct LoginLimiter {
    max_failures: u32,
    window: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

pub fn limiter_key(username: &str, ip: &str) -> String {
    format!("{}|{}", username.to_lowercase(), ip)
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self {
            max_failures,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Counts an attempt *before* the password is verified, so a burst of
    /// concurrent requests cannot all pass the check while argon2 runs.
    /// A successful login calls `reset`.
    pub fn try_acquire(&self, key: &str, now: Instant) -> Result<()> {
        let mut map = self.entries.lock().unwrap();
        if let Some(e) = map.get_mut(key) {
            let elapsed = now.duration_since(e.first);
            if elapsed >= self.window {
                *e = Entry {
                    failures: 0,
                    first: now,
                };
            } else if e.failures >= self.max_failures {
                let retry_after_secs = (self.window - elapsed).as_secs().max(1);
                return Err(Error::RateLimited { retry_after_secs });
            }
            e.failures += 1;
            return Ok(());
        }
        if map.len() >= MAX_TRACKED_KEYS {
            map.retain(|_, e| now.duration_since(e.first) < self.window);
        }
        if map.len() >= MAX_TRACKED_KEYS {
            let oldest = map
                .iter()
                .min_by_key(|(_, e)| e.first)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                map.remove(&k);
            }
        }
        map.insert(
            key.to_string(),
            Entry {
                failures: 1,
                first: now,
            },
        );
        Ok(())
    }

    pub fn reset(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(300);

    #[test]
    fn blocks_after_max_attempts_until_window_passes() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(l.try_acquire("k", t0).is_ok());
        }
        match l.try_acquire("k", t0 + Duration::from_secs(10)) {
            Err(Error::RateLimited { retry_after_secs }) => assert_eq!(retry_after_secs, 290),
            other => panic!("expected rate limit, got {other:?}"),
        }
        assert!(l.try_acquire("other", t0).is_ok());
        assert!(l.try_acquire("k", t0 + W).is_ok());
    }

    #[test]
    fn reset_clears_attempts() {
        let l = LoginLimiter::new(1, W);
        let t0 = Instant::now();
        assert!(l.try_acquire("k", t0).is_ok());
        assert!(l.try_acquire("k", t0).is_err());
        l.reset("k");
        assert!(l.try_acquire("k", t0).is_ok());
    }

    #[test]
    fn attempts_outside_window_restart_count() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..5 {
            l.try_acquire("k", t0).unwrap();
        }
        assert!(l.try_acquire("k", t0 + W + Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn map_is_bounded() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_KEYS {
            l.try_acquire(&format!("k{i}"), t0).unwrap();
        }
        l.try_acquire("new", t0 + W + Duration::from_secs(1))
            .unwrap();
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn map_is_hard_capped_within_window() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_KEYS {
            l.try_acquire(&format!("k{i}"), t0).unwrap();
        }
        l.try_acquire("new", t0 + Duration::from_secs(1)).unwrap();
        assert_eq!(l.len(), MAX_TRACKED_KEYS);
    }

    #[test]
    fn key_is_case_insensitive_on_username() {
        assert_eq!(
            limiter_key("Admin", "1.2.3.4"),
            limiter_key("admin", "1.2.3.4")
        );
        assert_ne!(
            limiter_key("admin", "1.2.3.4"),
            limiter_key("admin", "5.6.7.8")
        );
    }
}
