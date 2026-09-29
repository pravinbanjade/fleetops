use dockerer_core::{Error, Result};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Beyond this many tracked keys, stale entries are pruned on insert so an
/// attacker spraying usernames cannot grow memory without bound.
pub const MAX_TRACKED_KEYS: usize = 10_000;

struct Entry {
    failures: u32,
    first: Instant,
}

/// Fixed-window failure counter keyed by `limiter_key(username, ip)`.
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

    pub fn check(&self, key: &str, now: Instant) -> Result<()> {
        let map = self.entries.lock().unwrap();
        if let Some(e) = map.get(key) {
            let elapsed = now.duration_since(e.first);
            if elapsed < self.window && e.failures >= self.max_failures {
                let retry_after_secs = (self.window - elapsed).as_secs().max(1);
                return Err(Error::RateLimited { retry_after_secs });
            }
        }
        Ok(())
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut map = self.entries.lock().unwrap();
        if map.len() >= MAX_TRACKED_KEYS {
            map.retain(|_, e| now.duration_since(e.first) < self.window);
        }
        let e = map.entry(key.to_string()).or_insert(Entry {
            failures: 0,
            first: now,
        });
        if now.duration_since(e.first) >= self.window {
            *e = Entry {
                failures: 0,
                first: now,
            };
        }
        e.failures += 1;
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
    fn blocks_after_max_failures_until_window_passes() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(l.check("k", t0).is_ok());
            l.record_failure("k", t0);
        }
        match l.check("k", t0 + Duration::from_secs(10)) {
            Err(Error::RateLimited { retry_after_secs }) => assert_eq!(retry_after_secs, 290),
            other => panic!("expected rate limit, got {other:?}"),
        }
        assert!(l.check("other", t0).is_ok());
        assert!(l.check("k", t0 + W).is_ok());
    }

    #[test]
    fn reset_clears_failures() {
        let l = LoginLimiter::new(1, W);
        let t0 = Instant::now();
        l.record_failure("k", t0);
        assert!(l.check("k", t0).is_err());
        l.reset("k");
        assert!(l.check("k", t0).is_ok());
    }

    #[test]
    fn failures_outside_window_restart_count() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..4 {
            l.record_failure("k", t0);
        }
        l.record_failure("k", t0 + W + Duration::from_secs(1));
        assert!(l.check("k", t0 + W + Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn map_is_bounded() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_KEYS {
            l.record_failure(&format!("k{i}"), t0);
        }
        l.record_failure("new", t0 + W + Duration::from_secs(1));
        assert_eq!(l.len(), 1);
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
