pub mod cookie;
pub mod extract;
pub mod limiter;
pub mod password;
pub mod setup;
pub mod token;

pub const SESSION_IDLE_SECS: i64 = 7 * 86_400;
pub const SESSION_ABSOLUTE_SECS: i64 = 30 * 86_400;
/// Minimum gap between `last_seen_at` writes, to avoid a DB write per request.
pub const SESSION_TOUCH_SECS: i64 = 60;
pub const CSRF_HEADER: &str = "x-csrf-token";
