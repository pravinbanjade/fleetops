# Sub-project 2: Agent + Multi-host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `fleetops-agent` binary that dials out to the FleetOps server over one WebSocket, so a remote Docker host becomes an ordinary `docker-agent` environment where every existing page works unchanged.

**Architecture:** A new `fleetops-tunnel` crate runs a yamux session (`tokio-yamux`) over a WebSocket. The server's bollard client uses a custom transport whose hyper connector opens a yamux stream per HTTP connection; the agent pipes each stream to its Docker socket byte for byte. Stream 0 (opened by the agent) carries a small JSON control protocol: hello, welcome, ping, bye. Live sessions sit in a `Registry` shared by the env factory (consumer) and the `/api/v1/agent/connect` handler (producer).

**Tech Stack:** Rust 1.86 (edition 2021), tokio, tokio-yamux 0.3, bollard 0.21 (`connect_with_custom_transport`), hyper 1 + hyper-util 0.1 legacy client, axum 0.8 WebSocket, tokio-tungstenite 0.29 (agent side, rustls), sqlx/SQLite, React 19 + TanStack Query, Playwright.

**Spec:** `docs/superpowers/specs/2026-10-01-fleetops-agent-multihost-design.md`

## Global Constraints

- MSRV `rust-version = "1.86"`, edition 2021; new crates use `version.workspace = true`, `edition.workspace = true`, `rust-version.workspace = true`, and add their dependencies to `[workspace.dependencies]` in the root `Cargo.toml`.
- `make lint` (`cargo fmt --check`, `clippy --workspace --all-targets -D warnings`) and `make test` pass after every task.
- Protocol: `PROTO_MAJOR = 1`. The server accepts only `Hello.proto == PROTO_MAJOR`; otherwise `Bye { incompatible }`.
- Join token: 15 minutes, single use, prefix `fj_`. Agent key: prefix `fk_`. The server stores only SHA-256 hex hashes (the existing `auth::token::hash_token`).
- Heartbeat: server `Ping` every 15 s; two unanswered pings end the session (about 45 s).
- Limits: 256 streams per session; WebSocket max frame and message 16 MiB; yamux stream window 256 KiB.
- Agent reconnect backoff: 1 s doubling to 30 s, ±20 % jitter, reset after a session stayed up 60 s.
- Agent connect requires TLS, except from a loopback peer (`127.0.0.1`, `::1`).
- Agent environment URL is `agent://<env-id>`, created by the server only. `validate_env_url` keeps rejecting `agent://` from users.
- Enrollment command pins `ghcr.io/pravinbanjade/fleetops-agent:<server version>` (the server's `CARGO_PKG_VERSION`).
- Agent idle RSS ≤ 15 MiB. Server idle RSS ≤ 50 MiB + 1 MiB per connected agent, measured with 10 agents.
- No changes to the Docker route handlers (`crates/fleetops-api/src/routes/docker/*`). Remote environments work through `Environment::docker()` alone.
- After any API change, run `make openapi` and commit `crates/fleetops-api/openapi.json` and `web/src/api/gen/*`; `cargo test` and `make test-web` fail on drift.

## Review Focus

The spec implies these, but no feature test would catch them by accident. Each one has a test in the task named.

1. **The server restarts while agents are connected.** Agents should reconnect with their stored key and show online again with no admin action. → Task 9, `agent_reconnects_after_the_server_restarts`.
2. **One join command run on two hosts at once.** Exactly one agent should enroll and the other should get 401, never two keys for one environment. → Task 7, `a_join_token_enrolls_exactly_one_agent`.
3. **An environment deleted while its agent is connected.** The agent should get `Bye { revoked }` and exit, and the registry should be empty. → Task 7, `deleting_an_agent_environment_revokes_its_session`.
4. **The tunnel dies under a long-lived stream** (terminal, follow-logs). The stream should end promptly with EOF or an error instead of hanging. → Task 1, `closing_the_session_ends_open_streams`.
5. **The agent's data directory is not writable.** The agent should refuse to start, with a clear message, *before* it spends the single-use join token. → Task 9, `unwritable_data_dir_fails_before_connecting`.

---

## File Structure

```
crates/fleetops-tunnel/            NEW — no Docker or HTTP knowledge
  Cargo.toml
  src/lib.rs                       re-exports
  src/session.rs                   Session (yamux over any AsyncRead+AsyncWrite), Role, StreamOpener
  src/forward.rs                   Target (unix/tcp) and forward(stream, target)
  src/testing.rs                   loopback(target) for tests in other crates
  src/ws.rs                        WsIo: bytes-message Stream+Sink → AsyncRead+AsyncWrite; tungstenite_io()
  src/control.rs                   Msg, HostInfo, ByeReason, PROTO_MAJOR, read_msg/write_msg
  src/registry.rs                  Registry of live agent sessions, Live, AgentInfo
  tests/session.rs, tests/forward.rs, tests/ws.rs, tests/control.rs
crates/fleetops-agent/             NEW — bin `fleetops-agent`
  Cargo.toml
  src/main.rs                      CLI entry
  src/lib.rs                       run(config)
  src/config.rs                    clap Config
  src/creds.rs                     agent.key load/save, writable probe
  src/backoff.rs                   Backoff
  src/dial.rs                      WebSocket connect with bearer and optional extra CA
  src/session.rs                   one connected session: hello/welcome, ping/pong, bye
  tests/agent.rs                   against an in-process server
crates/fleetops-core/src/env.rs              EnvKind::DockerAgent, agent_url(), Environment::wait_change
crates/fleetops-store/migrations/0002_agents.sql
crates/fleetops-store/src/agents.rs          agent credential and status queries
crates/fleetops-store/tests/agents.rs
crates/fleetops-docker/src/tunnel.rs         LocalDocker::through(opener)
crates/fleetops-docker/src/agent_env.rs      AgentDockerEnv
crates/fleetops-docker/src/env.rs            DockerEnvFactory holds Arc<Registry>
crates/fleetops-api/src/agents/mod.rs        tokens, enrollment command, status publishing
crates/fleetops-api/src/agents/connect.rs    GET /api/v1/agent/connect and the server side of a session
crates/fleetops-api/src/agents/axum_io.rs    axum WebSocket → WsIo
crates/fleetops-api/src/routes/envs.rs       agent create, list view, rotate, delete revokes
crates/fleetops-api/src/state.rs             AppState.agents
crates/fleetops-api/src/ws/hub.rs            backoff sleeps wake on Environment::wait_change
crates/fleetops-server/src/lib.rs            one Registry shared by factory and state
web/src/pages/admin/EnvironmentsPage.tsx     kind choice, enrollment command, agent column, rotate
web/src/pages/admin/AgentEnrollment.tsx      one-time command display
web/e2e/agent.spec.ts
Dockerfile, Makefile, .github/workflows/{ci,release}.yml, scripts/{rss-bench,agent-smoke}.sh, README.md
```

---

## Phase A — Prove the tunnel

### Task 1: `fleetops-tunnel` sessions and forwarding

**Files:**
- Create: `crates/fleetops-tunnel/Cargo.toml`, `crates/fleetops-tunnel/src/lib.rs`, `crates/fleetops-tunnel/src/session.rs`, `crates/fleetops-tunnel/src/forward.rs`, `crates/fleetops-tunnel/src/testing.rs`
- Test: `crates/fleetops-tunnel/tests/session.rs`, `crates/fleetops-tunnel/tests/forward.rs`
- Modify: `Cargo.toml` (workspace dependencies)

**Interfaces:**
- Produces:
  - `pub enum Role { Client, Server }`
  - `#[derive(Clone)] pub struct Session` with `pub fn spawn<T, F>(io: T, role: Role, on_inbound: F) -> Session` where `T: AsyncRead + AsyncWrite + Send + Unpin + 'static`, `F: FnMut(StreamHandle) + Send + 'static`; `pub async fn open_stream(&self) -> io::Result<StreamHandle>`; `pub async fn close(&self)`; `pub fn is_closed(&self) -> bool`; `pub async fn closed(&self)`
  - `pub use tokio_yamux::StreamHandle;`
  - `#[async_trait] pub trait StreamOpener: Send + Sync + 'static { async fn open(&self) -> io::Result<StreamHandle>; }`, implemented for `Session`
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub enum Target { Unix(PathBuf), Tcp(String) }` with `pub fn parse(url: &str) -> io::Result<Target>`
  - `pub async fn forward(stream: StreamHandle, target: &Target) -> io::Result<()>`
  - `pub fn testing::loopback(target: Target) -> Session`, which returns the server side of an in-memory tunnel whose agent side forwards to `target`

- [ ] **Step 1: Add the workspace dependencies and the crate manifest**

In the root `Cargo.toml` `[workspace.dependencies]`, add (alphabetical within each block):

```toml
fleetops-tunnel = { path = "crates/fleetops-tunnel" }

bytes = "1"
tokio-yamux = "0.3"
```

Create `crates/fleetops-tunnel/Cargo.toml`:

```toml
[package]
name = "fleetops-tunnel"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
bytes.workspace = true
futures-util = { workspace = true, features = ["sink"] }
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tokio-yamux.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Write the failing session tests**

Create `crates/fleetops-tunnel/tests/session.rs`:

```rust
use fleetops_tunnel::{Role, Session, StreamOpener};
use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

/// A server session whose agent side echoes every stream back.
fn echo_pair() -> (Session, Session) {
    let (a, b) = duplex(1 << 20);
    let server = Session::spawn(a, Role::Server, |_s| {});
    let agent = Session::spawn(b, Role::Client, |s| {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(s);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    });
    (server, agent)
}

#[tokio::test]
async fn server_opens_streams_the_agent_serves() {
    let (server, _agent) = echo_pair();
    let mut tasks = Vec::new();
    for i in 0..3 {
        let server = server.clone();
        tasks.push(tokio::spawn(async move {
            let mut s = server.open().await.unwrap();
            let msg = format!("hello-{i}");
            s.write_all(msg.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
            let mut back = String::new();
            s.read_to_string(&mut back).await.unwrap();
            assert_eq!(back, msg);
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
}

#[tokio::test]
async fn a_slow_reader_stalls_only_its_own_stream() {
    let (server, _agent) = echo_pair();
    // Stream A: write far more than the 256 KiB window and never read the echo.
    let mut a = server.open_stream().await.unwrap();
    let stalled = tokio::time::timeout(Duration::from_millis(500), async {
        let chunk = vec![7u8; 64 * 1024];
        for _ in 0..128 {
            a.write_all(&chunk).await.unwrap();
        }
    })
    .await;
    assert!(stalled.is_err(), "8 MiB went through with nobody reading");

    // Stream B still round-trips.
    let mut b = server.open_stream().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        b.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    })
    .await
    .expect("stream B was blocked by stream A");
}

#[tokio::test]
async fn closing_the_session_ends_open_streams() {
    let (server, agent) = echo_pair();
    let mut s = server.open_stream().await.unwrap();
    s.write_all(b"x").await.unwrap();
    let mut one = [0u8; 1];
    s.read_exact(&mut one).await.unwrap();

    server.close().await;

    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(1), s.read(&mut buf))
        .await
        .expect("an open stream hung after its session closed");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    tokio::time::timeout(Duration::from_secs(1), agent.closed())
        .await
        .expect("the agent side never noticed");
    assert!(server.is_closed());
    assert!(server.open_stream().await.is_err());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p fleetops-tunnel --test session`
Expected: FAIL to compile (`unresolved import fleetops_tunnel::Session`).

- [ ] **Step 4: Implement the session**

Create `crates/fleetops-tunnel/src/session.rs`:

```rust
//! A yamux session over any byte stream. The agent dials, so it is the
//! yamux client; the server opens the data streams.

use async_trait::async_trait;
use futures_util::StreamExt;
use std::{io, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_yamux::{Config, Control, Session as Yamux, StreamHandle};

/// Streams one session may hold open at once (spec §2.2).
pub const MAX_STREAMS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

fn config() -> Config {
    Config {
        max_stream_count: MAX_STREAMS,
        // Liveness is the control stream's job (Ping/Pong), so a stalled
        // peer is reported with a reason rather than a yamux timeout.
        enable_keepalive: false,
        connection_write_timeout: Duration::from_secs(30),
        ..Config::default()
    }
}

/// A handle to one running session. Clones share it. The session runs on
/// its own task until the transport closes or `close` is called; dropping
/// handles does not stop it.
#[derive(Clone)]
pub struct Session {
    control: Control,
    closed: watch::Receiver<bool>,
}

impl Session {
    /// Starts the session. `on_inbound` receives each stream the peer opens;
    /// dropping a stream there resets it.
    pub fn spawn<T, F>(io: T, role: Role, mut on_inbound: F) -> Session
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
        F: FnMut(StreamHandle) + Send + 'static,
    {
        let mut yamux = match role {
            Role::Client => Yamux::new_client(io, config()),
            Role::Server => Yamux::new_server(io, config()),
        };
        let control = yamux.control();
        let (tx, closed) = watch::channel(false);
        tokio::spawn(async move {
            while let Some(item) = yamux.next().await {
                match item {
                    Ok(stream) => on_inbound(stream),
                    Err(e) => {
                        tracing::debug!(error = %e, "tunnel session ended");
                        break;
                    }
                }
            }
            let _ = tx.send(true);
        });
        Session { control, closed }
    }

    pub async fn open_stream(&self) -> io::Result<StreamHandle> {
        if self.is_closed() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "tunnel closed"));
        }
        self.control
            .clone()
            .open_stream()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionAborted, format!("{e:?}")))
    }

    /// Closes the session and every stream on it.
    pub async fn close(&self) {
        self.control.clone().close().await;
        let mut rx = self.closed.clone();
        let _ = tokio::time::timeout(Duration::from_secs(1), rx.wait_for(|c| *c)).await;
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
```

Create `crates/fleetops-tunnel/src/lib.rs`:

```rust
//! The agent tunnel: a yamux session over a WebSocket, a small control
//! protocol on the agent's first stream, and raw byte streams for the rest.
//! Nothing here knows about Docker or HTTP.

pub mod forward;
pub mod session;
pub mod testing;

pub use forward::{forward, Target};
pub use session::{Role, Session, StreamOpener, MAX_STREAMS};
pub use tokio_yamux::StreamHandle;
```

- [ ] **Step 5: Run the session tests**

Run: `cargo test -p fleetops-tunnel --test session`
Expected: compile error only for the missing `forward` and `testing` modules. Create empty `src/forward.rs` and `src/testing.rs` for now, then expect PASS (3 tests).

If `closing_the_session_ends_open_streams` hangs on the read, tokio-yamux is not ending streams when the session shuts down. The fix is to end the driver task itself. Give `Session` a `tokio::sync::Notify` (in an `Arc`), and have `close()` call `notify_waiters()` after `control.close()`. In the driver, loop with `tokio::select! { item = yamux.next() => …, _ = stop.notified() => break }`. Dropping the yamux `Session` drops the channels behind every `StreamHandle`, so their reads return `Ok(0)`. Only do this if the test fails.

- [ ] **Step 6: Write the failing forward tests**

Create `crates/fleetops-tunnel/tests/forward.rs`:

```rust
use fleetops_tunnel::{testing::loopback, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn targets_parse_from_docker_urls() {
    assert_eq!(
        Target::parse("unix:///var/run/docker.sock").unwrap(),
        Target::Unix("/var/run/docker.sock".into())
    );
    assert_eq!(
        Target::parse("tcp://10.0.0.5:2375").unwrap(),
        Target::Tcp("10.0.0.5:2375".into())
    );
    assert_eq!(
        Target::parse("http://localhost:2375").unwrap(),
        Target::Tcp("localhost:2375".into())
    );
    assert!(Target::parse("ssh://host").is_err());
    assert!(Target::parse("unix://").is_err());
}

#[tokio::test]
async fn streams_are_piped_to_a_unix_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("echo.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = c.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });

    let server = loopback(Target::Unix(path));
    let mut s = server.open_stream().await.unwrap();
    s.write_all(b"through the tunnel").await.unwrap();
    let mut buf = [0u8; 18];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"through the tunnel");
}

#[tokio::test]
async fn an_unreachable_target_resets_the_stream() {
    let server = loopback(Target::Unix("/nonexistent/fleetops.sock".into()));
    let mut s = server.open_stream().await.unwrap();
    let _ = s.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(std::time::Duration::from_secs(1), s.read(&mut buf))
        .await
        .expect("hung");
    assert!(matches!(n, Ok(0) | Err(_)));
}
```

- [ ] **Step 7: Run to verify they fail**

Run: `cargo test -p fleetops-tunnel --test forward`
Expected: FAIL to compile (`no function or associated item named parse`).

- [ ] **Step 8: Implement forward and loopback**

`crates/fleetops-tunnel/src/forward.rs`:

```rust
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
        let bad = || io::Error::new(io::ErrorKind::InvalidInput, format!("unsupported Docker URL '{url}'"));
        if let Some(path) = url.strip_prefix("unix://") {
            return if path.is_empty() { Err(bad()) } else { Ok(Target::Unix(path.into())) };
        }
        for scheme in ["tcp://", "http://"] {
            if let Some(addr) = url.strip_prefix(scheme) {
                let addr = addr.trim_end_matches('/');
                return if addr.is_empty() { Err(bad()) } else { Ok(Target::Tcp(addr.into())) };
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
```

`crates/fleetops-tunnel/src/testing.rs`:

```rust
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
```

- [ ] **Step 9: Run every tunnel test and lint**

Run: `cargo test -p fleetops-tunnel && cargo clippy -p fleetops-tunnel --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS (6 tests), no warnings.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-tunnel
git commit -m "feat(tunnel): yamux sessions over any byte stream, and forwarding to a Docker endpoint"
```

---

### Task 2: bollard over the tunnel — the docker-it suite through a loopback

This task is the go/no-go check for the whole design (spec §12). If hijacked exec fails through the tunnel, stop and report before starting Task 3.

**Files:**
- Create: `crates/fleetops-docker/src/tunnel.rs`
- Modify: `crates/fleetops-docker/src/lib.rs`, `crates/fleetops-docker/Cargo.toml`, `Cargo.toml`, `crates/fleetops-docker/tests/docker_it.rs:26-42`, `Makefile` (`test-docker`)
- Test: `crates/fleetops-docker/src/tunnel.rs` (unit, no Docker), `crates/fleetops-docker/tests/docker_it.rs` (whole suite, second pass)

**Interfaces:**
- Consumes: `fleetops_tunnel::{StreamOpener, StreamHandle, Target, testing::loopback}` (Task 1)
- Produces: `impl LocalDocker { pub fn through(opener: Arc<dyn StreamOpener>) -> Result<LocalDocker> }`

- [ ] **Step 1: Add the dependencies**

Root `Cargo.toml` `[workspace.dependencies]`:

```toml
hyper = { version = "1", features = ["client", "http1"] }
hyper-util = { version = "0.1", features = ["client-legacy", "http1", "tokio"] }
tower-service = "0.3"
```

`crates/fleetops-docker/Cargo.toml` `[dependencies]`: add `fleetops-tunnel.workspace = true`, `hyper.workspace = true`, `hyper-util.workspace = true`, `tower-service.workspace = true`.

- [ ] **Step 2: Write the failing unit test**

Create `crates/fleetops-docker/src/tunnel.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use crate::LocalDocker;
    use fleetops_core::docker::SystemOps;
    use fleetops_tunnel::{testing::loopback, Target};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-endpoint Docker stand-in: answers GET /version on a unix socket.
    fn fake_daemon(path: std::path::PathBuf) {
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            loop {
                let (mut c, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = c.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    assert!(req.contains("/version"), "{req}");
                    let body = r#"{"Version":"27.0.0","ApiVersion":"1.47","Os":"linux","Arch":"arm64"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    c.write_all(resp.as_bytes()).await.unwrap();
                });
            }
        });
    }

    #[tokio::test]
    async fn bollard_requests_travel_through_the_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("docker.sock");
        fake_daemon(sock.clone());
        let docker = LocalDocker::through(Arc::new(loopback(Target::Unix(sock)))).unwrap();
        let v = docker.version().await.unwrap();
        assert_eq!(v.version, "27.0.0");
        assert_eq!(v.arch, "arm64");
    }

    #[tokio::test]
    async fn a_closed_tunnel_is_env_unavailable() {
        let session = loopback(Target::Unix("/nonexistent.sock".into()));
        session.close().await;
        let docker = LocalDocker::through(Arc::new(session)).unwrap();
        assert!(matches!(
            docker.version().await,
            Err(fleetops_core::Error::EnvUnavailable(_))
        ));
    }
}
```

Add `mod tunnel;` to `crates/fleetops-docker/src/lib.rs`.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p fleetops-docker --lib tunnel`
Expected: FAIL to compile (`no function or associated item named through found for LocalDocker`).

- [ ] **Step 4: Implement the transport**

Prepend the implementation to `crates/fleetops-docker/src/tunnel.rs`:

```rust
//! bollard over an agent tunnel: a hyper client whose connector opens a
//! yamux stream instead of a socket. Upgrades (exec, attach) work because
//! hyper owns the connection and the stream is a plain byte pipe.

use crate::LocalDocker;
use bollard::{BollardRequest, Docker, API_DEFAULT_VERSION};
use fleetops_core::{Error, Result};
use fleetops_tunnel::{StreamHandle, StreamOpener};
use hyper::Uri;
use hyper_util::{
    client::legacy::{
        connect::{Connected, Connection},
        Client,
    },
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

/// Seconds bollard waits for response headers (its own default).
const REQUEST_TIMEOUT_SECS: u64 = 120;
/// Idle pooled streams close after this (spec §2.4).
const POOL_IDLE: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct TunnelConnector(Arc<dyn StreamOpener>);

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

struct TunnelIo(TokioIo<StreamHandle>);

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
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl LocalDocker {
    /// A client for the Docker endpoint at the far end of an agent tunnel.
    pub fn through(opener: Arc<dyn StreamOpener>) -> Result<Self> {
        let client = Arc::new(
            Client::builder(TokioExecutor::new())
                .pool_idle_timeout(POOL_IDLE)
                .pool_timer(TokioTimer::new())
                .build(TunnelConnector(opener)),
        );
        let client = Docker::connect_with_custom_transport(
            move |req: BollardRequest| {
                let client = client.clone();
                async move { client.request(req).await.map_err(bollard::errors::Error::from) }
            },
            Some("http://agent"),
            REQUEST_TIMEOUT_SECS,
            API_DEFAULT_VERSION,
        )
        .map_err(|e| Error::EnvUnavailable(format!("agent transport: {e}")))?;
        Ok(Self { client })
    }
}
```

If `bollard::errors::Error::from` has no `From<hyper_util::client::legacy::Error>` impl in 0.21.1, map it explicitly: `.map_err(|e| bollard::errors::Error::IOError { err: io::Error::new(io::ErrorKind::Other, e) })`. Check the `IOError` variant name in `bollard-0.21.1/src/errors.rs`. `map_err` in `error.rs` already turns any non-server bollard error into `EnvUnavailable`.

Add `tempfile` to the docker crate's `[dev-dependencies]` if it's missing (it's already there).

- [ ] **Step 5: Run the unit tests**

Run: `cargo test -p fleetops-docker --lib tunnel`
Expected: PASS (2 tests).

- [ ] **Step 6: Route the docker-it suite through a loopback tunnel on request**

In `crates/fleetops-docker/tests/docker_it.rs`, replace the last line of `docker()` (`LocalDocker::connect(&host()).unwrap()`) with `connect()`, and add:

```rust
/// With FLEETOPS_IT_TUNNEL=1 every test talks to Docker through an
/// in-memory agent tunnel instead of the socket (sub-project 2, spec §9).
fn connect() -> LocalDocker {
    if std::env::var_os("FLEETOPS_IT_TUNNEL").is_some() {
        let target = fleetops_tunnel::Target::parse(&host()).expect("DOCKER_HOST");
        let session = fleetops_tunnel::testing::loopback(target);
        LocalDocker::through(std::sync::Arc::new(session)).unwrap()
    } else {
        LocalDocker::connect(&host()).unwrap()
    }
}
```

Add `fleetops-tunnel.workspace = true` to the docker crate's `[dev-dependencies]` (it's already a normal dependency, so this is only needed if clippy complains; otherwise skip).

In the `Makefile`, change the `test-docker` recipe so the docker suite also runs a second time through the tunnel:

```make
test-docker:
	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && \
		FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-docker --features docker-it --test docker_it && \
		cargo test -p fleetops-server --features docker-it --test live'
```

- [ ] **Step 7: Run the whole suite through the tunnel**

Run: `DOCKER_HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}') FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-docker --features docker-it --test docker_it`
Expected: PASS for every test, including `streams::exec_*`, `streams::build_*` and the archive download tests. `daemon_loss_mid_session_is_unavailable_then_recovers` and `environment_status_is_up` build local environments and pass unchanged.

If the exec tests fail, run one alone with `RUST_LOG=trace` and check whether the response is `101 Switching Protocols`. If hyper never yields the upgrade, the legacy client is missing `http1` upgrade support: check that `hyper-util`'s `client-legacy` + `http1` features are on, and call `.http1_preserve_header_case(true)` only if Docker rejects the request. Report and stop if it still fails.

- [ ] **Step 8: Lint and commit**

Run: `make lint && make test`
Expected: PASS.

```bash
git add Cargo.toml Cargo.lock Makefile crates/fleetops-docker
git commit -m "feat(docker): bollard over an agent tunnel; docker-it runs a second time through it"
```

---

### Task 3: WebSocket transport and the control protocol

**Files:**
- Create: `crates/fleetops-tunnel/src/ws.rs`, `crates/fleetops-tunnel/src/control.rs`
- Modify: `crates/fleetops-tunnel/src/lib.rs`, `crates/fleetops-tunnel/Cargo.toml`
- Test: `crates/fleetops-tunnel/tests/ws.rs`, `crates/fleetops-tunnel/tests/control.rs`

**Interfaces:**
- Produces:
  - `pub const PROTO_MAJOR: u16 = 1;`
  - `pub const MAX_WS_MESSAGE: usize = 16 << 20;`
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)] pub struct HostInfo { pub hostname: String, pub os: String, pub arch: String, pub docker: String }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum ByeReason { Revoked, Replaced, Incompatible, Shutdown }`
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(tag = "type", rename_all = "snake_case")] pub enum Msg { Hello { proto: u16, agent_version: String, host: HostInfo }, Welcome { env_name: String, server_version: String, agent_key: Option<String> }, Ping { n: u64 }, Pong { n: u64 }, Bye { reason: ByeReason } }`
  - `pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> io::Result<()>`
  - `pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Msg>>` (returns `None` on a clean EOF)
  - `pub struct WsIo<S>` with `pub fn new(inner: S) -> Self`, implementing `AsyncRead` when `S: Stream<Item = io::Result<Bytes>> + Unpin` and `AsyncWrite` when `S: Sink<Bytes, Error = io::Error> + Unpin`
  - `pub fn tungstenite_io<T>(ws: WebSocketStream<T>) -> impl AsyncRead + AsyncWrite + Send + Unpin + 'static` where `T: AsyncRead + AsyncWrite + Send + Unpin + 'static`

