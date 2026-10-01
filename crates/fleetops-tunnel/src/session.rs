//! A yamux session over any byte stream. The agent dials, so it is the
//! yamux client; the server opens the data streams.
//!
//! Built on the `yamux` crate, which buffers each stream up to its own
//! receive window: a stream nobody reads stalls only its own sender.

use async_trait::async_trait;
use std::{
    collections::VecDeque,
    future::poll_fn,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use yamux::{Config, Connection, Mode};

/// Streams the server may open on one session at once (spec §2.2). Opens
/// past this fail with "agent busy" and leave the session alone.
pub const MAX_STREAMS: usize = 256;
/// yamux's own limit, a little above ours: hitting it tears down the whole
/// connection, so our semaphore must always run out first. The headroom
/// covers the agent's control stream.
const YAMUX_MAX_STREAMS: usize = MAX_STREAMS + 8;
/// Each stream starts with yamux's default 256 KiB window; the connection
/// may hand out at most one default window per allowed stream.
const MAX_CONNECTION_WINDOW: usize = YAMUX_MAX_STREAMS * 256 * 1024;

/// One multiplexed byte stream, readable and writable with tokio's traits.
/// A stream this side opened holds one of the session's stream permits
/// until it is dropped.
pub struct StreamHandle {
    inner: Compat<yamux::Stream>,
    _permit: Option<OwnedSemaphorePermit>,
}

impl AsyncRead for StreamHandle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for StreamHandle {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

fn config() -> Config {
    let mut c = Config::default();
    c.set_max_num_streams(YAMUX_MAX_STREAMS);
    c.set_max_connection_receive_window(Some(MAX_CONNECTION_WINDOW));
    c
}

enum Cmd {
    Open(oneshot::Sender<io::Result<Compat<yamux::Stream>>>),
    Close,
}

/// A handle to one running session. Clones share it. The session runs on
/// its own task until the transport closes or `close` is called; dropping
/// handles does not stop it.
#[derive(Clone)]
pub struct Session {
    cmds: mpsc::UnboundedSender<Cmd>,
    closed: watch::Receiver<bool>,
    permits: Arc<Semaphore>,
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
            let mut opening: VecDeque<oneshot::Sender<io::Result<Compat<yamux::Stream>>>> =
                VecDeque::new();
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
                // Closing drops the connection while it is still active:
                // that wakes every stream, so a reader parked on a terminal
                // or follow-logs stream ends at once. (yamux's graceful
                // close leaves parked readers asleep, and can stall on a
                // dead peer.)
                if closing {
                    return Poll::Ready(());
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
                    Poll::Ready(Some(Ok(stream))) => on_inbound(StreamHandle {
                        inner: stream.compat(),
                        _permit: None,
                    }),
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
        Session {
            cmds,
            closed,
            permits: Arc::new(Semaphore::new(MAX_STREAMS)),
        }
    }

    pub async fn open_stream(&self) -> io::Result<StreamHandle> {
        let permit = self.permits.clone().try_acquire_owned().map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("agent busy: {MAX_STREAMS} streams already open"),
            )
        })?;
        let (tx, rx) = oneshot::channel();
        if self.is_closed() || self.cmds.send(Cmd::Open(tx)).is_err() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "tunnel closed"));
        }
        let inner = rx.await.map_err(|_| aborted("tunnel closed"))??;
        Ok(StreamHandle {
            inner,
            _permit: Some(permit),
        })
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
