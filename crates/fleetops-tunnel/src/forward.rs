//! The agent's half of a data stream: a byte pipe to the Docker endpoint.

use crate::StreamHandle;
use std::{io, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Unix(PathBuf),
    Tcp(String),
}

impl Target {
    /// Accepts the Docker URL forms the agent's `--docker` flag takes.
    pub fn parse(url: &str) -> io::Result<Target> {
        let bad = || {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported Docker URL '{url}'"),
            )
        };
        if let Some(path) = url.strip_prefix("unix://") {
            return if path.is_empty() {
                Err(bad())
            } else {
                Ok(Target::Unix(path.into()))
            };
        }
        for scheme in ["tcp://", "http://"] {
            if let Some(addr) = url.strip_prefix(scheme) {
                let addr = addr.trim_end_matches('/');
                return if addr.is_empty() {
                    Err(bad())
                } else {
                    Ok(Target::Tcp(addr.into()))
                };
            }
        }
        Err(bad())
    }
}

/// Copies bytes both ways between `stream` and a new connection to
/// `target` until either side closes. A failed connect drops the stream,
/// which resets it for the server.
pub async fn forward(mut stream: StreamHandle, target: &Target) -> io::Result<()> {
    match target {
        Target::Unix(path) => {
            let mut up = tokio::net::UnixStream::connect(path).await?;
            tokio::io::copy_bidirectional(&mut stream, &mut up).await?;
        }
        Target::Tcp(addr) => {
            let mut up = tokio::net::TcpStream::connect(addr).await?;
            up.set_nodelay(true)?;
            tokio::io::copy_bidirectional(&mut stream, &mut up).await?;
        }
    }
    Ok(())
}
