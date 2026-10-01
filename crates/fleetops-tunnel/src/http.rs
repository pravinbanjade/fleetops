//! A hyper-util connector whose "connections" are yamux streams. Shared by
//! bollard (Docker agents) and kube-rs (Kubernetes agents); both run plain
//! HTTP/1.1 over the stream, and upgrades work because hyper owns it.

use crate::{StreamHandle, StreamOpener};
use hyper::Uri;
use hyper_util::{
    client::legacy::connect::{Connected, Connection},
    rt::TokioIo,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Opens a fresh tunnel stream for every connection hyper asks for. The
/// URI is ignored: the agent decides where the stream goes.
#[derive(Clone)]
pub struct TunnelConnector(Arc<dyn StreamOpener>);

impl TunnelConnector {
    pub fn new(opener: Arc<dyn StreamOpener>) -> Self {
        Self(opener)
    }
}

impl tower_service::Service<Uri> for TunnelConnector {
    type Response = TunnelIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<TunnelIo>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let opener = self.0.clone();
        Box::pin(async move { Ok(TunnelIo(TokioIo::new(opener.open().await?))) })
    }
}

pub struct TunnelIo(TokioIo<StreamHandle>);

impl Connection for TunnelIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for TunnelIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for TunnelIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