- [ ] **Step 1: Write the failing control tests**

`crates/fleetops-tunnel/tests/control.rs`:

```rust
use fleetops_tunnel::control::{read_msg, write_msg, ByeReason, HostInfo, Msg, PROTO_MAJOR};
use tokio::io::{duplex, AsyncWriteExt};

#[tokio::test]
async fn messages_round_trip_in_order() {
    let (mut a, mut b) = duplex(4096);
    let sent = vec![
        Msg::Hello { proto: PROTO_MAJOR, agent_version: "0.2.0".into(), host: HostInfo { hostname: "web-01".into(), ..Default::default() } },
        Msg::Ping { n: 7 },
        Msg::Bye { reason: ByeReason::Shutdown },
    ];
    for m in &sent {
        write_msg(&mut a, m).await.unwrap();
    }
    drop(a);
    for m in &sent {
        assert_eq!(&read_msg(&mut b).await.unwrap().unwrap(), m);
    }
    assert_eq!(read_msg(&mut b).await.unwrap(), None, "clean EOF");
}

#[tokio::test]
async fn unknown_fields_are_ignored_within_a_major() {
    let (mut a, mut b) = duplex(4096);
    let body = br#"{"type":"pong","n":3,"added_in_1_1":true}"#;
    a.write_all(&(body.len() as u32).to_be_bytes()).await.unwrap();
    a.write_all(body).await.unwrap();
    assert_eq!(read_msg(&mut b).await.unwrap(), Some(Msg::Pong { n: 3 }));
}

#[tokio::test]
async fn oversized_frames_are_refused() {
    let (mut a, mut b) = duplex(4096);
    a.write_all(&(1u32 << 20).to_be_bytes()).await.unwrap();
    assert!(read_msg(&mut b).await.is_err());
}

#[test]
fn wire_names_are_snake_case() {
    let v = serde_json::to_value(Msg::Bye { reason: ByeReason::Incompatible }).unwrap();
    assert_eq!(v, serde_json::json!({"type": "bye", "reason": "incompatible"}));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleetops-tunnel --test control`
Expected: FAIL to compile (`could not find control`).

- [ ] **Step 3: Implement control**

`crates/fleetops-tunnel/src/control.rs`:

```rust
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
    Hello { proto: u16, agent_version: String, host: HostInfo },
    /// `agent_key` is set once, on the connection that used a join token.
    Welcome { env_name: String, server_version: String, agent_key: Option<String> },
    Ping { n: u64 },
    Pong { n: u64 },
    Bye { reason: ByeReason },
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
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("control message of {len} bytes")));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

Add `pub mod control;` and `pub use control::PROTO_MAJOR;` to `lib.rs`. Add `serde_json` to the tunnel's `[dev-dependencies]` too (it's already a normal dependency, so the tests can use it as is).

- [ ] **Step 4: Run the control tests**

Run: `cargo test -p fleetops-tunnel --test control`
Expected: PASS (4 tests).

- [ ] **Step 5: Write the failing WebSocket test**

Add `tokio-tungstenite.workspace = true` to the tunnel's `[dependencies]`. Create `crates/fleetops-tunnel/tests/ws.rs`:

```rust
use fleetops_tunnel::{ws::tungstenite_io, Role, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A yamux session carried over a real WebSocket on loopback TCP, the
/// agent (client) echoing every stream the server opens.
#[tokio::test]
async fn a_yamux_session_runs_over_a_websocket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        Session::spawn(tungstenite_io(ws), Role::Server, |_s| {})
    });
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/")).await.unwrap();
    let _agent = Session::spawn(tungstenite_io(ws), Role::Client, |s| {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(s);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    });
    let server = server.await.unwrap();

    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut s = server.open_stream().await.unwrap();
    let (mut r, mut w) = tokio::io::split(&mut s);
    let send = async {
        w.write_all(&payload).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let recv = r.read_to_end(&mut back);
    let (_, n) = tokio::join!(send, recv);
    n.unwrap();
    assert_eq!(back, payload);
}
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p fleetops-tunnel --test ws`
Expected: FAIL to compile (`could not find ws`).

- [ ] **Step 7: Implement WsIo**

`crates/fleetops-tunnel/src/ws.rs`:

```rust
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
        Self { inner, pending: Bytes::new() }
    }
}

impl<S> AsyncRead for WsIo<S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
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
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        ready!(self.inner.poll_ready_unpin(cx))?;
        let n = buf.len().min(MAX_WRITE);
        self.inner.start_send_unpin(Bytes::copy_from_slice(&buf[..n]))?;
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
pub fn tungstenite_io<T>(ws: WebSocketStream<T>) -> impl AsyncRead + AsyncWrite + Send + Unpin + 'static
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
```

Add `pub mod ws;` to `lib.rs`.

If the compiler rejects the combinator chain because `TakeWhile`/`FilterMap` don't forward `Sink` in this futures version, switch to a hand-written adapter: a struct holding `WebSocketStream<T>` that implements `Stream<Item = io::Result<Bytes>>` (loop over `poll_next`, skip non-binary, end on `Close`) and `Sink<Bytes>` (wrap in `Message::Binary`). Same behaviour, about 40 lines.

- [ ] **Step 8: Run all tunnel tests**

Run: `cargo test -p fleetops-tunnel && cargo clippy -p fleetops-tunnel --all-targets -- -D warnings`
Expected: PASS (11 tests).

- [ ] **Step 9: Commit**

```bash
git add Cargo.lock crates/fleetops-tunnel
git commit -m "feat(tunnel): WebSocket byte transport and the length-prefixed control protocol"
```

---

## Phase B — Server side

### Task 4: `docker-agent` kind and agent credentials in the store

**Files:**
- Modify: `crates/fleetops-core/src/env.rs`
- Create: `crates/fleetops-store/migrations/0002_agents.sql`, `crates/fleetops-store/src/agents.rs`
- Modify: `crates/fleetops-store/src/lib.rs`
- Test: `crates/fleetops-core/src/env.rs` (unit), `crates/fleetops-store/tests/agents.rs`

**Interfaces:**
- Produces (core):
  - `EnvKind::DockerAgent` ⇄ `"docker-agent"`
  - `pub fn agent_url(env_id: i64) -> String` → `"agent://<id>"`
  - `Environment::wait_change(&self)`, an async trait method with a default body that never resolves
- Produces (store):
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)] pub struct AgentRecord { pub environment_id: i64, pub enrolled: bool, pub join_expires_at: Option<i64>, pub last_seen_at: Option<i64>, pub agent_version: Option<String>, pub host_info: Option<String>, pub last_error: Option<String> }`
  - `Store::create_agent_environment(&self, name: &str, join_hash: &str, join_expires_at: i64, now: i64) -> Result<EnvironmentRecord>`
  - `Store::agent_by_join_token(&self, join_hash: &str, now: i64) -> Result<Option<i64>>`
  - `Store::agent_by_key(&self, key_hash: &str) -> Result<Option<i64>>`
  - `Store::enroll_agent(&self, env_id: i64, join_hash: &str, key_hash: &str, now: i64) -> Result<bool>`, true only for the caller that consumed the token
  - `Store::rotate_agent(&self, env_id: i64, join_hash: &str, join_expires_at: i64) -> Result<()>`
  - `Store::record_agent_seen(&self, env_id: i64, now: i64, agent_version: Option<&str>, host_info: Option<&str>, last_error: Option<&str>) -> Result<()>`
  - `Store::get_agent(&self, env_id: i64) -> Result<Option<AgentRecord>>`
  - `Store::list_agents(&self) -> Result<Vec<AgentRecord>>`

- [ ] **Step 1: Write the failing core test**

Add to the test module of `crates/fleetops-core/src/env.rs` (create `#[cfg(test)] mod tests { use super::*; … }` if there is none):

```rust
#[test]
fn agent_kind_round_trips_and_has_a_server_made_url() {
    assert_eq!("docker-agent".parse::<EnvKind>().unwrap(), EnvKind::DockerAgent);
    assert_eq!(EnvKind::DockerAgent.as_str(), "docker-agent");
    assert_eq!(agent_url(42), "agent://42");
    // Users cannot type one in.
    assert!(validate_env_url("agent://42").is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p fleetops-core agent_kind`
Expected: FAIL to compile (`no variant DockerAgent`).

- [ ] **Step 3: Implement the core changes**

In `crates/fleetops-core/src/env.rs`:

```rust
pub enum EnvKind {
    DockerLocal,
    /// A remote Docker host reached through a `fleetops-agent` tunnel.
    DockerAgent,
}
```

Add `EnvKind::DockerAgent => "docker-agent",` to `as_str` and `"docker-agent" => Ok(EnvKind::DockerAgent),` to `from_str`. Add to the `Environment` trait, after `docker`:

```rust
    /// Resolves when this environment's reachability may have changed, so
    /// a waiting reconnect can retry at once instead of sleeping out its
    /// backoff. Environments with no such signal never resolve.
    async fn wait_change(&self) {
        std::future::pending::<()>().await
    }
```

And at the end of the file:

```rust
/// The URL of an agent environment: made by the server, never typed.
pub fn agent_url(env_id: i64) -> String {
    format!("agent://{env_id}")
}
```

