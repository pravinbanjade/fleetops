//! The server's side of the tunnel transport: axum's WebSocket as bytes.

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use fleetops_tunnel::ws::WsIo;
use futures_util::{future, SinkExt, StreamExt};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

fn to_io(e: axum::Error) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, e)
}

pub fn io(ws: WebSocket) -> impl AsyncRead + AsyncWrite + Send + Unpin + 'static {
    let adapted = ws
        .sink_map_err(to_io)
        .with(|b: Bytes| future::ready(Ok::<_, io::Error>(Message::Binary(b))))
        .take_while(|m| future::ready(!matches!(m, Ok(Message::Close(_)))))
        .filter_map(|m| {
            future::ready(match m {
                Ok(Message::Binary(b)) => Some(Ok(b)),
                Ok(_) => None,
                Err(e) => Some(Err(to_io(e))),
            })
        });
    WsIo::new(adapted)
}
