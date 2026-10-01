//! The control stream: the first stream the agent opens. Each message is a
//! big-endian u32 length followed by that many bytes of JSON.

use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The protocol major version. Agents and servers interoperate only
/// within one major; fields may be added within it.
pub const PROTO_MAJOR: u16 = 1;
/// Control messages are small; anything bigger is a protocol error.
const MAX_MSG: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    /// The Docker endpoint the agent forwards to.
    pub docker: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByeReason {
    /// The agent's key was rotated or its environment deleted.
    Revoked,
    /// Another connection with the same key took this one's place.
    Replaced,
    /// The agent's protocol major differs from the server's.
    Incompatible,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    Hello {
        proto: u16,
        agent_version: String,
        host: HostInfo,
    },
    /// `agent_key` is set once, on the connection that used a join token.
    Welcome {
        env_name: String,
        server_version: String,
        agent_key: Option<String>,
    },
    Ping {
        n: u64,
    },
    Pong {
        n: u64,
    },
    Bye {
        reason: ByeReason,
    },
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// `Ok(None)` on a clean end of stream before a new message starts.
pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Msg>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control message of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
