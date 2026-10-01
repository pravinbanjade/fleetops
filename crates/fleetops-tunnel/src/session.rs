//! A yamux session over any byte stream. The agent dials, so it is the
//! yamux client; the server opens the data streams.
//!
//! Built on the `yamux` crate, which buffers each stream up to its own
//! receive window: a stream nobody reads stalls only its own sender.

use async_trait::async_trait;
use std::{collections::VecDeque, future::poll_fn, io, task::Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use yamux::{Config, Connection, Mode};

/// Streams one session may hold open at once (spec §2.2).
pub const MAX_STREAMS: usize = 256;
/// Each stream starts with yamux's default 256 KiB window; the connection
/// may hand out at most one default window per allowed stream.
const MAX_CONNECTION_WINDOW: usize = MAX_STREAMS * 256 * 1024;

/// One multiplexed byte stream, readable and writable with tokio's traits.
pub type StreamHandle = Compat<yamux::Stream>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

fn config() -> Config {
    let mut c = Config::default();
    c.set_max_num_streams(MAX_STREAMS);
    c.set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW));
    c
}

enum Cmd {
    Open(oneshot::Sender<io::Result<StreamHandle>>),
    Close,
}

/// A handle to one running session. Clones share it. The session runs on
/// its own task until the transport closes or `close` is called; dropping
/// handles does not stop it.
#[derive(Clone)]
pub struct Session {
    cmds: mpsc::UnboundedSender<Cmd>,
    closed: watch::Receiver<bool>,
}

fn aborted(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string())
}

impl Session {
    /// Starts the session. `on_inbound` receives each stream the peer opens;
    /// dropping a stream there resets it.
    pub fn spawn<T, F>(io: T, role: Role, mut on_inbound: F) -> Session
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
        F: FnMut(StreamHandle) + Send + 'static,
    {
        let mode = match role {
            Role::Client => Mode::Client,
            Role::Server => Mode::Server,
        };
        let mut conn = Connection::new(io.compat(), config(), mode);
        let (cmds, mut rx) = mpsc::unbounded_channel();
        let (done, closed) = watch::channel(false);
        tokio::spawn(async move {
            let mut opening: VecDeque<oneshot::Sender<io::Result<StreamHandle>>> = VecDeque::new();
            let mut closing = false;
            let mut handles_gone = false;
            poll_fn(|cx| loop {
                while !handles_gone {
                    match rx.poll_recv(cx) {
                        Poll::Ready(Some(Cmd::Open(tx))) => opening.push_back(tx),
                        Poll::Ready(Some(Cmd::Close)) => closing = true,
                        // Every handle dropped: keep serving inbound streams.
                        Poll::Ready(None) => handles_gone = true,
                        Poll::Pending => break,
                    }
                }
                if closing {
                    return conn.poll_close(cx).map(|_| ());
                }
                while !opening.is_empty() {
                    match conn.poll_new_outbound(cx) {
                        Poll::Ready(r) => {
                            let tx = opening.pop_front().expect("non-empty");
                            let _ = tx.send(r.map(|s| s.compat()).map_err(aborted));
                        }
                        Poll::Pending => break,
                    }
                }
                match conn.poll_next_inbound(cx) {
                    Poll::Ready(Some(Ok(stream))) => on_inbound(stream.compat()),
                    Poll::Ready(Some(Err(e))) => {
                        tracing::debug!(error = %e, "tunnel session ended");
                        return Poll::Ready(());
                    }
                    Poll::Ready(None) => return Poll::Ready(()),
                    Poll::Pending => return Poll::Pending,
                }
            })
            .await;
            for tx in opening {
                let _ = tx.send(Err(aborted("tunnel closed")));
            }
            drop(conn);
            let _ = done.send(true);
        });
        Session { cmds, closed }
    }

    pub async fn open_stream(&self) -> io::Result<StreamHandle> {
        let (tx, rx) = oneshot::channel();
        if self.is_closed() || self.cmds.send(Cmd::Open(tx)).is_err() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "tunnel closed"));
        }
        rx.await.map_err(|_| aborted("tunnel closed"))?
    }

    /// Closes the session and every stream on it.
    pub async fn close(&self) {
        let _ = self.cmds.send(Cmd::Close);
        self.closed().await;
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Resolves once the session has ended, for any reason.
    pub async fn closed(&self) {
        let mut rx = self.closed.clone();
        let _ = rx.wait_for(|c| *c).await;
    }
}

/// Something that can open a fresh byte stream to the agent's Docker socket.
#[async_trait]
pub trait StreamOpener: Send + Sync + 'static {
    async fn open(&self) -> io::Result<StreamHandle>;
}

#[async_trait]
impl StreamOpener for Session {
    async fn open(&self) -> io::Result<StreamHandle> {
        self.open_stream().await
    }
}
