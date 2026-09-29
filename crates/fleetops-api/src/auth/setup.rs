use std::time::{Duration, Instant};

/// First-run setup is only accepted for a short window after process start,
/// so a stranger cannot claim a forgotten, exposed instance.
pub struct SetupGuard {
    started: Instant,
    window: Duration,
}

impl SetupGuard {
    pub fn new(started: Instant, window: Duration) -> Self {
        Self { started, window }
    }

    pub fn is_open(&self, now: Instant) -> bool {
        now.duration_since(self.started) < self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_only_within_window() {
        let t0 = Instant::now();
        let g = SetupGuard::new(t0, Duration::from_secs(600));
        assert!(g.is_open(t0));
        assert!(g.is_open(t0 + Duration::from_secs(599)));
        assert!(!g.is_open(t0 + Duration::from_secs(600)));
        assert!(!SetupGuard::new(t0, Duration::ZERO).is_open(t0));
    }
}
