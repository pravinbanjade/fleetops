/// The single error type crossing crate boundaries. The API layer maps each
/// variant to an HTTP status and a stable `code` string.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    /// An Enterprise feature the license does not cover (403).
    #[error("this needs a FleetOps Enterprise license with the '{0}' feature")]
    LicenseRequired(String),
    /// Creating a user would pass the license's user limit (403).
    #[error("the license allows {0} users; remove a user or extend the license")]
    SeatLimit(u32),
    /// Adding an environment would pass the license's node limit (403).
    #[error("the license allows {0} nodes; remove an environment or extend the license")]
    NodeLimit(u32),
    #[error("too many attempts; retry in {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("environment unavailable: {0}")]
    EnvUnavailable(String),
    /// An error reported by Docker; `message` is shown to the user verbatim.
    #[error("{message}")]
    Upstream { status: u16, message: String },
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The message without the variant's prefix, for showing next to a
    /// separate `code` (e.g. one document of an apply).
    pub fn detail(&self) -> String {
        match self {
            Error::NotFound(m)
            | Error::Conflict(m)
            | Error::Invalid(m)
            | Error::Unauthorized(m)
            | Error::Forbidden(m)
            | Error::EnvUnavailable(m)
            | Error::Internal(m) => m.clone(),
            Error::Upstream { message, .. } => message.clone(),
            Error::RateLimited { .. }
            | Error::LicenseRequired(_)
            | Error::SeatLimit(_)
            | Error::NodeLimit(_) => self.to_string(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Conflict(_) => "conflict",
            Error::Invalid(_) => "invalid",
            Error::Unauthorized(_) => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::LicenseRequired(_) => "license_required",
            Error::SeatLimit(_) => "seat_limit",
            Error::NodeLimit(_) => "node_limit",
            Error::RateLimited { .. } => "rate_limited",
            Error::EnvUnavailable(_) => "env_unavailable",
            Error::Upstream { .. } => "upstream",
            Error::Internal(_) => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Error::NotFound("x".into()).code(), "not_found");
        assert_eq!(Error::Conflict("x".into()).code(), "conflict");
        assert_eq!(Error::Invalid("x".into()).code(), "invalid");
        assert_eq!(Error::Unauthorized("x".into()).code(), "unauthorized");
        assert_eq!(Error::Forbidden("x".into()).code(), "forbidden");
        assert_eq!(
            Error::RateLimited {
                retry_after_secs: 1
            }
            .code(),
            "rate_limited"
        );
        assert_eq!(Error::EnvUnavailable("x".into()).code(), "env_unavailable");
        assert_eq!(
            Error::Upstream {
                status: 409,
                message: "x".into()
            }
            .code(),
            "upstream"
        );
        assert_eq!(Error::Internal("x".into()).code(), "internal");
        assert_eq!(
            Error::LicenseRequired("audit_export".into()).code(),
            "license_required"
        );
        assert_eq!(Error::SeatLimit(5).code(), "seat_limit");
        assert_eq!(Error::NodeLimit(10).code(), "node_limit");
    }

    #[test]
    fn display_keeps_upstream_message_verbatim() {
        let e = Error::Upstream {
            status: 409,
            message: "container is running".into(),
        };
        assert_eq!(e.to_string(), "container is running");
        assert_eq!(
            Error::NotFound("user 5".into()).to_string(),
            "not found: user 5"
        );
        assert_eq!(
            Error::RateLimited {
                retry_after_secs: 30
            }
            .to_string(),
            "too many attempts; retry in 30s"
        );
    }

    #[test]
    fn detail_drops_the_variant_prefix() {
        assert_eq!(Error::NotFound("pods \"x\"".into()).detail(), "pods \"x\"");
        assert_eq!(Error::Forbidden("no".into()).detail(), "no");
        assert_eq!(
            Error::Upstream {
                status: 502,
                message: "boom".into()
            }
            .detail(),
            "boom"
        );
        assert_eq!(
            Error::RateLimited {
                retry_after_secs: 3
            }
            .detail(),
            "too many attempts; retry in 3s"
        );
    }
}
