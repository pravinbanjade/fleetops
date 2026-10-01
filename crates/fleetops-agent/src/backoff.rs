//! Reconnect delays: doubling from `min` to `max`, ±20 % jitter so a fleet
//! of agents does not reconnect in lockstep after a server restart.

use rand::Rng;
use std::time::Duration;

pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(self.max);
        base.mul_f64(rand::thread_rng().gen_range(0.8..=1.2))
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn doubles_to_the_cap_with_jitter_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let expect = [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0];
        for want in expect {
            let got = b.next_delay().as_secs_f64();
            assert!(
                got >= want * 0.8 - 1e-9 && got <= want * 1.2 + 1e-9,
                "want ~{want}, got {got}"
            );
        }
        b.reset();
        assert!(b.next_delay() <= Duration::from_millis(1200));
    }
}
