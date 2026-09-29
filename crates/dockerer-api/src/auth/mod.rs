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

/// Cheap pre-check before any rate-limit, DB or argon2 work: input that can
/// never match an account is rejected outright, bounding per-request cost.
pub fn plausible_login(username: &str, password: &str) -> bool {
    dockerer_core::validate_username(username).is_ok()
        && password.chars().count() <= dockerer_core::user::PASSWORD_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_login_rejects_oversize_or_malformed_input() {
        assert!(plausible_login("admin", "anything"));
        assert!(plausible_login("admin", &"x".repeat(256)));
        assert!(!plausible_login("admin", &"x".repeat(257)));
        assert!(!plausible_login(&"a".repeat(65), "pw"));
        assert!(!plausible_login("bad name", "pw"));
        assert!(!plausible_login("", "pw"));
    }
}
