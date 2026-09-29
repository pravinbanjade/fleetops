use dockerer_core::Error;

/// Docker's own error responses keep their status and message; JSON errors
/// are bugs; everything else is a transport problem, i.e. the environment
/// is unreachable.
pub(crate) fn map_err(e: bollard::errors::Error) -> Error {
    use bollard::errors::Error as B;
    match e {
        B::DockerResponseServerError {
            status_code,
            message,
        } => Error::Upstream {
            status: status_code,
            message,
        },
        e @ (B::JsonDataError { .. } | B::JsonSerdeError { .. }) => {
            Error::Internal(format!("unexpected Docker response: {e}"))
        }
        other => Error::EnvUnavailable(other.to_string()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_errors_keep_status_and_message() {
        let e = map_err(bollard::errors::Error::DockerResponseServerError {
            status_code: 409,
            message: "container is running".into(),
        });
        assert!(
            matches!(e, Error::Upstream { status: 409, ref message } if message == "container is running")
        );
    }

    #[test]
    fn transport_errors_mean_unavailable() {
        let e = map_err(bollard::errors::Error::SocketNotFoundError(
            "/nope.sock".into(),
        ));
        assert!(matches!(e, Error::EnvUnavailable(m) if m.contains("/nope.sock")));
        assert!(matches!(
            map_err(bollard::errors::Error::RequestTimeoutError),
            Error::EnvUnavailable(_)
        ));
    }
}
