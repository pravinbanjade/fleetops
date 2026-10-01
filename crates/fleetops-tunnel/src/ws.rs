//! A WebSocket as a byte stream: binary messages in, binary messages out.
//! Each side adapts its own WebSocket type into a `Stream` of `Bytes` plus
//! a `Sink` of `Bytes`; `WsIo` turns that into `AsyncRead + AsyncWrite`.

use bytes::{Buf, Bytes};
use futures_util::{future, ready, Sink, SinkExt, Stream, StreamExt};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

/// Largest WebSocket message either side accepts (spec §2.2).
pub const MAX_WS_MESSAGE: usize = 16 << 20;
/// Writes are cut into messages of at most this size.
const MAX_WRITE: usize = 64 * 1024;

pub struct WsIo<S> {
    inner: S,
    pending: Bytes,
}

impl<S> WsIo<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            pending: Bytes::new(),
        }
    }
}

impl<S> AsyncRead for WsIo<S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        while self.pending.is_empty() {
            match ready!(self.inner.poll_next_unpin(cx)) {
                Some(Ok(b)) => self.pending = b,
                Some(Err(e)) => return Poll::Ready(Err(e)),
                None => return Poll::Ready(Ok(())), // EOF
            }
        }
        let n = self.pending.len().min(buf.remaining());
        buf.put_slice(&self.pending[..n]);
        self.pending.advance(n);
        Poll::Ready(Ok(()))
    }
}

impl<S> AsyncWrite for WsIo<S>
where
    S: Sink<Bytes, Error = io::Error> + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        ready!(self.inner.poll_ready_unpin(cx))?;
        let n = buf.len().min(MAX_WRITE);
        self.inner
            .start_send_unpin(Bytes::copy_from_slice(&buf[..n]))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_flush_unpin(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_close_unpin(cx)
    }
}

fn to_io(e: tokio_tungstenite::tungstenite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, e)
}

/// The agent's side (and the tests'): a tokio-tungstenite WebSocket.
pub fn tungstenite_io<T>(
    ws: WebSocketStream<T>,
) -> impl AsyncRead + AsyncWrite + Send + Unpin + 'static
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let adapted = ws
        .sink_map_err(to_io)
        .with(|b: Bytes| future::ready(Ok::<_, io::Error>(Message::Binary(b))))
        .take_while(|m| future::ready(!matches!(m, Ok(Message::Close(_)))))
        .filter_map(|m| {
            future::ready(match m {
                Ok(Message::Binary(b)) => Some(Ok(b)),
                Ok(_) => None, // ping/pong/text: tungstenite answers pings itself
                Err(e) => Some(Err(to_io(e))),
            })
        });
    WsIo::new(adapted)
}
