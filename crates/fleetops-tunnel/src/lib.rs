//! The agent tunnel: a yamux session over a WebSocket, a small control
//! protocol on the agent's first stream, and raw byte streams for the rest.
//! Nothing here knows about Docker or HTTP.

pub mod forward;
pub mod session;
pub mod testing;

pub use forward::{forward, Target};
pub use session::{Role, Session, StreamHandle, StreamOpener, MAX_STREAMS};
