//! Exec-plugin users (spec 3k): which plugins may run, running them, and
//! putting their token on every request.

pub mod layer;
pub mod policy;
pub mod provider;

pub use layer::ExecAuthLayer;
pub use policy::{ExecPolicy, Plugin, BUILT_IN, OFF, PLUGIN_TIMEOUT};
pub use provider::{ExecError, ProcessRunner, Runner, TokenSource};
