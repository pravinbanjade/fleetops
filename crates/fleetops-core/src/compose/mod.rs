//! Native Compose support (stacks spec §2–§3): parse, interpolate,
//! validate and normalise a Compose file, then plan it against what is
//! running. Pure; the deploy runner in `fleetops-api` does the I/O.

pub mod build;
pub mod docker;
pub mod files;
pub mod interp;
pub mod model;
pub mod parse;
pub mod plan;

use serde::Serialize;
use utoipa::ToSchema;

pub use interp::{interpolate, parse_variables};
pub use model::*;
pub use parse::{parse, validate_stack_name, Parsed};

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
