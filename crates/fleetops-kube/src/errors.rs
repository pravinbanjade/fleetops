//! kube-rs errors as FleetOps errors (spec §7). The apiserver's message
//! is kept: a 403 says which RBAC rule was missing.

use fleetops_core::Error;

pub fn map(e: kube::Error) -> Error {
    match e {
        kube::Error::Api(s) => match s.code {
            404 => Error::NotFound(s.message),
            403 => Error::Forbidden(s.message),
            409 => Error::Conflict(s.message),
            400 | 422 => Error::Invalid(s.message),
            _ => Error::Upstream {
                status: 502,
                message: s.message,
            },
        },
        other => Error::Upstream {
            status: 502,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn api(code: u16, msg: &str) -> kube::Error {
        let s: kube::core::Status = serde_json::from_value(json!({
            "status": "Failure", "code": code, "message": msg, "reason": "x"}))
        .unwrap();
        kube::Error::Api(Box::new(s))
    }

    #[test]
    fn apiserver_codes_map_to_fleetops_errors() {
        assert!(matches!(map(api(404, "nope")), Error::NotFound(m) if m == "nope"));
        assert!(
            matches!(map(api(403, "pods is forbidden: cannot list")), Error::Forbidden(m) if m.contains("cannot list"))
        );
        assert!(matches!(map(api(409, "c")), Error::Conflict(_)));
        assert!(matches!(map(api(422, "i")), Error::Invalid(_)));
        assert!(matches!(
            map(api(500, "boom")),
            Error::Upstream { status: 502, .. }
        ));
    }
}