Update the module doc comment's first line to: `//! Environments are the things fleetops manages: a local Docker socket, or a remote host behind an agent.`

- [ ] **Step 4: Run the core tests and fix exhaustive matches**

Run: `cargo test -p fleetops-core && cargo build --workspace`
Expected: core PASS. The workspace build fails in `crates/fleetops-docker/src/env.rs`, where `DockerEnvFactory::build`'s `match kind` doesn't cover `DockerAgent`. Add a temporary arm, `EnvKind::DockerAgent => Err(fleetops_core::Error::Invalid("agent environments need the agent registry".into())),`, which Task 5 replaces. Build again: PASS.

- [ ] **Step 5: Write the failing store tests**

`crates/fleetops-store/tests/agents.rs`:

```rust
use fleetops_store::Store;

const T0: i64 = 1_800_000_000;

#[tokio::test]
async fn an_agent_environment_is_created_with_a_pending_join_token() {
    let s = Store::in_memory().await.unwrap();
    let env = s.create_agent_environment("web-01", "join-hash", T0 + 900, T0).await.unwrap();
    assert_eq!(env.kind, "docker-agent");
    assert_eq!(env.url, format!("agent://{}", env.id));
    let a = s.get_agent(env.id).await.unwrap().unwrap();
    assert!(!a.enrolled);
    assert_eq!(a.join_expires_at, Some(T0 + 900));
}

#[tokio::test]
async fn join_tokens_expire_and_are_single_use() {
    let s = Store::in_memory().await.unwrap();
    let env = s.create_agent_environment("web-01", "j1", T0 + 900, T0).await.unwrap();
    assert_eq!(s.agent_by_join_token("j1", T0 + 899).await.unwrap(), Some(env.id));
    assert_eq!(s.agent_by_join_token("j1", T0 + 900).await.unwrap(), None, "expired");

    assert!(s.enroll_agent(env.id, "j1", "k1", T0 + 10).await.unwrap());
    assert!(!s.enroll_agent(env.id, "j1", "k2", T0 + 11).await.unwrap(), "second use");
    assert_eq!(s.agent_by_join_token("j1", T0 + 12).await.unwrap(), None);
    assert_eq!(s.agent_by_key("k1").await.unwrap(), Some(env.id));
    assert_eq!(s.agent_by_key("k2").await.unwrap(), None);
    assert!(s.get_agent(env.id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn rotating_revokes_the_key_and_issues_a_new_join_token() {
    let s = Store::in_memory().await.unwrap();
    let env = s.create_agent_environment("web-01", "j1", T0 + 900, T0).await.unwrap();
    s.enroll_agent(env.id, "j1", "k1", T0).await.unwrap();
    s.rotate_agent(env.id, "j2", T0 + 1800).await.unwrap();
    assert_eq!(s.agent_by_key("k1").await.unwrap(), None);
    assert_eq!(s.agent_by_join_token("j2", T0 + 1000).await.unwrap(), Some(env.id));
    assert!(!s.get_agent(env.id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn seen_records_version_host_and_errors_and_delete_cascades() {
    let s = Store::in_memory().await.unwrap();
    let env = s.create_agent_environment("web-01", "j1", T0 + 900, T0).await.unwrap();
    s.record_agent_seen(env.id, T0 + 5, Some("0.2.0"), Some(r#"{"hostname":"web-01"}"#), None).await.unwrap();
    s.record_agent_seen(env.id, T0 + 6, None, None, Some("protocol 2; server needs 1")).await.unwrap();
    let a = s.get_agent(env.id).await.unwrap().unwrap();
    assert_eq!(a.last_seen_at, Some(T0 + 6));
    assert_eq!(a.agent_version.as_deref(), Some("0.2.0"), "None keeps the previous value");
    assert_eq!(a.last_error.as_deref(), Some("protocol 2; server needs 1"));
    assert_eq!(s.list_agents().await.unwrap().len(), 1);

    s.delete_environment(env.id).await.unwrap();
    assert!(s.get_agent(env.id).await.unwrap().is_none());
}

#[tokio::test]
async fn agent_names_share_the_environment_namespace() {
    let s = Store::in_memory().await.unwrap();
    s.create_environment("web-01", "docker-local", "unix:///x.sock", T0).await.unwrap();
    assert!(matches!(
        s.create_agent_environment("web-01", "j", T0 + 900, T0).await,
        Err(fleetops_core::Error::Conflict(_))
    ));
}
```

Check that `fleetops-core` is in the store's `[dev-dependencies]` (it's a normal dependency, so it is usable).

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -p fleetops-store --test agents`
Expected: FAIL to compile (`no method named create_agent_environment`).

- [ ] **Step 7: Write the migration and the queries**

`crates/fleetops-store/migrations/0002_agents.sql`:

```sql
-- Sub-project 2: credentials and last-known state of agent environments.
-- Hashes are hex SHA-256 of the high-entropy tokens; the tokens themselves
-- are never stored.
CREATE TABLE agent_credentials (
    environment_id  INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    key_hash        TEXT UNIQUE,
    join_token_hash TEXT UNIQUE,
    join_expires_at INTEGER,
    last_seen_at    INTEGER,
    agent_version   TEXT,
    host_info       TEXT,
    last_error      TEXT
);
```

`crates/fleetops-store/src/agents.rs`:

```rust
//! Agent environments: an `environments` row of kind `docker-agent` plus
//! one `agent_credentials` row (spec §3).

use crate::{db_err, EnvironmentRecord, Store};
use fleetops_core::{Error, Result};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct AgentRecord {
    pub environment_id: i64,
    /// An agent key is set: the agent has enrolled and not been rotated since.
    pub enrolled: bool,
    pub join_expires_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub agent_version: Option<String>,
    /// JSON from the agent's hello.
    pub host_info: Option<String>,
    /// Why the last connection was refused, when it was.
    pub last_error: Option<String>,
}

const AGENT_COLS: &str = "environment_id, key_hash IS NOT NULL AS enrolled, join_expires_at, \
                          last_seen_at, agent_version, host_info, last_error";

