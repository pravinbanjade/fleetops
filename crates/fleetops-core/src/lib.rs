//! Domain types shared by every fleetops crate. No I/O lives here.

pub mod access;
pub mod docker;
pub mod edition;
pub mod env;
pub mod error;
pub mod helm;
pub mod kube;
pub mod perm;
pub mod recreate;
pub mod streams;
pub mod user;

pub use access::{EnvRole, Grant};
pub use error::{Error, Result};
pub use user::{validate_password, validate_username, Role, User};

/// Current wall-clock time as unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
