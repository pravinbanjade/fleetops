//! An in-memory tunnel for tests in other crates: no WebSocket, no agent
//! binary, the same yamux session and forwarding.

use crate::{forward, Role, Session, Target};

/// The server side of a tunnel whose agent side forwards every stream to
/// `target`. The agent side runs until the server side closes.
pub fn loopback(target: Target) -> Session {
    let (server_io, agent_io) = tokio::io::duplex(1 << 20);
    let _agent = Session::spawn(agent_io, Role::Client, move |stream| {
        let target = target.clone();
        tokio::spawn(async move {
            if let Err(e) = forward(stream, &target).await {
                tracing::debug!(error = %e, "loopback forward ended");
            }
        });
    });
    Session::spawn(server_io, Role::Server, |_unexpected| {})
}