impl Store {
    pub async fn create_agent_environment(
        &self,
        name: &str,
        join_hash: &str,
        join_expires_at: i64,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO environments (name, kind, url, created_at, updated_at)
             VALUES (?, 'docker-agent', '', ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match db_err(e) {
            Error::Conflict(_) => Error::Conflict(format!("environment '{name}' already exists")),
            other => other,
        })?;
        sqlx::query("UPDATE environments SET url = ? WHERE id = ?")
            .bind(fleetops_core::env::agent_url(id))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO agent_credentials (environment_id, join_token_hash, join_expires_at)
             VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(join_hash)
        .bind(join_expires_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.get_environment(id).await
    }

    pub async fn agent_by_join_token(&self, join_hash: &str, now: i64) -> Result<Option<i64>> {
        sqlx::query_scalar(
            "SELECT environment_id FROM agent_credentials
             WHERE join_token_hash = ? AND join_expires_at > ?",
        )
        .bind(join_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn agent_by_key(&self, key_hash: &str) -> Result<Option<i64>> {
        sqlx::query_scalar("SELECT environment_id FROM agent_credentials WHERE key_hash = ?")
            .bind(key_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    /// Swaps a live join token for a key in one statement, so of two
    /// agents racing on the same token exactly one wins.
    pub async fn enroll_agent(&self, env_id: i64, join_hash: &str, key_hash: &str, now: i64) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE agent_credentials
             SET key_hash = ?, join_token_hash = NULL, join_expires_at = NULL, last_error = NULL
             WHERE environment_id = ? AND join_token_hash = ? AND join_expires_at > ?",
        )
        .bind(key_hash)
        .bind(env_id)
        .bind(join_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn rotate_agent(&self, env_id: i64, join_hash: &str, join_expires_at: i64) -> Result<()> {
        let done = sqlx::query(
            "UPDATE agent_credentials
             SET key_hash = NULL, join_token_hash = ?, join_expires_at = ?
             WHERE environment_id = ?",
        )
        .bind(join_hash)
        .bind(join_expires_at)
        .bind(env_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound(format!("agent environment {env_id}")));
        }
        Ok(())
    }

    /// `None` for version or host keeps the stored value; `last_error` is
    /// always written (None clears it).
    pub async fn record_agent_seen(
        &self,
        env_id: i64,
        now: i64,
        agent_version: Option<&str>,
        host_info: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE agent_credentials
             SET last_seen_at = ?, agent_version = COALESCE(?, agent_version),
                 host_info = COALESCE(?, host_info), last_error = ?
             WHERE environment_id = ?",
        )
        .bind(now)
        .bind(agent_version)
        .bind(host_info)
        .bind(last_error)
        .bind(env_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn get_agent(&self, env_id: i64) -> Result<Option<AgentRecord>> {
        sqlx::query_as(&format!("SELECT {AGENT_COLS} FROM agent_credentials WHERE environment_id = ?"))
            .bind(env_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        sqlx::query_as(&format!("SELECT {AGENT_COLS} FROM agent_credentials ORDER BY environment_id"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }
}
```

In `crates/fleetops-store/src/lib.rs`, add `mod agents;` and `pub use agents::AgentRecord;`. If `db_err` or `pool` aren't visible to sibling modules, they already are for `environments.rs` (it uses `crate::db_err` and `self.pool`). The store depends on `utoipa` already, since `EnvironmentRecord` derives `ToSchema`.

- [ ] **Step 8: Run the store tests**

Run: `cargo test -p fleetops-store`
Expected: PASS, including the 5 new agent tests.

- [ ] **Step 9: Lint and commit**

Run: `make lint && make test`

```bash
git add crates/fleetops-core crates/fleetops-store crates/fleetops-docker/src/env.rs
git commit -m "feat(store): docker-agent environments with join tokens, keys and last-seen state"
```

---

### Task 5: the session registry, `AgentDockerEnv`, and reconnects that wake on change

**Files:**
- Create: `crates/fleetops-tunnel/src/registry.rs`, `crates/fleetops-docker/src/agent_env.rs`
- Modify: `crates/fleetops-tunnel/src/lib.rs`, `crates/fleetops-docker/src/env.rs`, `crates/fleetops-docker/src/lib.rs`, `crates/fleetops-docker/tests/docker_it.rs` (`DockerEnvFactory` → `DockerEnvFactory::default()`), `crates/fleetops-server/src/lib.rs:22`, `crates/fleetops-server/tests/static_files.rs:58`, `crates/fleetops-server/tests/live.rs:59`, `crates/fleetops-api/src/ws/hub.rs` (`run_events`), `crates/fleetops-api/tests/common/fake_docker.rs` (`FakeEnv::wait_change`)
- Test: `crates/fleetops-tunnel/tests/registry.rs`, `crates/fleetops-docker/src/agent_env.rs` (unit), `crates/fleetops-api/tests/ws_events.rs` (new test)

**Interfaces:**
- Consumes: `Session`, `ByeReason`, `HostInfo` (Tasks 1, 3); `LocalDocker::through` (Task 2); `Environment::wait_change`, `EnvKind::DockerAgent` (Task 4)
- Produces:
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct AgentInfo { pub agent_version: String, pub host: HostInfo, pub connected_at: i64 }`
  - `#[derive(Clone)] pub struct Live { pub id: u64, pub session: Session, pub info: AgentInfo }`
  - `#[derive(Default)] pub struct Registry` with:
    - `pub fn insert(&self, env_id: i64, session: Session, info: AgentInfo, bye: mpsc::Sender<ByeReason>) -> u64` (replaces and says `Replaced` to any previous session)
    - `pub fn remove(&self, env_id: i64, id: u64, reason: &str) -> bool` (only removes if `id` still matches; records the offline reason)
    - `pub fn revoke(&self, env_id: i64)` (says `Revoked` to the live session, if any)
    - `pub fn set_offline_reason(&self, env_id: i64, reason: &str)`
    - `pub fn get(&self, env_id: i64) -> Option<Live>`
    - `pub fn offline_reason(&self, env_id: i64) -> Option<String>`
    - `pub fn connected(&self) -> usize`
    - `pub async fn wait_change(&self)`
  - `pub struct AgentDockerEnv` with `pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self`, implementing `Environment`
  - `pub struct DockerEnvFactory { agents: Arc<Registry> }` with `pub fn new(agents: Arc<Registry>) -> Self` and `impl Default`

- [ ] **Step 1: Write the failing registry tests**

`crates/fleetops-tunnel/tests/registry.rs`:

```rust
use fleetops_tunnel::{
    control::{ByeReason, HostInfo},
    registry::{AgentInfo, Registry},
    testing::loopback,
    Target,
};
use std::time::Duration;
use tokio::sync::mpsc;

fn info(v: &str) -> AgentInfo {
    AgentInfo { agent_version: v.into(), host: HostInfo::default(), connected_at: 0 }
}

fn session() -> fleetops_tunnel::Session {
    loopback(Target::Unix("/nonexistent.sock".into()))
}

#[tokio::test]
async fn a_newer_session_replaces_the_older_one() {
    let r = Registry::default();
    let (bye1, mut rx1) = mpsc::channel(1);
    let first = r.insert(7, session(), info("0.2.0"), bye1);
    let (bye2, _rx2) = mpsc::channel(1);
    let second = r.insert(7, session(), info("0.2.1"), bye2);
    assert_ne!(first, second);
    assert_eq!(rx1.recv().await, Some(ByeReason::Replaced));
    assert_eq!(r.get(7).unwrap().info.agent_version, "0.2.1");

    // The replaced session's cleanup must not remove its successor.
    assert!(!r.remove(7, first, "dropped"));
    assert!(r.get(7).is_some());
    assert!(r.remove(7, second, "connection lost"));
    assert!(r.get(7).is_none());
    assert_eq!(r.offline_reason(7).as_deref(), Some("connection lost"));
    assert_eq!(r.connected(), 0);
}

#[tokio::test]
async fn revoke_says_bye_to_the_live_session() {
    let r = Registry::default();
    let (bye, mut rx) = mpsc::channel(1);
    r.insert(3, session(), info("0.2.0"), bye);
    r.revoke(3);
    assert_eq!(rx.recv().await, Some(ByeReason::Revoked));
    r.revoke(99); // no session: nothing happens
}

#[tokio::test]
async fn waiters_wake_on_every_change() {
    let r = std::sync::Arc::new(Registry::default());
    let waiter = {
        let r = r.clone();
        tokio::spawn(async move { r.wait_change().await })
    };
    tokio::task::yield_now().await;
    let (bye, _rx) = mpsc::channel(1);
    r.insert(1, session(), info("0.2.0"), bye);
    tokio::time::timeout(Duration::from_secs(1), waiter).await.expect("no wake").unwrap();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleetops-tunnel --test registry`
Expected: FAIL to compile (`could not find registry`).

- [ ] **Step 3: Implement the registry**

`crates/fleetops-tunnel/src/registry.rs`:

```rust
//! Live agent sessions by environment id. The connect handler adds and
//! removes them; agent environments read them on every request.

use crate::{
    control::{ByeReason, HostInfo},
    Session,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub agent_version: String,
    pub host: HostInfo,
    /// Unix seconds.
    pub connected_at: i64,
}

#[derive(Clone)]
pub struct Live {
    /// Distinguishes successive sessions of one environment.
    pub id: u64,
    pub session: Session,
    pub info: AgentInfo,
}

struct Entry {
    live: Live,
    bye: mpsc::Sender<ByeReason>,
}

pub struct Registry {
    live: Mutex<HashMap<i64, Entry>>,
    offline: Mutex<HashMap<i64, String>>,
    next_id: AtomicU64,
    changes: watch::Sender<u64>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            live: Mutex::default(),
            offline: Mutex::default(),
            next_id: AtomicU64::new(1),
            changes: watch::channel(0).0,
        }
    }
}

impl Registry {
    fn changed(&self) {
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Makes `session` the environment's live session. A previous one is
    /// told `Replaced`; its own cleanup then finds a different id and
    /// leaves this one alone.
    pub fn insert(&self, env_id: i64, session: Session, info: AgentInfo, bye: mpsc::Sender<ByeReason>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let old = self
            .live
            .lock()
            .unwrap()
            .insert(env_id, Entry { live: Live { id, session, info }, bye });
        if let Some(old) = old {
            let _ = old.bye.try_send(ByeReason::Replaced);
        }
        self.offline.lock().unwrap().remove(&env_id);
        self.changed();
        id
    }

    /// Removes session `id` if it is still the live one, remembering why.
    pub fn remove(&self, env_id: i64, id: u64, reason: &str) -> bool {
        let mut live = self.live.lock().unwrap();
        if live.get(&env_id).is_some_and(|e| e.live.id == id) {
            live.remove(&env_id);
            drop(live);
            self.set_offline_reason(env_id, reason);
            return true;
        }
        false
    }

    pub fn revoke(&self, env_id: i64) {
        if let Some(e) = self.live.lock().unwrap().get(&env_id) {
            let _ = e.bye.try_send(ByeReason::Revoked);
        }
    }

    pub fn set_offline_reason(&self, env_id: i64, reason: &str) {
        self.offline.lock().unwrap().insert(env_id, reason.to_string());
        self.changed();
    }

    pub fn get(&self, env_id: i64) -> Option<Live> {
        self.live.lock().unwrap().get(&env_id).map(|e| e.live.clone())
    }

    pub fn offline_reason(&self, env_id: i64) -> Option<String> {
        self.offline.lock().unwrap().get(&env_id).cloned()
    }

    pub fn connected(&self) -> usize {
        self.live.lock().unwrap().len()
    }

    /// Resolves at the next insert, remove or reason change, for any environment.
    pub async fn wait_change(&self) {
        let mut rx = self.changes.subscribe();
        let _ = rx.changed().await;
    }
}
```

Add `pub mod registry;` to `crates/fleetops-tunnel/src/lib.rs`.

- [ ] **Step 4: Run the registry tests**

Run: `cargo test -p fleetops-tunnel --test registry`
Expected: PASS (3 tests).

- [ ] **Step 5: Write the failing AgentDockerEnv tests**

Create `crates/fleetops-docker/src/agent_env.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::env::{EnvStatus, Environment};
    use fleetops_tunnel::{control::HostInfo, registry::AgentInfo, testing::loopback, Target};
    use tokio::sync::mpsc;

    fn info() -> AgentInfo {
        AgentInfo { agent_version: "0.2.0".into(), host: HostInfo::default(), connected_at: 0 }
    }

    #[tokio::test]
    async fn without_a_session_it_is_down_and_has_no_docker() {
        let agents = Arc::new(Registry::default());
        let env = AgentDockerEnv::new(5, "web-01", agents.clone());
        assert!(env.docker().is_none());
        match env.status().await {
            EnvStatus::Down { reason } => assert_eq!(reason, "waiting for the agent to connect"),
            other => panic!("{other:?}"),
        }
        agents.set_offline_reason(5, "agent v0.9 uses protocol 2; server needs 1");
        match env.status().await {
            EnvStatus::Down { reason } => assert!(reason.contains("protocol 2"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_live_session_gives_a_client_rebuilt_per_session() {
        let agents = Arc::new(Registry::default());
        let env = AgentDockerEnv::new(5, "web-01", agents.clone());
        let (bye, _rx) = mpsc::channel(1);
        agents.insert(5, loopback(Target::Unix("/nonexistent.sock".into())), info(), bye);
        let a = env.client().expect("live");
        let again = env.client().expect("live");
        assert!(Arc::ptr_eq(&a, &again), "same session, same client");
        let (bye, _rx) = mpsc::channel(1);
        agents.insert(5, loopback(Target::Unix("/nonexistent.sock".into())), info(), bye);
        let b = env.client().expect("live");
        assert!(!Arc::ptr_eq(&a, &b), "a new session gets a new client");
        // Reachable tunnel, unreachable daemon: down with the daemon's error.
        assert!(matches!(env.status().await, EnvStatus::Down { .. }));
    }

    #[tokio::test]
    async fn wait_change_follows_the_registry() {
        let agents = Arc::new(Registry::default());
        let env = Arc::new(AgentDockerEnv::new(5, "web-01", agents.clone()));
        let w = {
            let env = env.clone();
            tokio::spawn(async move { env.wait_change().await })
        };
        tokio::task::yield_now().await;
        agents.set_offline_reason(5, "x");
        tokio::time::timeout(std::time::Duration::from_secs(1), w).await.unwrap().unwrap();
    }
}
```

- [ ] **Step 6: Run to verify it fails**

Add `mod agent_env;` and `pub use agent_env::AgentDockerEnv;` to `crates/fleetops-docker/src/lib.rs`.
Run: `cargo test -p fleetops-docker --lib agent_env`
Expected: FAIL to compile (`cannot find type AgentDockerEnv`).

- [ ] **Step 7: Implement AgentDockerEnv and the factory change**

Prepend to `crates/fleetops-docker/src/agent_env.rs`:

```rust
//! A remote Docker host behind an agent tunnel. Every request goes through
//! the environment's live session in the registry; with none, the
//! environment is down and `docker()` is `None` (spec §2.4).

use crate::LocalDocker;
use async_trait::async_trait;
use fleetops_core::docker::{DockerOps, SystemOps};
use fleetops_core::env::{EnvKind, EnvStatus, Environment};
use fleetops_tunnel::registry::Registry;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

pub struct AgentDockerEnv {
    id: i64,
    name: String,
    agents: Arc<Registry>,
    /// The client for the current session, keyed by session id.
    client: Mutex<Option<(u64, Arc<LocalDocker>)>>,
}

impl AgentDockerEnv {
    pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self {
        Self { id, name: name.to_string(), agents, client: Mutex::new(None) }
    }

    pub(crate) fn client(&self) -> Option<Arc<LocalDocker>> {
        let live = self.agents.get(self.id)?;
        let mut slot = self.client.lock().unwrap();
        if let Some((id, c)) = slot.as_ref() {
            if *id == live.id {
                return Some(c.clone());
            }
        }
        let c = Arc::new(LocalDocker::through(Arc::new(live.session)).ok()?);
        *slot = Some((live.id, c.clone()));
        Some(c)
    }
}

#[async_trait]
impl Environment for AgentDockerEnv {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> EnvKind {
        EnvKind::DockerAgent
    }

    async fn status(&self) -> EnvStatus {
        let Some(client) = self.client() else {
            let reason = self
                .agents
                .offline_reason(self.id)
                .map(|r| format!("agent offline: {r}"))
                .unwrap_or_else(|| "waiting for the agent to connect".into());
            return EnvStatus::Down { reason };
        };
        match tokio::time::timeout(STATUS_TIMEOUT, client.version()).await {
            Ok(Ok(version)) => EnvStatus::Up { version },
            Ok(Err(e)) => EnvStatus::Down { reason: format!("agent connected, Docker unreachable: {e}") },
            Err(_) => EnvStatus::Down { reason: format!("no response through the agent within {STATUS_TIMEOUT:?}") },
        }
    }

    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        self.client().map(|c| c as Arc<dyn DockerOps>)
    }

    async fn wait_change(&self) {
        self.agents.wait_change().await
    }
}
```

The first test expects the exact reason `"waiting for the agent to connect"`, and a reason set through `set_offline_reason` to come back prefixed with `"agent offline: "`. `contains("protocol 2")` still holds with the prefix.

In `crates/fleetops-docker/src/env.rs`, replace the factory:

```rust
/// Builds local and agent environments. Agent environments read their
/// live sessions from `agents`, which the server shares with the connect
/// handler.
#[derive(Default)]
pub struct DockerEnvFactory {
    agents: Arc<fleetops_tunnel::registry::Registry>,
}

impl DockerEnvFactory {
    pub fn new(agents: Arc<fleetops_tunnel::registry::Registry>) -> Self {
        Self { agents }
    }
}

impl EnvFactory for DockerEnvFactory {
    fn build(&self, id: i64, name: &str, kind: EnvKind, url: &str) -> Result<Arc<dyn Environment>> {
        match kind {
            EnvKind::DockerLocal => Ok(Arc::new(LocalDockerEnv::new(id, name, url))),
            EnvKind::DockerAgent => Ok(Arc::new(crate::AgentDockerEnv::new(id, name, self.agents.clone()))),
        }
    }
}
```

Replace every `DockerEnvFactory` value expression with `DockerEnvFactory::default()`: `crates/fleetops-docker/src/env.rs` (its tests), `crates/fleetops-docker/tests/docker_it.rs` (2 places), `crates/fleetops-server/tests/static_files.rs:58`, `crates/fleetops-server/tests/live.rs:59`. Leave `crates/fleetops-server/src/lib.rs` for Task 8, but make it compile now with `Arc::new(DockerEnvFactory::default())`.

- [ ] **Step 8: Run the docker crate tests**

Run: `cargo test -p fleetops-docker`
Expected: PASS, including the 3 new `agent_env` tests.

- [ ] **Step 9: Write the failing hub test**

In `crates/fleetops-api/tests/common/fake_docker.rs`, add a field `pub changed: Notify` to `FakeDocker` (initialise it in `FakeDocker::new` with `Notify::new()`), and to `impl Environment for FakeEnv`:

```rust
    async fn wait_change(&self) {
        self.docker.changed.notified().await
    }
```

Append to `crates/fleetops-api/tests/ws_events.rs`:

```rust
#[tokio::test]
async fn a_reachability_change_cuts_the_reconnect_backoff_short() {
    let mut config = ws_config();
    config.ws_backoff = std::time::Duration::from_secs(20);
    let t = app_with(config).await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, Value::Null)).await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    t.fake.reachable.store(false, Ordering::SeqCst);
    t.fake.end_event_streams();
    let down = ws.until(|v| v["type"] == "env_status").await;
    assert_eq!(down["status"]["state"], "down");

    // Back, and the environment says so: no 20 s wait.
    t.fake.reachable.store(true, Ordering::SeqCst);
    t.fake.changed.notify_waiters();
    let up = tokio::time::timeout(std::time::Duration::from_secs(2), ws.until(|v| v["type"] == "env_status"))
        .await
        .expect("the backoff was not cut short");
    assert_eq!(up["status"]["state"], "up");
}
```

(`ws_config`, `Ws`, `ws_sub`, `eventually` and `Ordering` are already imported by that file; check the imports at the top and add anything missing.)

- [ ] **Step 10: Run to verify it fails**

Run: `cargo test -p fleetops-api --test ws_events a_reachability_change`
Expected: FAIL with "the backoff was not cut short".

- [ ] **Step 11: Make the backoff sleeps wake on change**

In `crates/fleetops-api/src/ws/hub.rs`, add a helper above `run_events`:

```rust
/// Sleeps out a reconnect backoff, or less if the environment says its
/// reachability changed (an agent reconnected).
async fn backoff_sleep(env: &Arc<dyn Environment>, d: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = env.wait_change() => {}
    }
}
```

and replace both `tokio::time::sleep(backoff).await;` lines inside `run_events` with `backoff_sleep(&env, backoff).await;`.

- [ ] **Step 12: Run the API tests**

Run: `cargo test -p fleetops-api`
Expected: PASS.

- [ ] **Step 13: Lint and commit**

Run: `make lint && make test`

```bash
git add crates
git commit -m "feat: agent session registry, agent environments, and reconnects that wake on change"
```

---

### Task 6: enrollment API — create, list, rotate, delete

**Files:**
- Create: `crates/fleetops-api/src/agents/mod.rs`
- Modify: `crates/fleetops-api/src/lib.rs` (add `pub mod agents;`), `crates/fleetops-api/src/state.rs`, `crates/fleetops-api/src/routes/envs.rs`, `crates/fleetops-api/src/routes/mod.rs`, `crates/fleetops-api/src/openapi.rs` (register the new schemas if `openapi.rs` lists components by hand), `crates/fleetops-api/Cargo.toml` (`fleetops-tunnel.workspace = true`)
- Test: `crates/fleetops-api/tests/agents_enroll.rs`

**Interfaces:**
- Consumes: store agent queries (Task 4), `Registry` (Task 5), `auth::token::{generate_token, hash_token}`
- Produces:
  - `AppState.agents: Arc<Registry>` and `pub fn with_agents(self, agents: Arc<Registry>) -> Self`
  - `pub const JOIN_TTL_SECS: i64 = 900;`
  - `pub fn new_join_token() -> String` (`"fj_" + 64 hex`) and `pub fn new_agent_key() -> String` (`"fk_" + 64 hex`)
  - `pub fn enrollment_command(server_url: &str, join_token: &str) -> String`
  - `#[derive(Serialize, ToSchema)] pub struct Enrollment { pub join_token: String, pub expires_at: i64, pub command: String }`
  - `#[derive(Serialize, ToSchema)] pub struct AgentView { pub connected: bool, pub enrolled: bool, pub last_seen_at: Option<i64>, pub agent_version: Option<String>, pub host: Option<serde_json::Value>, pub last_error: Option<String>, pub join_expires_at: Option<i64> }`
  - `#[derive(Serialize, ToSchema)] pub struct EnvView { #[serde(flatten)] pub env: EnvironmentRecord, pub agent: Option<AgentView> }`
  - `#[derive(Serialize, ToSchema)] pub struct CreatedEnv { #[serde(flatten)] pub env: EnvironmentRecord, pub enrollment: Option<Enrollment> }`
  - `pub async fn publish_status(s: &AppState, env_id: i64)`, which sends the environment's current status on the `env_status` bus
  - `CreateEnv.url: Option<String>` (required for `docker-local`, forbidden for `docker-agent`)
  - Route `POST /api/v1/envs/{env}/agent/rotate` → `200 Enrollment` (operation id `rotateAgent`)
  - `GET /api/v1/envs` → `Vec<EnvView>`

- [ ] **Step 1: Write the failing API tests**

`crates/fleetops-api/tests/agents_enroll.rs`:

```rust
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

const ORIGIN: &str = "https://fleet.example.com";

async fn add_agent(t: &TestApp, admin: &Session, name: &str) -> Resp {
    let mut r = req("POST", "/api/v1/envs", Some(json!({"name": name, "kind": "docker-agent"})), Some(admin));
    r.headers_mut().insert("origin", ORIGIN.parse().unwrap());
    t.send(r).await
}

#[tokio::test]
async fn creating_an_agent_environment_returns_a_one_time_command() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = add_agent(&t, &admin, "web-01").await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    assert_eq!(r.json["kind"], "docker-agent");
    assert_eq!(r.json["url"], format!("agent://{}", r.json["id"]));
    let token = r.json["enrollment"]["join_token"].as_str().unwrap();
    assert!(token.starts_with("fj_") && token.len() == 67, "{token}");
    let cmd = r.json["enrollment"]["command"].as_str().unwrap();
    assert!(cmd.contains(&format!("--server {ORIGIN}")), "{cmd}");
    assert!(cmd.contains(&format!("--join-token {token}")), "{cmd}");
    assert!(cmd.contains(&format!("ghcr.io/pravinbanjade/fleetops-agent:{}", env!("CARGO_PKG_VERSION"))), "{cmd}");

    // The token is never shown again.
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let row = &list.json[0];
    assert_eq!(row["name"], "web-01");
    assert_eq!(row["agent"]["connected"], false);
    assert_eq!(row["agent"]["enrolled"], false);
    assert!(!list.json.to_string().contains(token));
}

#[tokio::test]
async fn agent_and_local_inputs_are_validated() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t.send(req("POST", "/api/v1/envs", Some(json!({"name": "a", "kind": "docker-agent", "url": "unix:///x"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "agents take no URL");
    let r = t.send(req("POST", "/api/v1/envs", Some(json!({"name": "b", "kind": "docker-local"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "local needs a URL");
    let r = t.send(req("POST", "/api/v1/envs", Some(json!({"name": "c", "kind": "docker-local", "url": "agent://1"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "agent:// is server-made");
}

#[tokio::test]
async fn an_agent_url_cannot_be_edited() {
    let t = app().await;
    let admin = t.setup_admin().await;
    add_agent(&t, &admin, "web-01").await;
    let r = t.send(req("PATCH", "/api/v1/envs/web-01", Some(json!({"url": "unix:///x.sock"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = t.send(req("PATCH", "/api/v1/envs/web-01", Some(json!({"name": "web-1"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::OK, "renaming is fine");
}

#[tokio::test]
async fn rotating_issues_a_new_token_and_is_admin_only() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let first = add_agent(&t, &admin, "web-01").await.json["enrollment"]["join_token"].clone();
    let mut r = req("POST", "/api/v1/envs/web-01/agent/rotate", None, Some(&admin));
    r.headers_mut().insert("origin", ORIGIN.parse().unwrap());
    let r = t.send(r).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_ne!(r.json["join_token"], first);
    assert!(r.json["command"].as_str().unwrap().contains("--join-token fj_"));

    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let r = t.send(req("POST", "/api/v1/envs/web-01/agent/rotate", None, Some(&bob))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = t.send(req("POST", "/api/v1/envs/nope/agent/rotate", None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rotating_a_local_environment_is_rejected() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = t.send(req("POST", "/api/v1/envs/local/agent/rotate", None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
```

`req` returns a `Request<Body>`, and `headers_mut` is the axum `Request` method. If `req` already sets an `Origin` header for CSRF (check `common/mod.rs:59-92`), *replace* it rather than adding a second one. If the helper sets `Origin: http://localhost` or similar, use that value as `ORIGIN` instead, and assert on it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p fleetops-api --test agents_enroll`
Expected: FAIL. The 201 response lacks `enrollment`, or a 400 or 422 comes back because `url` is missing.

- [ ] **Step 3: Add the registry to AppState**

In `crates/fleetops-api/src/state.rs`: add the field `pub agents: Arc<fleetops_tunnel::registry::Registry>,`, set `agents: Arc::default(),` in `new`, and add:

```rust
    /// Shares `agents` with the env factory, so agent environments see the
    /// sessions the connect handler registers.
    pub fn with_agents(mut self, agents: Arc<fleetops_tunnel::registry::Registry>) -> Self {
        self.agents = agents;
        self
    }
```

Add `fleetops-tunnel.workspace = true` to `crates/fleetops-api/Cargo.toml`.

- [ ] **Step 4: Implement the agents module**

`crates/fleetops-api/src/agents/mod.rs`:

```rust
//! Agent environments: enrollment tokens, the docker run command, and the
//! status updates the connect handler publishes (sub-project 2).

use crate::{auth::token::generate_token, ws::hub::StatusChange, AppState};
use fleetops_store::{AgentRecord, EnvironmentRecord};
use serde::Serialize;
use utoipa::ToSchema;

/// How long a join token stays valid (spec §4.1).
pub const JOIN_TTL_SECS: i64 = 900;
pub const AGENT_IMAGE: &str = "ghcr.io/pravinbanjade/fleetops-agent";

pub fn new_join_token() -> String {
    format!("fj_{}", generate_token())
}

pub fn new_agent_key() -> String {
    format!("fk_{}", generate_token())
}

/// The command an admin pastes on the remote host. The image tag is pinned
/// to this server's version.
pub fn enrollment_command(server_url: &str, join_token: &str) -> String {
    format!(
        "docker run -d --name fleetops-agent --restart unless-stopped \\\n  \
         -v fleetops-agent:/data -v /var/run/docker.sock:/var/run/docker.sock \\\n  \
         --group-add \"$(stat -c %g /var/run/docker.sock)\" \\\n  \
         {AGENT_IMAGE}:{} \\\n  \
         --server {server_url} --join-token {join_token}",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Enrollment {
    /// Shown once. Valid for 15 minutes and for one agent.
    pub join_token: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub command: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentView {
    pub connected: bool,
    pub enrolled: bool,
    pub last_seen_at: Option<i64>,
    pub agent_version: Option<String>,
    pub host: Option<serde_json::Value>,
    pub last_error: Option<String>,
    pub join_expires_at: Option<i64>,
}

impl AgentView {
    pub fn new(rec: AgentRecord, connected: bool) -> Self {
        Self {
            connected,
            enrolled: rec.enrolled,
            last_seen_at: rec.last_seen_at,
            agent_version: rec.agent_version,
            host: rec.host_info.and_then(|h| serde_json::from_str(&h).ok()),
            last_error: rec.last_error,
            join_expires_at: rec.join_expires_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvView {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub agent: Option<AgentView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedEnv {
    #[serde(flatten)]
    pub env: EnvironmentRecord,
    pub enrollment: Option<Enrollment>,
}

/// Announces environment `env_id`'s current status to every websocket.
pub async fn publish_status(s: &AppState, env_id: i64) {
    let Ok(rec) = s.store.get_environment(env_id).await else { return };
    let Ok(env) = s.envs.get(&rec.name) else { return };
    let status = env.status().await;
    let _ = s.hubs.status().send(StatusChange {
        env: rec.name,
        status: serde_json::to_value(status).unwrap_or_default(),
    });
}
```

Check that `crate::auth::token` and `crate::ws::hub` are reachable from the crate root, adding `pub(crate)` where they are private. Add `pub mod agents;` to `lib.rs`.

- [ ] **Step 5: Change the env routes**

In `crates/fleetops-api/src/routes/envs.rs`:

1. `CreateEnv.url` becomes `#[serde(default)] pub url: Option<String>`.
2. Add `use crate::agents::{self, AgentView, CreatedEnv, Enrollment, EnvView};`, `use crate::auth::token::hash_token;`, `use axum::http::HeaderMap;`, and `use fleetops_core::Error;`.
3. A helper for the server URL the agent should dial, taken from the admin's browser:

```rust
/// The URL agents should dial: the origin the admin's browser used. CSRF
/// already requires Origin on mutations, so it is present here.
fn server_url(headers: &HeaderMap) -> ApiResult<String> {
    headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| o.starts_with("https://") || o.starts_with("http://"))
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid("an Origin header is needed to build the agent command".into()).into())
}

async fn new_enrollment(s: &AppState, headers: &HeaderMap, env_id: i64, rotate: bool) -> ApiResult<Enrollment> {
    let token = agents::new_join_token();
    let expires_at = now_unix() + agents::JOIN_TTL_SECS;
    if rotate {
        s.store.rotate_agent(env_id, &hash_token(&token), expires_at).await?;
    }
    let command = agents::enrollment_command(&server_url(headers)?, &token);
    Ok(Enrollment { join_token: token, expires_at, command })
}
```

4. `list` returns `Vec<EnvView>`:

```rust
#[utoipa::path(
    get,
    path = "/envs",
    operation_id = "listEnvs",
    tag = "environments",
    summary = "List environments",
    responses((status = 200, description = "All environments; agent environments include their agent's state", body = Vec<EnvView>))
)]
pub async fn list(State(s): State<AppState>, cu: CurrentUser) -> ApiResult<Json<Vec<EnvView>>> {
    authorize(&cu.user, None, Action::EnvRead)?;
    let mut agents: std::collections::HashMap<i64, _> = s
        .store
        .list_agents()
        .await?
        .into_iter()
        .map(|a| (a.environment_id, a))
        .collect();
    let views = s
        .store
        .list_environments()
        .await?
        .into_iter()
        .map(|env| {
            let agent = agents
                .remove(&env.id)
                .map(|a| AgentView::new(a, s.agents.get(env.id).is_some()));
            EnvView { env, agent }
        })
        .collect();
    Ok(Json(views))
}
```

5. `create` handles both kinds:

```rust
#[utoipa::path(
    post,
    path = "/envs",
    operation_id = "createEnv",
    tag = "environments",
    summary = "Add an environment",
    request_body = CreateEnv,
    responses((status = 201, description = "Created; an agent environment includes its one-time enrollment", body = CreatedEnv))
)]
pub async fn create(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateEnv>,
) -> ApiResult<(StatusCode, Json<CreatedEnv>)> {
    authorize(&cu.user, None, Action::EnvManage)?;
    validate_env_name(&body.name)?;
    let _guard = s.envs.lock_mutations().await;
    let (rec, enrollment) = match body.kind {
        EnvKind::DockerLocal => {
            let url = body.url.ok_or_else(|| Error::Invalid("a local environment needs a Docker URL".into()))?;
            validate_env_url(&url)?;
            let rec = s.store.create_environment(&body.name, body.kind.as_str(), &url, now_unix()).await?;
            (rec, None)
        }
        EnvKind::DockerAgent => {
            if body.url.is_some() {
                return Err(Error::Invalid("an agent environment takes no URL".into()).into());
            }
            let server = server_url(&headers)?;
            let token = agents::new_join_token();
            let expires_at = now_unix() + agents::JOIN_TTL_SECS;
            let rec = s
                .store
                .create_agent_environment(&body.name, &hash_token(&token), expires_at, now_unix())
                .await?;
            let command = agents::enrollment_command(&server, &token);
            (rec, Some(Enrollment { join_token: token, expires_at, command }))
        }
    };
    s.envs.insert(&rec)?;
    audit(&s, Some(cu.user.id), "env.create", "environment", &rec.name, "success").await;
    Ok((StatusCode::CREATED, Json(CreatedEnv { env: rec, enrollment })))
}
```

6. In `update`, after `let current = find(&s, &name).await?;`, add:

```rust
    if current.kind == EnvKind::DockerAgent.as_str() && body.url.is_some() {
        return Err(Error::Invalid("an agent environment's URL cannot be changed".into()).into());
    }
```

and only validate the URL when it changes. Replace `validate_env_url(&new_url)?;` with `if new_url != current.url { validate_env_url(&new_url)?; }`.

7. In `remove`, after `s.envs.remove(&current.name);`, add `s.agents.revoke(current.id);`.

8. The new rotate handler:

```rust
#[utoipa::path(
    post,
    path = "/envs/{env}/agent/rotate",
    operation_id = "rotateAgent",
    tag = "environments",
    summary = "Revoke an agent's key and issue a new join command",
    params(EnvParam),
    responses((status = 200, description = "The new one-time enrollment", body = Enrollment))
)]
pub async fn rotate_agent(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ApiPath(name): ApiPath<String>,
) -> ApiResult<Json<Enrollment>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let _guard = s.envs.lock_mutations().await;
    let current = find(&s, &name).await?;
    if current.kind != EnvKind::DockerAgent.as_str() {
        return Err(Error::Invalid(format!("'{name}' is not an agent environment")).into());
    }
    let enrollment = new_enrollment(&s, &headers, current.id, true).await?;
    s.agents.revoke(current.id);
    audit(&s, Some(cu.user.id), "agent.rotate", "environment", &current.name, "success").await;
    Ok(Json(enrollment))
}
```

The `rotating_a_local_environment_is_rejected` test sends no Origin, but the kind check comes first, so it gets 400 regardless.

9. Register the route in `crates/fleetops-api/src/routes/mod.rs` `env_routes()`: `.routes(routes!(envs::rotate_agent))`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p fleetops-api`
Expected: the new tests PASS. Fix existing tests that read the `createEnv` or `listEnvs` body. Responses are supersets (flattened), so most pass unchanged. The OpenAPI contract test fails until Step 7.

- [ ] **Step 7: Regenerate the OpenAPI document and the web client**

Run: `make openapi && cargo test -p fleetops-api && pnpm -C web typecheck`
Expected: PASS. `web/src/api/gen/types.gen.ts` now has `EnvView`, `CreatedEnv`, `Enrollment` and `AgentView`, and `sdk.gen.ts` has `rotateAgent`. If `EnvironmentsPage.tsx` fails to typecheck because `envsQuery` now yields `EnvView`, change its `EnvironmentRecord` type references to `EnvView`. Task 10 builds the UI.

- [ ] **Step 8: Lint and commit**

Run: `make lint && make test && pnpm -C web lint`

```bash
git add crates web/src/api/gen web/src
git commit -m "feat(api): agent environments with one-time enrollment, rotate, and agent state in the list"
```

---

### Task 7: `GET /api/v1/agent/connect` — the server side of a session

**Files:**
- Create: `crates/fleetops-api/src/agents/connect.rs`, `crates/fleetops-api/src/agents/axum_io.rs`
- Modify: `crates/fleetops-api/src/agents/mod.rs` (`pub mod connect; mod axum_io;`), `crates/fleetops-api/src/routes/mod.rs` (route), `crates/fleetops-api/src/state.rs` (`ApiConfig.agent_ping_interval`)
- Test: `crates/fleetops-api/tests/agents_connect.rs`, `crates/fleetops-api/tests/common/agent.rs` (a scriptable test agent)

**Interfaces:**
- Consumes: `Session`, `Role`, `WsIo`, control `Msg`/`read_msg`/`write_msg`/`PROTO_MAJOR`, `Registry`, `AgentInfo` (Tasks 1, 3, 5); store queries (Task 4); `publish_status`, `new_agent_key` (Task 6)
- Produces:
  - `ApiConfig.agent_ping_interval: Duration` (default 15 s)
  - The route `GET /api/v1/agent/connect`, documented as `operation_id = "agentConnect"` with tag `agents`. It answers 101 on success; 401 for a missing, bad or expired credential; 403 for plain HTTP from a non-loopback peer; 429 when rate-limited.
  - Audit actions `agent.enroll`, `agent.connect`, `agent.disconnect` and `agent.reject`, all with target type `environment`.
  - Test helper `common::agent::TestAgent::connect(base: &str, bearer: &str) -> Result<TestAgent, u16>`, with `hello(proto)`, `welcome() -> Msg`, `next() -> Option<Msg>`, `pong(n)`, `closed()` and `session()`.

- [ ] **Step 1: Write the test agent helper**

Add `fleetops-tunnel.workspace = true` to the api crate's `[dev-dependencies]` (it's also a normal dependency since Task 6). Create `crates/fleetops-api/tests/common/agent.rs`, and add `pub mod agent;` to `common/mod.rs`:

```rust
//! A scriptable agent for connect-handler tests: real WebSocket, real
//! yamux, control messages sent by hand, data streams echoed.

use fleetops_tunnel::{
    control::{read_msg, write_msg, HostInfo, Msg},
    ws::tungstenite_io,
    Role, Session, StreamHandle,
};
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error as WsError};

pub struct TestAgent {
    session: Session,
    rx: ReadHalf<StreamHandle>,
    tx: WriteHalf<StreamHandle>,
}

impl TestAgent {
    /// `base` is `http://127.0.0.1:port`. Err carries the HTTP status of a refused upgrade.
    pub async fn connect(base: &str, bearer: &str) -> Result<TestAgent, u16> {
        let url = format!("{}/api/v1/agent/connect", base.replacen("http", "ws", 1));
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert("authorization", format!("Bearer {bearer}").parse().unwrap());
        let ws = match tokio_tungstenite::connect_async(req).await {
            Ok((ws, _)) => ws,
            Err(WsError::Http(resp)) => return Err(resp.status().as_u16()),
            Err(e) => panic!("connect: {e}"),
        };
        let session = Session::spawn(tungstenite_io(ws), Role::Client, |s| {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(s);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        });
        let control = session.open_stream().await.unwrap();
        let (rx, tx) = tokio::io::split(control);
        Ok(TestAgent { session, rx, tx })
    }

    pub async fn hello(&mut self, proto: u16) {
        let host = HostInfo { hostname: "test-host".into(), os: "linux".into(), arch: "x86_64".into(), docker: "unix:///var/run/docker.sock".into() };
        write_msg(&mut self.tx, &Msg::Hello { proto, agent_version: "0.2.0-test".into(), host }).await.unwrap();
    }

    pub async fn next(&mut self) -> Option<Msg> {
        tokio::time::timeout(std::time::Duration::from_secs(5), read_msg(&mut self.rx))
            .await
            .expect("no control message within 5 s")
            .ok()
            .flatten()
    }

    pub async fn welcome(&mut self) -> Msg {
        let m = self.next().await.expect("welcome");
        assert!(matches!(m, Msg::Welcome { .. }), "{m:?}");
        m
    }

    pub async fn pong(&mut self, n: u64) {
        write_msg(&mut self.tx, &Msg::Pong { n }).await.unwrap();
    }

    pub async fn closed(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.session.closed())
            .await
            .expect("session still open after 5 s");
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
}
```

`TestApp::serve()` already exists (the ws tests use it). It returns the `SocketAddr` of a real listener with `ConnectInfo`, and the tests format it as `http://{addr}`.

- [ ] **Step 2: Write the failing connect tests**

`crates/fleetops-api/tests/agents_connect.rs`:

```rust
mod common;

use common::{agent::TestAgent, *};
use fleetops_tunnel::control::{ByeReason, Msg, PROTO_MAJOR};
use serde_json::json;
use std::time::Duration;

/// `req` sets no Origin; the agent command is built from it.
fn with_origin(mut r: axum::http::Request<axum::body::Body>) -> axum::http::Request<axum::body::Body> {
    r.headers_mut().insert("origin", "http://127.0.0.1".parse().unwrap());
    r
}

async fn agent_env(t: &TestApp, admin: &Session, name: &str) -> String {
    let r = t.send(with_origin(req("POST", "/api/v1/envs", Some(json!({"name": name, "kind": "docker-agent"})), Some(admin)))).await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    r.json["enrollment"]["join_token"].as_str().unwrap().to_string()
}

/// The only agent environment in these tests.
async fn env_id(t: &TestApp) -> i64 {
    t.store.list_agents().await.unwrap()[0].environment_id
}

async fn base(t: &TestApp) -> String {
    format!("http://{}", t.serve().await)
}

async fn enroll(base: &str, token: &str) -> (TestAgent, String) {
    let mut a = TestAgent::connect(base, token).await.unwrap();
    a.hello(PROTO_MAJOR).await;
    match a.welcome().await {
        Msg::Welcome { agent_key: Some(k), env_name, .. } => {
            assert_eq!(env_name, "web-01");
            (a, k)
        }
        m => panic!("{m:?}"),
    }
}

#[tokio::test]
async fn a_join_token_enrolls_and_the_key_reconnects() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;

    let (a, key) = enroll(&base, &token).await;
    assert!(key.starts_with("fk_"));
    eventually("registered", || t.state.agents.connected() == 1).await;
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert_eq!(list.json[0]["agent"]["connected"], true);
    assert_eq!(list.json[0]["agent"]["enrolled"], true);
    assert_eq!(list.json[0]["agent"]["agent_version"], "0.2.0-test");
    drop(a);
    eventually("unregistered", || t.state.agents.connected() == 0).await;

    // The join token is spent; the key works.
    assert_eq!(TestAgent::connect(&base, &token).await.err(), Some(401));
    let mut again = TestAgent::connect(&base, &key).await.unwrap();
    again.hello(PROTO_MAJOR).await;
    assert!(matches!(again.welcome().await, Msg::Welcome { agent_key: None, .. }));
}

#[tokio::test]
async fn a_join_token_enrolls_exactly_one_agent() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (a, b) = tokio::join!(TestAgent::connect(&base, &token), TestAgent::connect(&base, &token));
    let mut winners = 0;
    for agent in [a, b] {
        let Ok(mut agent) = agent else { continue };
        agent.hello(PROTO_MAJOR).await;
        match agent.next().await {
            Some(Msg::Welcome { agent_key: Some(_), .. }) => winners += 1,
            Some(Msg::Welcome { agent_key: None, .. }) => panic!("a second agent got in without a key"),
            _ => {} // refused after the upgrade: the other one won the race
        }
    }
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn bad_and_missing_credentials_are_401() {
    let t = app().await;
    let admin = t.setup_admin().await;
    agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    assert_eq!(TestAgent::connect(&base, "fj_nope").await.err(), Some(401));
    assert_eq!(TestAgent::connect(&base, "fk_nope").await.err(), Some(401));
    assert_eq!(TestAgent::connect(&base, "session-cookie-value").await.err(), Some(401));
}

#[tokio::test]
async fn repeated_bad_credentials_are_rate_limited() {
    let t = app().await;
    let base = base(&t).await;
    let mut last = None;
    for _ in 0..8 {
        last = TestAgent::connect(&base, "fk_wrong").await.err();
    }
    assert_eq!(last, Some(429));
}

#[tokio::test]
async fn an_incompatible_agent_is_told_why_and_the_reason_is_shown() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let mut a = TestAgent::connect(&base, &token).await.unwrap();
    a.hello(PROTO_MAJOR + 1).await;
    assert_eq!(a.next().await, Some(Msg::Bye { reason: ByeReason::Incompatible }));
    a.closed().await;
    let status = t.send(req("GET", "/api/v1/envs/web-01/status", None, Some(&admin))).await;
    assert_eq!(status.json["state"], "down");
    assert!(status.json["reason"].as_str().unwrap().contains("protocol 2"), "{:?}", status.json);
    // The join token is not spent by a refused agent.
    let (_ok, _key) = enroll(&base, &token).await;
}

#[tokio::test]
async fn a_second_connection_replaces_the_first() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut first, key) = enroll(&base, &token).await;
    let mut second = TestAgent::connect(&base, &key).await.unwrap();
    second.hello(PROTO_MAJOR).await;
    second.welcome().await;
    assert_eq!(first.next().await, Some(Msg::Bye { reason: ByeReason::Replaced }));
    first.closed().await;
    assert_eq!(t.state.agents.connected(), 1);
}

#[tokio::test]
async fn rotating_revokes_the_live_session_and_the_key() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, key) = enroll(&base, &token).await;
    let r = t.send(with_origin(req("POST", "/api/v1/envs/web-01/agent/rotate", None, Some(&admin)))).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(a.next().await, Some(Msg::Bye { reason: ByeReason::Revoked }));
    a.closed().await;
    assert_eq!(TestAgent::connect(&base, &key).await.err(), Some(401));
}

#[tokio::test]
async fn deleting_an_agent_environment_revokes_its_session() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, _key) = enroll(&base, &token).await;
    let r = t.send(req("DELETE", "/api/v1/envs/web-01", None, Some(&admin))).await;
    assert_eq!(r.status, 204);
    assert_eq!(a.next().await, Some(Msg::Bye { reason: ByeReason::Revoked }));
    a.closed().await;
    eventually("registry emptied", || t.state.agents.connected() == 0).await;
}

#[tokio::test]
async fn an_agent_that_stops_answering_pings_is_dropped() {
    let mut config = fleetops_api::ApiConfig::default();
    config.agent_ping_interval = Duration::from_millis(100);
    let t = app_with(config).await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, _key) = enroll(&base, &token).await;
    // Answer one ping, then go silent.
    if let Some(Msg::Ping { n }) = a.next().await {
        a.pong(n).await;
    }
    a.closed().await;
    eventually("unregistered", || t.state.agents.connected() == 0).await;
    let rows = t.store.list_audit(50, None).await.unwrap();
    assert!(rows.iter().any(|r| r.action == "agent.enroll"));
    assert!(rows.iter().any(|r| r.action == "agent.disconnect"));
}

#[tokio::test]
async fn the_server_opens_data_streams_to_the_agent() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (_a, _key) = enroll(&base, &token).await;
    eventually("registered", || t.state.agents.connected() == 1).await;
    let live = t.state.agents.get(env_id(&t).await).unwrap();
    let mut s = live.session.open_stream().await.unwrap();
    s.write_all(b"echo").await.unwrap();
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"echo");
}
```

`eventually(what, cond)` and `TestApp::serve() -> SocketAddr` already exist in `common/mod.rs`.

Note on the rate limit: the limiter allows `LOGIN_MAX_FAILURES` (5) per window, so the 6th to 8th attempts get 429.

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p fleetops-api --test agents_connect`
Expected: FAIL. Every connect returns 404 (no route) or 401 from the fallback.

- [ ] **Step 4: The axum WebSocket adapter**

`crates/fleetops-api/src/agents/axum_io.rs`:

```rust
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
```

Add `bytes.workspace = true` to the api crate's dependencies. If Task 3 fell back to a hand-written adapter, mirror it here.

- [ ] **Step 5: The connect handler and session loop**

Add `agent_ping_interval: Duration` to `ApiConfig` with the doc comment `/// How often the server pings each agent; two unanswered pings end the session.` and default `Duration::from_secs(15)`.

`crates/fleetops-api/src/agents/connect.rs`:

```rust
//! `GET /api/v1/agent/connect`: authenticates an agent, upgrades to a
//! WebSocket, and runs the server side of its tunnel (spec §4.2, §4.3).

use super::{axum_io, new_agent_key, publish_status};
use crate::{
    auth::{extract::ClientMeta, token::hash_token},
    routes::audit,
    ApiError, AppState,
};
use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use fleetops_core::{now_unix, Error};
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, Msg, PROTO_MAJOR},
    registry::AgentInfo,
    ws::MAX_WS_MESSAGE,
    Role, Session,
};
use std::{net::IpAddr, time::{Duration, Instant}};
use tokio::{io::{ReadHalf, WriteHalf}, sync::{mpsc, oneshot}};

const HANDSHAKE: Duration = Duration::from_secs(10);
/// At most one last-seen write per this long while a session is up.
const SEEN_EVERY: Duration = Duration::from_secs(60);

enum Credential {
    Join { env_id: i64, join_hash: String },
    Key { env_id: i64 },
}

async fn authenticate(s: &AppState, headers: &HeaderMap) -> Result<Credential, Error> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| Error::Unauthorized("agent credential required".into()))?;
    let hash = hash_token(bearer);
    if bearer.starts_with("fj_") {
        if let Some(env_id) = s.store.agent_by_join_token(&hash, now_unix()).await? {
            return Ok(Credential::Join { env_id, join_hash: hash });
        }
    } else if bearer.starts_with("fk_") {
        if let Some(env_id) = s.store.agent_by_key(&hash).await? {
            return Ok(Credential::Key { env_id });
        }
    }
    Err(Error::Unauthorized("unknown or expired agent credential".into()))
}

#[utoipa::path(
    get,
    path = "/agent/connect",
    operation_id = "agentConnect",
    tag = "agents",
    summary = "Agent tunnel (WebSocket upgrade, Bearer fj_… or fk_…). Not for browsers.",
    responses(
        (status = 101, description = "Tunnel open"),
        (status = 401, description = "Missing, unknown or expired credential"),
        (status = 403, description = "Plain HTTP from a non-loopback address"),
        (status = 429, description = "Too many failed attempts")
    )
)]
pub async fn handler(
    State(s): State<AppState>,
    meta: ClientMeta,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let loopback = meta.ip.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !meta.tls && !loopback {
        return ApiError::from(Error::Forbidden("agents must connect over HTTPS".into())).into_response();
    }
    let limit_key = format!("agent|{}", meta.ip);
    if let Err(e) = s.limiter.try_acquire(&limit_key, Instant::now()) {
        return ApiError::from(e).into_response();
    }
    let cred = match authenticate(&s, &headers).await {
        Ok(c) => c,
        Err(e) => return ApiError::from(e).into_response(),
    };
    s.limiter.reset(&limit_key);
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run(s, cred, socket))
}

/// The agent's control stream, split.
struct Control {
    rx: ReadHalf<fleetops_tunnel::StreamHandle>,
    tx: WriteHalf<fleetops_tunnel::StreamHandle>,
}

async fn run(s: AppState, cred: Credential, socket: WebSocket) {
    let (first_tx, first_rx) = oneshot::channel();
    let mut first_tx = Some(first_tx);
    // The agent opens exactly one stream, the control stream. Any other
    // stream it opens is dropped, which resets it.
    let session = Session::spawn(axum_io::io(socket), Role::Server, move |stream| {
        if let Some(tx) = first_tx.take() {
            let _ = tx.send(stream);
        }
    });
    let env_id = match &cred {
        Credential::Join { env_id, .. } | Credential::Key { env_id } => *env_id,
    };
    if let Err(reason) = serve(&s, cred, &session, first_rx).await {
        tracing::info!(env_id, %reason, "agent session ended");
    }
    session.close().await;
    publish_status(&s, env_id).await;
}

async fn handshake(first: oneshot::Receiver<fleetops_tunnel::StreamHandle>) -> Result<(Control, AgentInfo, u16), String> {
    let stream = tokio::time::timeout(HANDSHAKE, first)
        .await
        .map_err(|_| "no control stream".to_string())?
        .map_err(|_| "session closed before the control stream".to_string())?;
    let (rx, tx) = tokio::io::split(stream);
    let mut c = Control { rx, tx };
    match tokio::time::timeout(HANDSHAKE, read_msg(&mut c.rx)).await {
        Ok(Ok(Some(Msg::Hello { proto, agent_version, host }))) => {
            Ok((c, AgentInfo { agent_version, host, connected_at: now_unix() }, proto))
        }
        other => Err(format!("expected hello, got {other:?}")),
    }
}

async fn serve(
    s: &AppState,
    cred: Credential,
    session: &Session,
    first: oneshot::Receiver<fleetops_tunnel::StreamHandle>,
) -> Result<(), String> {
    let (mut c, info, proto) = handshake(first).await?;
    let (env_id, join_hash) = match cred {
        Credential::Join { env_id, join_hash } => (env_id, Some(join_hash)),
        Credential::Key { env_id } => (env_id, None),
    };
    let env = s.store.get_environment(env_id).await.map_err(|e| e.to_string())?;
    let host_json = serde_json::to_string(&info.host).ok();

    if proto != PROTO_MAJOR {
        let reason = format!(
            "agent v{} uses protocol {proto}; server needs {PROTO_MAJOR}",
            info.agent_version
        );
        let _ = write_msg(&mut c.tx, &Msg::Bye { reason: ByeReason::Incompatible }).await;
        let _ = s
            .store
            .record_agent_seen(env_id, now_unix(), Some(&info.agent_version), host_json.as_deref(), Some(&reason))
            .await;
        s.agents.set_offline_reason(env_id, &reason);
        audit(s, None, "agent.reject", "environment", &env.name, "failure").await;
        return Err(reason);
    }

    // Enrollment: the join token is spent only now, after a compatible hello.
    let agent_key = match join_hash {
        Some(join_hash) => {
            let key = new_agent_key();
            let won = s
                .store
                .enroll_agent(env_id, &join_hash, &hash_token(&key), now_unix())
                .await
                .map_err(|e| e.to_string())?;
            if !won {
                return Err("join token already used".into());
            }
            audit(s, None, "agent.enroll", "environment", &env.name, "success").await;
            Some(key)
        }
        None => None,
    };
    write_msg(
        &mut c.tx,
        &Msg::Welcome {
            env_name: env.name.clone(),
            server_version: env!("CARGO_PKG_VERSION").into(),
            agent_key,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    let (bye_tx, mut bye_rx) = mpsc::channel(1);
    let id = s.agents.insert(env_id, session.clone(), info.clone(), bye_tx);
    let _ = s
        .store
        .record_agent_seen(env_id, now_unix(), Some(&info.agent_version), host_json.as_deref(), None)
        .await;
    audit(s, None, "agent.connect", "environment", &env.name, "success").await;
    publish_status(s, env_id).await;

    let reason = control_loop(s, env_id, &mut c, session, &mut bye_rx).await;

    s.agents.remove(env_id, id, &reason);
    let _ = s.store.record_agent_seen(env_id, now_unix(), None, None, None).await;
    audit(s, None, "agent.disconnect", "environment", &env.name, "success").await;
    Err(reason)
}

/// Runs until the session ends; returns why, for the environment's status.
async fn control_loop(
    s: &AppState,
    env_id: i64,
    c: &mut Control,
    session: &Session,
    bye_rx: &mut mpsc::Receiver<ByeReason>,
) -> String {
    let mut ping = tokio::time::interval(s.config.agent_ping_interval);
    ping.tick().await; // the first tick is immediate
    let mut seen = tokio::time::interval(SEEN_EVERY);
    seen.tick().await;
    let mut n = 0u64;
    let mut unanswered = 0u32;
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if unanswered >= 2 {
                    return "no answer to two pings".into();
                }
                n += 1;
                unanswered += 1;
                if write_msg(&mut c.tx, &Msg::Ping { n }).await.is_err() {
                    return "connection lost".into();
                }
            }
            msg = read_msg(&mut c.rx) => match msg {
                Ok(Some(Msg::Pong { .. })) => unanswered = 0,
                Ok(Some(Msg::Bye { reason })) => return format!("agent said bye ({reason:?})"),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return "connection lost".into(),
            },
            Some(reason) = bye_rx.recv() => {
                let _ = write_msg(&mut c.tx, &Msg::Bye { reason }).await;
                return match reason {
                    ByeReason::Replaced => "replaced by a newer connection".into(),
                    ByeReason::Revoked => "key revoked".into(),
                    other => format!("{other:?}"),
                };
            }
            _ = session.closed() => return "connection lost".into(),
            _ = seen.tick() => {
                // Fails only if the environment was deleted; the bye follows.
                let _ = s.store.record_agent_seen(env_id, now_unix(), None, None, None).await;
            }
        }
    }
}
```

`audit(s, None, …)` assumes `routes::audit` takes `Option<i64>` for the user, as the existing calls `audit(&s, Some(cu.user.id), …)` show. Make `audit` `pub(crate)` if it isn't already.

`LoginLimiter::reset(&self, key: &str)` exists (the login flow calls it on success). Confirm the name in `auth/limiter.rs`. The 429 test needs `try_acquire` to fail on the 6th attempt, which it does by construction.

On the disconnect audit: the spec debounces disconnect entries to one per 5 minutes per environment. Add a `last_disconnect_audit: Mutex<HashMap<i64, Instant>>` to `AppState` (default empty), and skip the `agent.disconnect` audit when the previous one for that `env_id` was less than 300 s ago. Add this test to the same file:

```rust
#[tokio::test]
async fn flapping_agents_audit_one_disconnect_per_five_minutes() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (a, key) = enroll(&base, &token).await;
    drop(a);
    for _ in 0..3 {
        eventually("unregistered", || t.state.agents.connected() == 0).await;
        let mut b = TestAgent::connect(&base, &key).await.unwrap();
        b.hello(PROTO_MAJOR).await;
        b.welcome().await;
        drop(b);
    }
    eventually("unregistered", || t.state.agents.connected() == 0).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let n = t.store.list_audit(100, None).await.unwrap().iter().filter(|r| r.action == "agent.disconnect").count();
    assert_eq!(n, 1);
}
```

- [ ] **Step 6: Register the route**

In `crates/fleetops-api/src/routes/mod.rs` `api()`, add `.routes(routes!(crate::agents::connect::handler))` next to the `ws::handler` line. The route needs no `CurrentUser`, so CSRF and session auth don't apply. Credentials come only from `Authorization: Bearer`.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p fleetops-api --test agents_connect`
Expected: PASS (11 tests). Then `make openapi` (the route is documented) and `cargo test -p fleetops-api`.

- [ ] **Step 8: Lint and commit**

Run: `make lint && make test && pnpm -C web check:api`

```bash
git add crates web/src/api/gen
git commit -m "feat(api): agent connect endpoint — enrollment, heartbeat, replacement, revocation"
```

---

### Task 8: wire the registry into the server, and test a full remote environment against Docker

**Files:**
- Modify: `crates/fleetops-server/src/lib.rs`, `crates/fleetops-server/Cargo.toml` (`fleetops-tunnel.workspace = true`)
- Test: `crates/fleetops-server/tests/live.rs` (new docker-it test)

**Interfaces:**
- Consumes: `DockerEnvFactory::new`, `AppState::with_agents`, `Registry` (Tasks 5, 6)
- Produces: a running server whose `docker-agent` environments are served by connected agents

- [ ] **Step 1: Write the failing docker-it test**

Add to `crates/fleetops-server/tests/live.rs` (it is `#![cfg(feature = "docker-it")]` and already starts a real server with `DOCKER_HOST`). Reuse its existing server-start helper. Here it's called `start()`, returning `(base, client-with-admin-session)`; adapt to the real names in the file:

```rust
#[tokio::test]
async fn an_agent_environment_serves_the_same_api_as_local() {
    let srv = start().await;
    let created = srv.post_json("/api/v1/envs", json!({"name": "remote", "kind": "docker-agent"})).await;
    let token = created["enrollment"]["join_token"].as_str().unwrap().to_string();

    // A minimal agent in-process: the real tunnel, forwarding to DOCKER_HOST.
    let target = fleetops_tunnel::Target::parse(&docker_host()).unwrap();
    let url = format!("{}/api/v1/agent/connect", srv.base.replacen("http", "ws", 1));
    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let session = fleetops_tunnel::Session::spawn(
        fleetops_tunnel::ws::tungstenite_io(ws),
        fleetops_tunnel::Role::Client,
        move |s| {
            let target = target.clone();
            tokio::spawn(async move { let _ = fleetops_tunnel::forward(s, &target).await; });
        },
    );
    let control = session.open_stream().await.unwrap();
    let (mut rx, mut tx) = tokio::io::split(control);
    use fleetops_tunnel::control::*;
    write_msg(&mut tx, &Msg::Hello { proto: PROTO_MAJOR, agent_version: "it".into(), host: HostInfo::default() }).await.unwrap();
    assert!(matches!(read_msg(&mut rx).await.unwrap(), Some(Msg::Welcome { .. })));
    // Answer pings in the background.
    tokio::spawn(async move {
        while let Ok(Some(m)) = read_msg(&mut rx).await {
            if let Msg::Ping { n } = m {
                let _ = write_msg(&mut tx, &Msg::Pong { n }).await;
            }
        }
    });

    let status = srv.get_json("/api/v1/envs/remote/status").await;
    assert_eq!(status["state"], "up", "{status}");
    let local = srv.get_json("/api/v1/envs/local/containers").await;
    let remote = srv.get_json("/api/v1/envs/remote/containers").await;
    assert_eq!(local.as_array().unwrap().len(), remote.as_array().unwrap().len(), "same daemon, same list");
    let info = srv.get_json("/api/v1/envs/remote/info").await;
    assert!(info["ServerVersion"].is_string());

    session.close().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let status = srv.get_json("/api/v1/envs/remote/status").await;
    assert_eq!(status["state"], "down");
}
```

Add `fleetops-tunnel.workspace = true` and `serde_json` (if missing) to `crates/fleetops-server/[dev-dependencies]`. If `live.rs` has no `post_json`/`get_json` helpers, write the requests inline with the client it already uses.

- [ ] **Step 2: Run to verify it fails**

Run: `DOCKER_HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}') cargo test -p fleetops-server --features docker-it --test live an_agent_environment`
Expected: FAIL. Status stays `down` because the server's factory and state hold different registries (Task 5 left `DockerEnvFactory::default()` in `lib.rs`).

- [ ] **Step 3: Share one registry**

In `crates/fleetops-server/src/lib.rs` `run`:

```rust
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    let state = AppState::new(
        store.clone(),
        ApiConfig {
            hsts: config.hsts,
            ..ApiConfig::default()
        },
        Arc::new(DockerEnvFactory::new(agents.clone())),
    )
    .with_agents(agents);
```

If `live.rs` builds its own `AppState` instead of calling `run`, apply the same pattern there.

- [ ] **Step 4: Run the test and the whole live suite**

Run: `make test-docker`
Expected: PASS, including the docker_it pass through the tunnel (Task 2) and the new live test.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-server
git commit -m "feat(server): one agent registry shared by the env factory and the connect endpoint"
```

---

### Task 9: the `fleetops-agent` binary

**Files:**
- Create: `crates/fleetops-agent/Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/config.rs`, `src/creds.rs`, `src/backoff.rs`, `src/dial.rs`, `src/session.rs`
- Test: unit tests in `backoff.rs` and `creds.rs`; `crates/fleetops-agent/tests/agent.rs`

**Interfaces:**
- Consumes: the tunnel crate (`Session`, `Role`, `forward`, `Target`, `tungstenite_io`, control types)
- Produces:
  - `pub struct Config { pub server: String, pub join_token: Option<String>, pub data: PathBuf, pub docker: String, pub ca: Option<PathBuf>, pub log: String }` (clap, env prefix `FLEETOPS_AGENT_`)
  - `pub async fn run(config: Config) -> anyhow::Result<()>`, which returns `Err` only for conditions retrying can't fix: revoked, replaced, incompatible, unwritable data dir, no credential.
  - `pub struct Backoff` with `new(min, max)`, `next() -> Duration` and `reset()`
  - `creds::{load_key(dir) -> io::Result<Option<String>>, save_key(dir, key) -> io::Result<()>, probe_writable(dir) -> io::Result<()>}`

- [ ] **Step 1: The crate manifest**

```toml
[package]
name = "fleetops-agent"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "fleetops-agent"
path = "src/main.rs"

[dependencies]
fleetops-tunnel.workspace = true
anyhow.workspace = true
clap.workspace = true
futures-util.workspace = true
rand.workspace = true
rustls.workspace = true
serde_json.workspace = true
tokio.workspace = true
tokio-tungstenite = { workspace = true, features = ["connect", "rustls-tls-webpki-roots"] }
tracing.workspace = true
tracing-subscriber.workspace = true
webpki-roots = "0.26"

[dev-dependencies]
fleetops-api.workspace = true
fleetops-docker.workspace = true
fleetops-store.workspace = true
axum.workspace = true
tempfile.workspace = true
```

Check `tokio-tungstenite 0.29`'s feature names in `~/.cargo/registry/src/*/tokio-tungstenite-0.29*/Cargo.toml`. You need a rustls client with webpki roots, using the `ring` provider that the workspace `rustls` already enables. If enabling `rustls-tls-webpki-roots` pulls `aws-lc-rs` and breaks the zigbuild musl image build, use the `__rustls-tls` feature plus our own `ClientConfig` (Step 6 builds one anyway) and drop `webpki-roots` from the tungstenite feature list.

- [ ] **Step 2: Write the failing unit tests**

`src/backoff.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn doubles_to_the_cap_with_jitter_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let expect = [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0];
        for want in expect {
            let got = b.next().as_secs_f64();
            assert!(got >= want * 0.8 - 1e-9 && got <= want * 1.2 + 1e-9, "want ~{want}, got {got}");
        }
        b.reset();
        assert!(b.next() <= Duration::from_millis(1200));
    }
}
```

`src/creds.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_loads_back_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_key(dir.path()).unwrap(), None);
        save_key(dir.path(), "fk_abc").unwrap();
        assert_eq!(load_key(dir.path()).unwrap().as_deref(), Some("fk_abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_dir_fails_the_probe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions; the test is meaningless there.
        if std::fs::write(dir.path().join("x"), b"x").is_ok() {
            return;
        }
        assert!(probe_writable(dir.path()).is_err());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Create `src/lib.rs` with `pub mod backoff; pub mod config; pub mod creds; mod dial; mod session;`, plus empty `config.rs`, `dial.rs` and `session.rs`.
Run: `cargo test -p fleetops-agent --lib`
Expected: FAIL to compile (`cannot find type Backoff`).

- [ ] **Step 4: Implement backoff and creds**

`src/backoff.rs`:

```rust
//! Reconnect delays: doubling from `min` to `max`, ±20 % jitter so a fleet
//! of agents does not reconnect in lockstep after a server restart.

use rand::Rng;
use std::time::Duration;

pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self { min, max, next: min }
    }

    pub fn next(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(self.max);
        base.mul_f64(rand::thread_rng().gen_range(0.8..=1.2))
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }
}
```

`src/creds.rs`:

```rust
//! The agent's long-lived key, in `<data>/agent.key`, readable by its owner only.

use std::{fs, io, path::Path};

pub const KEY_FILE: &str = "agent.key";

pub fn load_key(dir: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(dir.join(KEY_FILE)) {
        Ok(s) => Ok(Some(s.trim().to_string()).filter(|k| !k.is_empty())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Written to a temporary file and renamed, so a crash never leaves half a key.
pub fn save_key(dir: &Path, key: &str) -> io::Result<()> {
    let tmp = dir.join(format!("{KEY_FILE}.tmp"));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        io::Write::write_all(&mut f, key.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dir.join(KEY_FILE))
}

/// Fails if the key could not be saved, so a join token is never spent
/// by an agent that would lose the key it gets for it.
pub fn probe_writable(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(".write-probe");
    fs::write(&probe, b"ok")?;
    fs::remove_file(probe)
}
```

- [ ] **Step 5: Run the unit tests**

Run: `cargo test -p fleetops-agent --lib`
Expected: PASS (3 tests).

- [ ] **Step 6: Config, dial, session, run, main**

`src/config.rs`:

```rust
use clap::Parser;
use std::path::PathBuf;

/// fleetops-agent: connects this host's Docker to a FleetOps server.
#[derive(Debug, Clone, Parser)]
#[command(version)]
pub struct Config {
    /// The FleetOps server, e.g. https://fleet.example.com
    #[arg(long, env = "FLEETOPS_AGENT_SERVER")]
    pub server: String,
    /// One-time join token from the server's "Add agent" dialog. Used only
    /// until a key is stored in --data.
    #[arg(long, env = "FLEETOPS_AGENT_JOIN_TOKEN")]
    pub join_token: Option<String>,
    /// Holds agent.key.
    #[arg(long, env = "FLEETOPS_AGENT_DATA", default_value = "/data")]
    pub data: PathBuf,
    /// The Docker endpoint to manage.
    #[arg(long, env = "FLEETOPS_AGENT_DOCKER", default_value = "unix:///var/run/docker.sock")]
    pub docker: String,
    /// Extra CA certificate (PEM) to trust for the server, e.g. its self-signed certificate.
    #[arg(long, env = "FLEETOPS_AGENT_CA")]
    pub ca: Option<PathBuf>,
    #[arg(long, env = "FLEETOPS_AGENT_LOG", default_value = "info")]
    pub log: String,
}
```

`src/dial.rs`:

```rust
//! One WebSocket connection to the server's agent endpoint.

use anyhow::{anyhow, Context};
use fleetops_tunnel::ws::MAX_WS_MESSAGE;
use std::{path::Path, sync::Arc};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Error as WsError},
    Connector, MaybeTlsStream, WebSocketStream,
};

pub enum DialError {
    /// The server refused the credential (401) or the client (403).
    Refused(u16),
    Other(anyhow::Error),
}

pub fn tls_config(ca: Option<&Path>) -> anyhow::Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca {
        use rustls::pki_types::{pem::PemObject, CertificateDer};
        for cert in CertificateDer::pem_file_iter(path).with_context(|| format!("reading {}", path.display()))? {
            roots.add(cert?)?;
        }
    }
    Ok(rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
}

pub async fn dial(
    server: &str,
    bearer: &str,
    tls: Arc<rustls::ClientConfig>,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, DialError> {
    let base = server.trim_end_matches('/');
    let url = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/api/v1/agent/connect")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/api/v1/agent/connect")
    } else {
        return Err(DialError::Other(anyhow!("--server must start with https:// or http://")));
    };
    let mut req = url.into_client_request().map_err(|e| DialError::Other(e.into()))?;
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {bearer}").parse().map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| DialError::Other(e.into()))?,
    );
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE));
    match tokio_tungstenite::connect_async_tls_with_config(req, Some(ws_config), true, Some(Connector::Rustls(tls))).await {
        Ok((ws, _)) => Ok(ws),
        Err(WsError::Http(resp)) => Err(DialError::Refused(resp.status().as_u16())),
        Err(e) => Err(DialError::Other(e.into())),
    }
}
```

(`WebSocketConfig` builder method names changed across tungstenite versions. If `.max_message_size(...)` isn't a method in 0.29, set the public fields instead: `let mut c = WebSocketConfig::default(); c.max_message_size = Some(...); c.max_frame_size = Some(...);`.)

`src/session.rs`:

```rust
//! One connected session: hello, welcome (save a new key first), then
//! answer pings until the server or the network ends it.

use crate::creds;
use anyhow::anyhow;
use fleetops_tunnel::{
    control::{read_msg, write_msg, ByeReason, HostInfo, Msg, PROTO_MAJOR},
    forward,
    ws::tungstenite_io,
    Role, Session, Target,
};
use std::{path::Path, time::Duration};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub enum Ended {
    /// The network or server went away; reconnect.
    Dropped,
    /// Retrying cannot help.
    Fatal(anyhow::Error),
}

fn host_info(docker: &str) -> HostInfo {
    HostInfo {
        hostname: std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        docker: docker.into(),
    }
}

pub async fn run(
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    target: Target,
    docker_url: &str,
    data: &Path,
) -> Ended {
    let session = Session::spawn(tungstenite_io(ws), Role::Client, move |stream| {
        let target = target.clone();
        tokio::spawn(async move {
            if let Err(e) = forward(stream, &target).await {
                tracing::debug!(error = %e, "stream to Docker ended");
            }
        });
    });
    let ended = control(&session, docker_url, data).await;
    session.close().await;
    ended
}

async fn control(session: &Session, docker_url: &str, data: &Path) -> Ended {
    let stream = match session.open_stream().await {
        Ok(s) => s,
        Err(_) => return Ended::Dropped,
    };
    let (mut rx, mut tx) = tokio::io::split(stream);
    let hello = Msg::Hello { proto: PROTO_MAJOR, agent_version: env!("CARGO_PKG_VERSION").into(), host: host_info(docker_url) };
    if write_msg(&mut tx, &hello).await.is_err() {
        return Ended::Dropped;
    }
    match tokio::time::timeout(Duration::from_secs(10), read_msg(&mut rx)).await {
        Ok(Ok(Some(Msg::Welcome { env_name, agent_key, .. }))) => {
            if let Some(key) = agent_key {
                if let Err(e) = creds::save_key(data, &key) {
                    return Ended::Fatal(anyhow!("enrolled, but could not save the key in {}: {e}; rotate the agent in the UI and start again", data.display()));
                }
                tracing::info!(env = %env_name, "enrolled; key saved");
            }
            tracing::info!(env = %env_name, "connected");
        }
        Ok(Ok(Some(Msg::Bye { reason }))) => return bye(reason),
        _ => return Ended::Dropped,
    }
    loop {
        match read_msg(&mut rx).await {
            Ok(Some(Msg::Ping { n })) => {
                if write_msg(&mut tx, &Msg::Pong { n }).await.is_err() {
                    return Ended::Dropped;
                }
            }
            Ok(Some(Msg::Bye { reason })) => return bye(reason),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return Ended::Dropped,
        }
    }
}

fn bye(reason: ByeReason) -> Ended {
    match reason {
        ByeReason::Shutdown => Ended::Dropped,
        ByeReason::Revoked => Ended::Fatal(anyhow!("the server revoked this agent's key; re-enroll with a new join command")),
        ByeReason::Replaced => Ended::Fatal(anyhow!("another agent connected with this key; only one may run per environment")),
        ByeReason::Incompatible => Ended::Fatal(anyhow!("this agent's protocol version is not supported by the server; use the image tag from the server's command")),
    }
}
```

`src/lib.rs`:

```rust
//! fleetops-agent: dials out to a FleetOps server and serves its Docker
//! socket through the tunnel (sub-project 2).

pub mod backoff;
pub mod config;
pub mod creds;
mod dial;
mod session;

pub use config::Config;

use anyhow::{anyhow, Context};
use backoff::Backoff;
use std::{sync::Arc, time::{Duration, Instant}};

/// A session that stayed up this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);

pub async fn run(config: Config) -> anyhow::Result<()> {
    creds::probe_writable(&config.data)
        .with_context(|| format!("the data directory {} must be writable", config.data.display()))?;
    let target = fleetops_tunnel::Target::parse(&config.docker)?;
    let tls = Arc::new(dial::tls_config(config.ca.as_deref())?);
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
    loop {
        let key = creds::load_key(&config.data)?;
        let bearer = match (&key, &config.join_token) {
            (Some(k), _) => k.clone(),
            (None, Some(j)) => j.clone(),
            (None, None) => return Err(anyhow!("no agent key in {} and no --join-token", config.data.display())),
        };
        let started = Instant::now();
        match dial::dial(&config.server, &bearer, tls.clone()).await {
            Ok(ws) => match session::run(ws, target.clone(), &config.docker, &config.data).await {
                session::Ended::Fatal(e) => return Err(e),
                session::Ended::Dropped => tracing::warn!("disconnected from {}", config.server),
            },
            Err(dial::DialError::Refused(401)) if key.is_some() => {
                return Err(anyhow!("the server rejected this agent's key (revoked or environment deleted)"));
            }
            Err(dial::DialError::Refused(code)) => tracing::warn!(code, "server refused the connection"),
            Err(dial::DialError::Other(e)) => tracing::warn!(error = %e, "cannot reach {}", config.server),
        }
        if started.elapsed() >= STABLE {
            backoff.reset();
        }
        tokio::time::sleep(backoff.next()).await;
    }
}
```

`src/main.rs`:

```rust
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = fleetops_agent::Config::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&config.log))
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    fleetops_agent::run(config).await
}
```

- [ ] **Step 7: Write the failing integration tests**

`crates/fleetops-agent/tests/agent.rs`. It uses an in-process server built from `fleetops-api` with an in-memory store, the agent library forwarding to a fake Docker unix socket that answers `/version`, and a real loopback listener:

```rust
use fleetops_agent::Config;
use fleetops_api::{ApiConfig, AppState};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Answers every request with a fixed /version body.
fn fake_docker(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("docker.sock");
    let l = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = c.read(&mut buf).await {
                    if n == 0 { break; }
                    let body = r#"{"Version":"27.0.0","ApiVersion":"1.47","Os":"linux","Arch":"x86_64"}"#;
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                    if c.write_all(resp.as_bytes()).await.is_err() { break; }
                }
            });
        }
    });
    path
}

struct Server {
    base: String,
    state: AppState,
    task: tokio::task::JoinHandle<()>,
}

async fn server(store: Store, port: u16) -> Server {
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    let state = AppState::new(store.clone(), ApiConfig::default(), Arc::new(DockerEnvFactory::new(agents.clone())))
        .with_agents(agents);
    state.envs.load(&store).await.unwrap();
    let app = fleetops_api::app(state.clone(), axum::Router::new());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Server { base, state, task }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() { return; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Creates the environment straight in the store; returns (env id, join token).
async fn agent_env(store: &Store) -> (i64, String) {
    let token = fleetops_api::agents::new_join_token();
    let hash = fleetops_api::auth_token_hash(&token);
    let env = store.create_agent_environment("web-01", &hash, fleetops_core::now_unix() + 900, fleetops_core::now_unix()).await.unwrap();
    (env.id, token)
}

fn config(base: &str, data: &std::path::Path, docker: &std::path::Path, join: Option<String>) -> Config {
    Config {
        server: base.into(),
        join_token: join,
        data: data.into(),
        docker: format!("unix://{}", docker.display()),
        ca: None,
        log: "warn".into(),
    }
}

#[tokio::test]
async fn the_agent_enrolls_and_its_environment_comes_up() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("data");
    tokio::spawn(fleetops_agent::run(config(&srv.base, &data, &docker, Some(token))));

    eventually("connected", || srv.state.agents.connected() == 1).await;
    assert!(data.join("agent.key").exists());
    let env = srv.state.envs.get("web-01").unwrap();
    match env.status().await {
        fleetops_core::env::EnvStatus::Up { version } => assert_eq!(version.version, "27.0.0"),
        other => panic!("{other:?}"),
    }
    assert!(store.get_agent(env_id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn agent_reconnects_after_the_server_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (_env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let port: u16 = srv.base.rsplit(':').next().unwrap().parse().unwrap();
    let data = dir.path().join("data");
    tokio::spawn(fleetops_agent::run(config(&srv.base, &data, &docker, Some(token))));
    eventually("connected", || srv.state.agents.connected() == 1).await;

    // Stop the old process: its listener and its live tunnel. (axum's
    // per-connection tasks outlive the aborted serve future.)
    srv.task.abort();
    let _ = srv.task.await;
    srv.state.agents.get(_env_id).unwrap().session.close().await;
    // A new process: same store and port, empty registry.
    let srv = server(store.clone(), port).await;
    for _ in 0..100 {
        if srv.state.agents.connected() == 1 { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(srv.state.agents.connected(), 1, "the agent came back with its key");
}

#[tokio::test]
async fn a_revoked_key_stops_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("data");
    let agent = tokio::spawn(fleetops_agent::run(config(&srv.base, &data, &docker, Some(token))));
    eventually("connected", || srv.state.agents.connected() == 1).await;

    store.rotate_agent(env_id, "unused", fleetops_core::now_unix() + 900).await.unwrap();
    srv.state.agents.revoke(env_id);
    let res = tokio::time::timeout(Duration::from_secs(5), agent).await.expect("agent kept running").unwrap();
    let err = res.unwrap_err().to_string();
    assert!(err.contains("revoked"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn unwritable_data_dir_fails_before_connecting() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("ro");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(data.join("x"), b"x").is_ok() {
        return; // running as root
    }
    let err = fleetops_agent::run(config(&srv.base, &data, &docker, Some(token))).await.unwrap_err();
    assert!(format!("{err:#}").contains("must be writable"), "{err:#}");
    assert!(!store.get_agent(env_id).await.unwrap().unwrap().enrolled, "the join token was not spent");
}
```

`fleetops_api::auth_token_hash` doesn't exist. Re-export the hash function from the api crate root with `pub use auth::token::hash_token as auth_token_hash;`, or compute it in the test with `sha2` + `hex` added as dev-dependencies. Prefer the re-export, which keeps a single definition. Add `fleetops-core` and `fleetops-tunnel` to `[dev-dependencies]`.

- [ ] **Step 8: Run the agent tests**

Run: `cargo test -p fleetops-agent`
Expected: PASS (3 unit + 4 integration). The restart test can take up to about 3 s while the agent backs off.

- [ ] **Step 9: Measure agent idle RSS**

Run:

```sh
cargo build --release -p fleetops-agent -p fleetops-server
```

Then start `target/release/fleetops` on loopback, create an agent environment through the API, run `target/release/fleetops-agent --server http://127.0.0.1:<port> --join-token … --data $(mktemp -d)`, wait 10 s, and run `ps -o rss= -p <agent pid>`.
Expected: ≤ 15360 KiB. Note the figure in the commit message. Task 12 automates this.

- [ ] **Step 10: Lint and commit**

Run: `make lint && make test`

```bash
git add Cargo.toml Cargo.lock crates/fleetops-agent crates/fleetops-api/src/lib.rs
git commit -m "feat(agent): fleetops-agent — enrollment, key storage, dial-out with backoff, Docker forwarding"
```

---

## Phase C — Product surface

### Task 10: web UI for agent environments

**Files:**
- Create: `web/src/pages/admin/AgentEnrollment.tsx`
- Modify: `web/src/pages/admin/EnvironmentsPage.tsx`, `web/src/pages/admin/admin.test.tsx`
- Test: `web/src/pages/admin/admin.test.tsx`

**Interfaces:**
- Consumes: the generated `createEnv` (returns `CreatedEnv`), `rotateAgent` (returns `Enrollment`), `listEnvs` (returns `EnvView[]`), and the types `EnvView`, `Enrollment` and `AgentView` from `@/api/gen/types.gen`
- Produces: `export function AgentEnrollment({ enrollment, onClose }: { enrollment: Enrollment; onClose: () => void })`

Before writing code, read `web/src/pages/admin/EnvironmentsPage.tsx`, `FormDialog.tsx`, `admin.test.tsx`, `web/src/components/Shell.tsx` (`EnvStatusBadge`) and `DESIGN.md`. Match their component and test idioms exactly. The steps below give the behaviour and the test code. Fit the JSX to the existing `FormDialog` / `Dialog` / `Button` components.

- [ ] **Step 1: Write the failing tests**

Add to `web/src/pages/admin/admin.test.tsx`. Follow the file's existing mocking of `@/api/gen/sdk.gen`; the snippet shows the assertions, so adapt the render and mock helpers to the ones the file already uses:

```tsx
describe("agent environments", () => {
  it("adds an agent environment and shows its one-time command", async () => {
    mockSdk.createEnv.mockResolvedValue({
      data: {
        id: 2, name: "web-01", kind: "docker-agent", url: "agent://2", created_at: 0, updated_at: 0,
        enrollment: { join_token: "fj_abc", expires_at: Math.floor(Date.now() / 1000) + 900, command: "docker run … --join-token fj_abc" },
      },
    });
    renderEnvironments([]);
    await user.click(screen.getByRole("button", { name: "Add environment" }));
    await user.click(screen.getByRole("radio", { name: /agent/i }));
    expect(screen.queryByLabelText("Docker URL")).toBeNull();
    await user.type(screen.getByLabelText("Name"), "web-01");
    await user.click(screen.getByRole("button", { name: "Add" }));
    expect(mockSdk.createEnv).toHaveBeenCalledWith({ body: { name: "web-01", kind: "docker-agent" } });
    expect(await screen.findByText(/--join-token fj_abc/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /copy/i })).toBeInTheDocument();
    expect(screen.getByText(/expires in 1[45] min/i)).toBeInTheDocument();
  });

  it("shows agent state and offers rotate only for agents", async () => {
    renderEnvironments([
      { id: 1, name: "local", kind: "docker-local", url: "unix:///var/run/docker.sock", created_at: 0, updated_at: 0, agent: null },
      { id: 2, name: "web-01", kind: "docker-agent", url: "agent://2", created_at: 0, updated_at: 0,
        agent: { connected: false, enrolled: true, last_seen_at: 1_800_000_000, agent_version: "0.2.0", host: { hostname: "web-01" }, last_error: null, join_expires_at: null } },
    ]);
    const rows = await screen.findAllByRole("row");
    expect(within(rows[2]).getByText("0.2.0")).toBeInTheDocument();
    expect(within(rows[2]).getByRole("button", { name: "Rotate key" })).toBeInTheDocument();
    expect(within(rows[1]).queryByRole("button", { name: "Rotate key" })).toBeNull();
  });

  it("rotating asks first, then shows the new command", async () => {
    mockSdk.rotateAgent.mockResolvedValue({ data: { join_token: "fj_new", expires_at: Math.floor(Date.now() / 1000) + 900, command: "docker run … --join-token fj_new" } });
    renderEnvironments([{ id: 2, name: "web-01", kind: "docker-agent", url: "agent://2", created_at: 0, updated_at: 0,
      agent: { connected: true, enrolled: true, last_seen_at: null, agent_version: "0.2.0", host: null, last_error: null, join_expires_at: null } }]);
    await user.click(await screen.findByRole("button", { name: "Rotate key" }));
    expect(screen.getByText(/disconnects the running agent/i)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Rotate" }));
    expect(mockSdk.rotateAgent).toHaveBeenCalledWith({ path: { env: "web-01" } });
    expect(await screen.findByText(/--join-token fj_new/)).toBeInTheDocument();
  });

  it("an agent's URL is not editable", async () => {
    renderEnvironments([{ id: 2, name: "web-01", kind: "docker-agent", url: "agent://2", created_at: 0, updated_at: 0, agent: null }]);
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    expect(screen.queryByLabelText("Docker URL")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm -C web test -- admin`
Expected: FAIL (no "Agent" radio, no "Rotate key").

- [ ] **Step 3: Implement**

`AgentEnrollment.tsx` is a dialog titled "Run this on the remote host". It contains:
- the command in a `<pre className="font-mono">` block, with a **Copy** button (`navigator.clipboard.writeText`, then the `sonner` toast "Copied");
- the line "Expires in N min, works once", where N = `Math.ceil((expires_at - now) / 60)` and updates every 30 s;
- a note: "The agent shows as online here within a few seconds of starting. Its key is stored in the fleetops-agent volume.";
- a **Done** button that calls `onClose`.

The command is not stored anywhere after close.

`EnvironmentsPage.tsx`:
1. Row type `EnvView` replaces `EnvironmentRecord` throughout.
2. `EnvForm` (add mode) gets a radio group "Kind": **Local socket** (default) or **Agent**. With Agent: hide the Docker URL field, submit `{ name, kind: "docker-agent" }`, and on success close the form and open `AgentEnrollment` with `data.enrollment`. In edit mode for an agent environment, hide the URL field.
3. New column **Agent**, after Status: empty for local; for agents, `agent_version` and, when offline, "last seen {relative time}" (use the relative-time helper the dashboard already uses, found under `web/src/lib`); `last_error` in the destructive text colour when set; "waiting for agent" when `!enrolled`.
4. A **Rotate key** button in the actions cell, for agent rows only. It opens the existing `ConfirmDialog` with the title `Rotate key for ${name}?`, the body "This disconnects the running agent. Run the new command on the host to reconnect it.", and the confirm label "Rotate". On confirm it calls `rotateAgent({ path: { env: name } })` and shows `AgentEnrollment` with the result.
5. After create, rotate and delete, call the existing `refresh()`.

- [ ] **Step 4: Run the web checks**

Run: `make test-web`
Expected: PASS (typecheck, lint, all vitest suites, API drift check).

- [ ] **Step 5: Look at it**

Run `make dev` and open http://localhost:5173/admin/environments. Add an agent environment, copy the command, check light and dark themes and a 390 px wide viewport (the command must wrap or scroll inside its block, never the page), and rotate. Take screenshots with the Playwright MCP tools if available.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): add agent environments, show the one-time command, agent state and key rotation"
```

---

### Task 11: agent image, CI and release

**Files:**
- Modify: `Dockerfile`, `Makefile`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- Create: `scripts/agent-smoke.sh`

**Interfaces:**
- Produces: Dockerfile targets `agent` (distroless image, entrypoint `/fleetops-agent`, `VOLUME /data`) and `agent-binary` (`FROM scratch`, `/fleetops-agent`); `make image-agent` (`AGENT_IMAGE ?= fleetops-agent:dev`); `make agent-smoke`

- [ ] **Step 1: Build both binaries in the build stage**

In the `Dockerfile` build stage, change the cargo command to `cargo zigbuild --locked --release -p fleetops-server -p fleetops-agent --target "$target"` and add `cp "target/$target/release/fleetops-agent" /fleetops-agent` after the existing `cp`. Then add, after the existing `binary` stage:

```dockerfile
# The bare agent binary, for release downloads.
FROM scratch AS agent-binary
COPY --from=build /fleetops-agent /fleetops-agent

# The agent image: dials out to a FleetOps server and serves this host's
# Docker socket. Build it with `make image-agent`.
FROM gcr.io/distroless/static-debian13:nonroot AS agent
COPY --from=build /fleetops-agent /fleetops-agent
COPY --from=build --chown=65532:65532 /data /data
VOLUME /data
USER nonroot
ENTRYPOINT ["/fleetops-agent"]
```

Move the server's final stage so it **stays last** (the default target), and give it a name: `FROM gcr.io/distroless/static-debian13:nonroot AS server`.

- [ ] **Step 2: Makefile targets**

```make
AGENT_IMAGE ?= fleetops-agent:dev

# The agent image, same build as the server's.
image-agent:
	docker buildx build --platform $(PLATFORMS) --target agent --tag $(AGENT_IMAGE) --load .

# Runs $(IMAGE) and $(AGENT_IMAGE) together and checks that the agent
# enrolls and its environment reaches Docker.
agent-smoke:
	scripts/agent-smoke.sh $(IMAGE) $(AGENT_IMAGE)
```

Add `image-agent agent-smoke` to `.PHONY`.

- [ ] **Step 3: The smoke script**

`scripts/agent-smoke.sh`:

```sh
#!/bin/sh
# Smoke-tests the server and agent images together on the local daemon:
# the server runs without the Docker socket, the agent runs with it, and
# the agent environment must come up and list containers.
#
# Usage: scripts/agent-smoke.sh [server image] [agent image]
set -eu

server_image=${1:-fleetops:dev}
agent_image=${2:-fleetops-agent:dev}
net=fleetops-smoke-net-$$
srv=fleetops-smoke-srv-$$
agt=fleetops-smoke-agent-$$
jar=$(mktemp)
trap 'docker rm -f "$srv" "$agt" >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; rm -f "$jar"' EXIT

fail() {
  echo "agent-smoke: $*" >&2
  docker logs "$srv" >&2 2>&1 || true
  docker logs "$agt" >&2 2>&1 || true
  exit 1
}

docker network create "$net" >/dev/null
docker run -d --name "$srv" --network "$net" -p 127.0.0.1::9000 "$server_image" >/dev/null
base="http://$(docker port "$srv" 9000/tcp | head -n1)"
i=0
until curl -fsS "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "server did not start"; sleep 1
done

curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" >/dev/null || fail "setup"
csrf=$(curl -fsS -b "$jar" "$base/api/v1/auth/me" | sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
token=$(curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
  -d '{"name":"remote","kind":"docker-agent"}' "$base/api/v1/envs" | sed -n 's/.*"join_token":"\([^"]*\)".*/\1/p')
[ -n "$token" ] || fail "no join token"

# The agent reaches the server over the private network. That is plain
# HTTP from a non-loopback address, so the server must refuse it...
gid=$(docker run --rm -v /var/run/docker.sock:/docker.sock busybox:1.37 stat -c %g /docker.sock)
docker run -d --name "$agt" --network "$net" -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" \
  "$agent_image" --server "http://$srv:9000" --join-token "$token" >/dev/null
sleep 3
docker logs "$agt" 2>&1 | grep -q "403" || fail "plain HTTP from another host was not refused"
docker rm -f "$agt" >/dev/null

# ...so run it in the server's network namespace, where it is loopback.
docker run -d --name "$agt" --network "container:$srv" -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" \
  "$agent_image" --server "http://127.0.0.1:9000" --join-token "$token" >/dev/null
i=0
until curl -fsS -b "$jar" "$base/api/v1/envs/remote/status" | grep -q '"state":"up"'; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "agent environment never came up"; sleep 1
done
curl -fsS -b "$jar" "$base/api/v1/envs/remote/containers" | grep -q "$srv" || fail "remote containers list is missing the server container"
echo "agent-smoke: ok ($base)"
```

The `/auth/me` response's CSRF field name and header name come from the existing auth code (`crates/fleetops-api/src/auth/`). Check both, and fix the `sed` and the header in the script to match. `scripts/image-smoke.sh` shows the setup call.

Run: `chmod +x scripts/agent-smoke.sh && make image PLATFORMS=linux/arm64 && make image-agent PLATFORMS=linux/arm64 && make agent-smoke` (use the local platform).
Expected: `agent-smoke: ok`.

- [ ] **Step 4: CI and release**

`.github/workflows/ci.yml`, in the `image` job: add `--target server` to the existing build steps (`target: server`), add a step that loads the agent (`target: agent`, `tags: fleetops-agent:ci`, `load: true`, `platforms: linux/amd64`, `cache-from: type=gha`), and add `- run: make agent-smoke IMAGE=fleetops:ci AGENT_IMAGE=fleetops-agent:ci`.

`.github/workflows/release.yml`:
- add `target: server` to the existing image push;
- add a second metadata step (`id: meta-agent`, `images: ghcr.io/${{ github.repository_owner }}/fleetops-agent`, the same tag rules) and a push step with `target: agent`;
- change the binary export step to also export `agent-binary` into `out-agent`;
- package `fleetops-agent-$GITHUB_REF_NAME-linux-$arch` with a `.sha256` file, the same way as the server binary.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile Makefile scripts/agent-smoke.sh .github/workflows
git commit -m "build: agent image and binaries, an agent smoke test in CI, and agent release artifacts"
```

---

### Task 12: end-to-end test, memory gates, and docs

**Files:**
- Create: `web/e2e/agent.spec.ts`
- Modify: `web/playwright.config.ts` (if it needs the agent binary path), `Makefile` (`e2e` builds the agent and passes `FLEETOPS_AGENT_BIN`), `scripts/rss-bench.sh`, `README.md`, `docs/superpowers/specs/2026-10-01-fleetops-agent-multihost-design.md` (status line)

- [ ] **Step 1: Write the e2e spec**

Read `web/e2e/core-flow.spec.ts` and `web/playwright.config.ts` first, and reuse their server start-up and login helpers. The agent spec:

```ts
import { test, expect } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Runs after core-flow: the admin exists and the server listens on FLEETOPS_E2E_BASE.
test("an agent environment comes online, serves logs, and goes offline", async ({ page, baseURL }) => {
  await page.goto("/admin/environments");
  await page.getByRole("button", { name: "Add environment" }).click();
  await page.getByRole("radio", { name: /agent/i }).click();
  await page.getByLabel("Name").fill("e2e-remote");
  await page.getByRole("button", { name: "Add" }).click();
  const command = await page.locator("pre").innerText();
  const token = /--join-token (fj_[0-9a-f]+)/.exec(command)![1];
  await page.getByRole("button", { name: "Done" }).click();

  const agent: ChildProcess = spawn(process.env.FLEETOPS_AGENT_BIN!, [
    "--server", baseURL!, "--join-token", token,
    "--data", mkdtempSync(join(tmpdir(), "fleetops-agent-")),
    "--docker", process.env.DOCKER_HOST ?? "unix:///var/run/docker.sock",
  ], { stdio: "inherit" });
  try {
    const row = page.getByRole("row", { name: /e2e-remote/ });
    await expect(row.getByText(/online|up/i)).toBeVisible({ timeout: 15_000 });

    await page.goto("/envs/e2e-remote/containers");
    await expect(page.getByRole("table")).toBeVisible();
    const first = page.getByRole("row").nth(1).getByRole("link").first();
    await first.click();
    await page.getByRole("tab", { name: "Logs" }).click();
    await expect(page.getByTestId("log-viewer")).toBeVisible();
  } finally {
    agent.kill("SIGTERM");
  }
  await page.goto("/admin/environments");
  await expect(page.getByRole("row", { name: /e2e-remote/ }).getByText(/offline|down/i)).toBeVisible({ timeout: 60_000 });
});
```

Adapt the selectors (`log-viewer` test id, the status text the badge shows, the container link) to what `core-flow.spec.ts` already uses. The e2e daemon has at least the `fleetops-e2e-*` container while core-flow runs, but core-flow removes it, so start a `busybox:1.37 sleep 300` container labelled `fleetops.e2e=1` in a `beforeAll` (and remove it in `afterAll`) so the list is never empty.

In the `Makefile` `e2e` target, build the agent too (`cargo build --release -p fleetops-agent` after `build`) and export `FLEETOPS_AGENT_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops-agent`.

Run: `make e2e`
Expected: both specs PASS.

- [ ] **Step 2: Extend rss-bench with agents**

In `scripts/rss-bench.sh`, after the existing single-server measurement, add an agent phase controlled by `AGENTS` (default 10):
- create `$AGENTS` agent environments through the API, the same way as `agent-smoke.sh` (with `Origin` and the CSRF header);
- start one `target/release/fleetops-agent` per environment against `--server $base` (loopback, so plain HTTP is accepted), each with its own `mktemp -d` data directory and `--docker` set to the daemon;
- wait until `GET /api/v1/envs` shows `"connected":true` `$AGENTS` times, then `sleep "$settle"`;
- sample the server's RSS (as before) and the first agent's RSS;
- fail if the server exceeds `(LIMIT_MIB + AGENTS) * 1024` KiB, or the agent exceeds `AGENT_LIMIT_MIB` (default 15) * 1024 KiB;
- print `rss-bench: server ${mib} MiB with $AGENTS agents (limit …); agent ${amib} MiB (limit …)`.

Kill the agents in `cleanup`. In the Makefile, `rss-bench` depends on building `fleetops-agent` too.

Run: `make rss-bench`
Expected: both gates pass. Put the numbers in the commit message.

- [ ] **Step 3: README**

Update `README.md`:
- **Status:** sub-project 2 complete; next is sub-project 3.
- **Features → Available now:** a new "Remote hosts" group covering the dial-out agent, the one-time join command, key rotation, live online/offline state, and that every page works on remote hosts.
- **Quick start → Add a remote host:** add the agent in Admin → Environments, run the command on the host, and note that the server must be reachable over HTTPS (`--tls-bind`), or the agent must run on the same host for plain HTTP. For a self-signed certificate, mount it and pass `--ca /ca.pem`.
- **Security:** a compromised server is root on every agent host, just as it already is on its local socket. Agent credentials grant tunnel access only. Behind a reverse proxy, enable WebSocket pass-through, and keep idle timeouts above 30 s (the server pings every 15 s).
- **Architecture:** add the `fleetops-tunnel` and `fleetops-agent` crates to the tree, plus a paragraph on the tunnel (yamux over WebSocket, server-opened byte streams, bollard custom transport).
- **Development:** the `make` table gains `image-agent` and `agent-smoke`; `test-docker` also runs through the tunnel.
- **Roadmap:** sub-project 2 marked done.

Set the spec's `**Status:**` to `Implemented`.

- [ ] **Step 4: Full verification**

Run: `make lint && make test && make test-docker && make test-web && make e2e && make rss-bench`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add web/e2e Makefile scripts/rss-bench.sh README.md docs/superpowers/specs/2026-10-01-fleetops-agent-multihost-design.md
git commit -m "test: agent end-to-end flow and memory gates; docs for remote hosts"
```
