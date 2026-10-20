//! Native Compose support (stacks spec §2–§3): parse, interpolate,
//! validate and normalise a Compose file, then plan it against what is
//! running. Pure; the deploy runner in `fleetops-api` does the I/O.

pub mod interp;

use serde::Serialize;
use utoipa::ToSchema;

pub use interp::{interpolate, parse_variables};

/// One problem in a Compose file, at its YAML path (`services.web.ports[1]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ComposeIssue {
    pub path: String,
    pub message: String,
}

impl ComposeIssue {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}
