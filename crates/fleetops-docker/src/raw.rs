//! Raw JSON reads beside bollard, for the answers its model loses: Docker
//! returns a service's healthcheck as `Healthcheck`, while bollard's model
//! names it `HealthCheck` and drops it on read. One HTTP/1.0 GET per stream,
//! so Docker answers without chunking and closes when done.

use fleetops_core::{Error, Result};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// GETs a Docker API path: (status, body).
pub(crate) type RawGet =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<(u16, Vec<u8>)>> + Send + Sync>;

/// Answers bigger than this are refused: a service is a few KiB.
const MAX: usize = 4 * 1024 * 1024;

pub(crate) async fn get_over<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    path: &str,
) -> Result<(u16, Vec<u8>)> {
    let io = |e: std::io::Error| Error::EnvUnavailable(format!("docker: {e}"));
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: docker\r\n\r\n").as_bytes())
        .await
        .map_err(io)?;
    s.flush().await.map_err(io)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = s.read(&mut chunk).await.map_err(io)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX {
            return Err(Error::Internal("docker's answer is too large".into()));
        }
        // A server that keeps the stream open still says how long it is.
        if let Some((head, body)) = split(&buf) {
            if content_length(head).is_some_and(|l| body.len() >= l) {
                break;
            }
        }
    }
    let (head, body) =
        split(&buf).ok_or_else(|| Error::Internal("docker sent no HTTP answer".into()))?;
    let status = std::str::from_utf8(head)
        .ok()
        .and_then(|h| h.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Internal("docker sent no HTTP status".into()))?;
    let body = match content_length(head) {
        Some(l) => body[..l.min(body.len())].to_vec(),
        None => body.to_vec(),
    };
    Ok((status, body))
}

fn split(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((&buf[..i], &buf[i + 4..]))
}

fn content_length(head: &[u8]) -> Option<usize> {
    std::str::from_utf8(head).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse().ok())
            .flatten()
    })
}

/// Over a unix socket.
pub(crate) fn unix(path: std::path::PathBuf) -> RawGet {
    Arc::new(move |p: String| {
        let path = path.clone();
        Box::pin(async move {
            let s = tokio::net::UnixStream::connect(&path)
                .await
                .map_err(|e| Error::EnvUnavailable(format!("docker: {e}")))?;
            get_over(s, &p).await
        })
    })
}

/// Over a fresh agent tunnel stream.
pub(crate) fn tunnel(opener: Arc<dyn fleetops_tunnel::StreamOpener>) -> RawGet {
    Arc::new(move |p: String| {
        let opener = opener.clone();
        Box::pin(async move {
            let s = opener
                .open()
                .await
                .map_err(|e| Error::EnvUnavailable(format!("agent stream: {e}")))?;
            get_over(s, &p).await
        })
    })
}
