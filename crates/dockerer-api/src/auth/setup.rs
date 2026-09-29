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
