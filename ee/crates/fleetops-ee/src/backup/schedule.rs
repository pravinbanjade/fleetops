//! When a scheduled backup is due, and the loop that runs it.

use super::{db, run::Runner};
use fleetops_api::AppState;
use fleetops_core::now_unix;
use std::{sync::Arc, time::Duration};

const DAY: i64 = 86_400;
/// How long after startup a catch-up run waits.
pub const CATCH_UP_DELAY: i64 = 120;

/// The most recent `hour_utc:00` UTC at or before `now`.
pub fn last_slot(now: i64, hour_utc: u8) -> i64 {
    let slot = now - now.rem_euclid(DAY) + i64::from(hour_utc) * 3600;
    if slot > now {
        slot - DAY
    } else {
        slot
    }
}

/// The next `hour_utc:00` UTC after `now`.
pub fn next_slot(now: i64, hour_utc: u8) -> i64 {
    last_slot(now, hour_utc) + DAY
}

/// What run, if any, is due now.
pub fn due(
    now: i64,
    hour_utc: u8,
    last_attempt: Option<i64>,
    last_success: Option<i64>,
    started_at: i64,
) -> Option<db::Trigger> {
    let slot = last_slot(now, hour_utc);
    // Only slots that passed while this server ran; one missed while it was
    // down is caught up below, after the startup delay.
    if slot > last_attempt.unwrap_or(started_at).max(started_at) {
        return Some(db::Trigger::Schedule);
    }
    let stale = last_success.is_none_or(|t| t < now - DAY);
    let none_since_start = last_attempt.is_none_or(|t| t < started_at);
    if stale && none_since_start && now >= started_at + CATCH_UP_DELAY {
        return Some(db::Trigger::CatchUp);
    }
    None
}

/// Checks every minute whether a scheduled backup is due, and runs it.
pub async fn run(state: AppState, runner: Arc<Runner>) {
    let started_at = now_unix();
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        if let Err(e) = check(&state, &runner, started_at).await {
            tracing::warn!(error = %e, "scheduled backup check failed");
        }
    }
}

async fn check(state: &AppState, runner: &Runner, started_at: i64) -> fleetops_core::Result<()> {
    let pool = state.store.pool();
    let schedule = db::get_schedule(pool).await?;
    if !schedule.enabled {
        return Ok(());
    }
    let trigger = due(
        now_unix(),
        schedule.hour_utc,
        db::last_attempt_at(pool).await?,
        db::last_success_at(pool).await?,
        started_at,
    );
    if let Some(t) = trigger {
        runner.run(state, t, None).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::Trigger;

    // 2027-01-15 00:00:00 UTC.
    const D: i64 = 1_800_000_000 - 1_800_000_000 % DAY;

    #[test]
    fn runs_at_hour_once_per_day() {
        let start = D - 10 * DAY;
        let at = D + 2 * 3600 + 30;
        assert_eq!(
            due(at, 2, Some(D - DAY + 7200), Some(D - DAY), start),
            Some(Trigger::Schedule)
        );
        // Once it ran, not again until tomorrow.
        assert_eq!(due(at + 600, 2, Some(at), Some(at), start), None);
        assert_eq!(
            due(D + DAY + 7200, 2, Some(at), Some(at), start),
            Some(Trigger::Schedule)
        );
    }

    #[test]
    fn not_before_hour() {
        let start = D - 10 * DAY;
        let ran = D - DAY + 2 * 3600;
        assert_eq!(due(D + 3600, 2, Some(ran), Some(ran), start), None);
    }

    #[test]
    fn catch_up_after_startup_delay() {
        let start = D + 10 * 3600;
        let old = D - 3 * DAY;
        assert_eq!(
            due(start + 30, 2, Some(old), Some(old), start),
            None,
            "too soon"
        );
        assert_eq!(
            due(start + CATCH_UP_DELAY, 2, Some(old), Some(old), start),
            Some(Trigger::CatchUp)
        );
        // Never succeeded at all.
        assert_eq!(
            due(start + 200, 2, None, None, start),
            Some(Trigger::CatchUp)
        );
        // Not twice in one run of the server.
        assert_eq!(due(start + 300, 2, Some(start + 200), None, start), None);
    }

    #[test]
    fn no_catch_up_when_recent_success() {
        let start = D + 10 * 3600;
        let recent = D + 2 * 3600;
        assert_eq!(due(start + 600, 2, Some(recent), Some(recent), start), None);
    }
}
