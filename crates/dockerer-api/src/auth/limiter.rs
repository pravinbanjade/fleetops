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
}
