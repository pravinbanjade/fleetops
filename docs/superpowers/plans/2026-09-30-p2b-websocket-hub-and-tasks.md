# FleetOps P2b — Real-Time Websocket Hub & Background Tasks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the browser one websocket per tab, at `/api/v1/ws`, that streams Docker events, container stats, logs and an interactive terminal. Image pull and build run as background tasks that report progress over the same socket.

**Architecture:**
- `fleetops-core` gains stream types and a `StreamOps` trait. The bollard implementation lives in `fleetops-docker`. `DockerOps` then includes `StreamOps`, so handlers still see only `Arc<dyn DockerOps>`.
- `fleetops-api::ws` has four parts:
  - `protocol`: the wire format.
  - `queue`: a bounded outbound queue per subscription, with a drop, gap or block policy.
  - `hub`: one shared Docker events stream per environment, with reconnect and `env_status`, plus ref-counted stats streams per container.
  - `conn` and `topics`: a reader/writer pair per connection, and one producer per subscription.
- `fleetops-api::tasks` runs pull and build jobs in the background. It keeps a progress backlog for late subscribers, and audits each job when it finishes.

**Tech Stack:** Rust ≥1.86, axum 0.8 (`ws` and `multipart` features), bollard 0.21, tokio broadcast/mpsc/Notify, futures-util 0.3, and tokio-tungstenite 0.29 for tests only.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md` (§4.3 Real-time UX, §5.1 image pull/build, §5.2 Websocket)

**Plan series for sub-project 1:**
- P1 (done): backend foundation.
- P2a (done): environments and REST resources.
- **P2b (this plan):** websocket hub and background tasks.
- **P2c:** utoipa OpenAPI for every handler, served at `/api/v1/openapi.json`.
- P3: web UI, e2e tests, image build, CI.

## Global Constraints

- Handlers and websocket topics never import bollard. They use `fleetops_core` traits through `Arc<dyn DockerOps>`.
- Wire format (spec §5.2):
  - Frames are JSON text. Exec I/O uses binary frames laid out as `[u32 sub id, big-endian][payload]`.
  - Client ops are `sub` / `unsub` / `input` / `resize`.
  - Server messages have `type` set to `data` / `gap` / `end` / `error`, or they are `{type:"env_status", env, status}`.
- Batching: logs are flushed at most every 50 ms per subscription (`LOG_FLUSH`), with at most 500 lines per message.
- Backpressure: every subscription has its own bounded queue (default 1024, `ApiConfig.ws_queue_capacity`).
  - `events`, `stats` and `task` drop the oldest message.
  - `logs` drops new lines and reports them with `{type:"gap", dropped:n}`.
  - `exec` blocks its producer, so Docker's output is read only as fast as the client takes it.
  - A slow client never blocks other subscriptions or other clients.
- Fan-out: one Docker events stream per environment instance, shared through `tokio::sync::broadcast`. One stats stream per container, shared by all subscribers and dropped when the last one leaves.
- Every `sub` is checked with `authorize()`: `DockerRead` for events, stats and logs, and `DockerWrite` for exec. A task is visible only to its owner and to admins. Every exec attempt is audited as `container.exec`, with the command.
- Resilience: when a Docker events stream drops, the hub reconnects with exponential backoff (initial `ApiConfig.ws_backoff`, default 1 s, doubling up to 30 s). It broadcasts `env_status` on every up/down transition.
- Security:
  - The websocket needs a valid session cookie.
  - An `Origin` header, when present, must match `Host`.
  - The session is re-checked every `ApiConfig.ws_session_check` (default 30 s). Logout, revocation or user deletion closes the socket, and an open socket keeps its session from idling out.
- Image pull and build answer `202 {task_id}`. Progress arrives on the websocket `task` topic, and `GET /tasks/{id}` returns the task's current state.
- All P1 and P2a constraints still apply to new REST routes: CSRF on mutations, the JSON error body, request ids and audit rows with `environment_id`.
- **Spec deltas (intentional):**
  - `env` is optional for the `task` topic, since tasks have their own ids.
  - `events` accepts an optional `id`, which keeps only events whose actor id or name matches.
  - `end` may carry extra fields: `exit_code` for exec, and `task` with the final `TaskInfo`.
  - `error.code` is the core error code (`invalid`, `not_found`, `env_unavailable`, `upstream`, …).
  - `GET /tasks/{id}` is added.
  - Stats are not batched, because Docker already sends about one sample per second.
  - Limits: 256 subscriptions per connection, 1 MiB per client frame, 8 running tasks per server, 100 MiB per build upload.
  - OpenAPI moves to P2c.

## Review Focus

1. **A slow or stalled browser tab** (laptop asleep, bad Wi-Fi, a huge log). Other subscriptions and other clients keep flowing. The logs subscription reports `gap` instead of stalling or buffering without bound, and exec pauses Docker instead of queueing. Pinned in Task 7 (`slow_log_reader_gets_a_gap_instead_of_stalling`) and Task 8 (`exec_output_waits_for_a_slow_client`, `other_connections_keep_flowing_while_one_is_slow`).
2. **Docker restarts while the UI is open.** Every open tab gets `env_status` down and then up, and the same `events` subscription resumes without the client doing anything. Pinned in Task 5 (`dropped_event_stream_reports_down_then_up_and_resumes`) and P2a's `daemon_loss_mid_session_is_unavailable_then_recovers`.
3. **A session revoked, logged out or deleted while its websocket stays open.** The socket gets an `unauthorized` error and closes within one session-check interval, so it cannot keep streaming or running an exec. Pinned in Task 5 (`logging_out_closes_open_websockets`, `deleted_user_loses_the_websocket`).
4. **Many subscribers to the same thing** (a list page with many tabs open). Docker sees one events stream per environment and one stats stream per container, released when the last subscriber leaves. Pinned in Task 5 (`events_fan_out_from_one_shared_stream_and_filter_by_id`) and Task 6 (`stats_streams_are_shared_per_container_and_released`).
5. **An environment deleted and re-created, or its URL changed, while clients are subscribed.** New subscriptions get a fresh Docker connection to the new endpoint, never the old one. Pinned in Task 5 (`recreated_environment_gets_a_fresh_docker_stream`).

---

## File Structure

```
Cargo.toml                                        (modify) axum features ws+multipart; tokio-tungstenite (dev)
Makefile                                          (modify) test-docker also runs the server's live test
crates/fleetops-core/
  Cargo.toml                                      (modify) + futures-util; tokio becomes a normal dependency
  src/streams.rs                                  stream types, StreamOps, LineSplitter, stats maths
  src/docker.rs                                   (modify) split_image_reference; DockerOps includes StreamOps
  tests/streams.rs  tests/docker_types.rs         (new / extend)
crates/fleetops-docker/
  src/streams.rs                                  impl StreamOps for LocalDocker
  src/error.rs                                    (modify) DockerStreamError → Upstream
  tests/docker_it.rs                              (extend) `streams` module against the real daemon
crates/fleetops-api/
  Cargo.toml                                      (modify) + futures-util; tokio-tungstenite (dev)
  src/state.rs                                    (modify) hubs, tasks, ws_* config
  src/tasks.rs                                    TaskManager: background jobs with backlog and audit hook
  src/ws/mod.rs                                   /ws handler, Origin check
  src/ws/protocol.rs                              ClientMsg, Out, message builders, binary frames
  src/ws/queue.rs                                 SubQueue with DropOldest / Gap / Block
  src/ws/hub.rs                                   EventHub (reconnect, env_status), StatsHub, Hubs
  src/ws/conn.rs                                  connection reader/writer, subscriptions, session re-check
  src/ws/topics.rs                                events / stats / logs / exec / task producers
  src/routes/mod.rs                               (modify) /ws, /tasks/{id}, images/pull, images/build
  src/routes/tasks.rs                             GET /tasks/{id}
  src/routes/docker/image_tasks.rs                POST images/pull, POST images/build (multipart)
  tests/common/fake_docker.rs                     (extend) hand-driven fake streams
  tests/common/mod.rs                             (extend) TestApp::serve, Ws client, ws_sub, ws_config
  tests/ws.rs tests/ws_events.rs tests/ws_stats.rs tests/ws_logs.rs tests/ws_exec.rs tests/tasks.rs
crates/fleetops-server/
  Cargo.toml                                      (modify) docker-it feature; dev deps
  tests/live.rs                                   every topic end to end against the real daemon
```

---

### Task 1: Core stream types and the `StreamOps` trait

**Files:**
- Create: `crates/fleetops-core/src/streams.rs`
- Modify: `crates/fleetops-core/Cargo.toml`, `crates/fleetops-core/src/lib.rs`, `crates/fleetops-core/src/docker.rs`
- Test: `crates/fleetops-core/tests/streams.rs`, `crates/fleetops-core/tests/docker_types.rs` (extend)

**Interfaces:**
- Consumes: `fleetops_core::{Error, Result}`.
- Produces (in `fleetops_core::streams`):
  - `BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T>> + Send>>`.
  - Data types:
    - `DockerEvent{kind, action, id, name, attributes, time}`
    - `StatsSample{time (ms), cpu_percent, memory_used, memory_limit, net_rx, net_tx, block_read, block_write, pids}`
    - `LogStream{Stdout,Stderr}` and `LogLine{stream, text}`
  - Options:
    - `LogOptions{follow=true, tail=Some(1000), since, timestamps, stdout=true, stderr=true}`
    - `ExecOptions{cmd, user, cols=80, rows=24}`, with `command()`, which falls back to a bash-then-sh probe.
  - `ExecSession{exec_id, output: BoxStream<Vec<u8>>, input: mpsc::Sender<Vec<u8>>}`.
  - Build inputs: `BuildContext{Tar(Vec<u8>), Dockerfile(Vec<u8>)}` and `BuildSpec{context, tag, dockerfile}`.
  - `trait StreamOps`:
    - `events()`, `stats(id)`, `logs(id, &LogOptions)`
    - `exec_start(id, &ExecOptions)`, `exec_resize(exec_id, cols, rows)`, `exec_exit_code(exec_id)`
    - `pull_image(image, tag)` and `build_image(BuildSpec)`, which return Docker's messages as `serde_json::Value`
  - Helpers: `LineSplitter{push, finish}`, `MAX_LINE_BYTES`, `cpu_percent(&Value)` and `stats_sample(&Value, time_ms)`.
- Produces (in `fleetops_core::docker`): `split_image_reference(&str) -> Result<(repo, tag)>`.
  - The tag defaults to `latest`.
  - A colon before the last `/` is a registry port, not a tag.
  - A digest reference comes back whole, with an empty tag.

- [ ] **Step 1: Update the manifest**

In `crates/fleetops-core/Cargo.toml`, add `futures-util` and move `tokio` from dev-dependencies to dependencies. `ExecSession` carries a tokio `mpsc::Sender`.

```diff
--- a/crates/fleetops-core/Cargo.toml
+++ b/crates/fleetops-core/Cargo.toml
@@ -6,9 +6,8 @@ rust-version.workspace = true
 
 [dependencies]
 async-trait.workspace = true
+futures-util.workspace = true
 serde_json.workspace = true
 serde.workspace = true
 thiserror.workspace = true
-
-[dev-dependencies]
 tokio.workspace = true
```

- [ ] **Step 2: Write the failing tests**

`crates/fleetops-core/tests/streams.rs`:

```rust
use fleetops_core::streams::{
    cpu_percent, stats_sample, ExecOptions, LineSplitter, LogOptions, LogStream, MAX_LINE_BYTES,
};
use serde_json::json;

#[test]
fn splitter_joins_chunks_into_lines_per_stream() {
    let mut s = LineSplitter::default();
    assert!(s.push(LogStream::Stdout, b"hel").is_empty());
    let out = s.push(LogStream::Stderr, b"err\n");
    assert_eq!(out.len(), 1);
    assert_eq!(
        (out[0].stream, out[0].text.as_str()),
        (LogStream::Stderr, "err")
    );
    let out = s.push(LogStream::Stdout, b"lo\r\nworld\npart");
    let texts: Vec<_> = out.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["hello", "world"]);
    let rest = s.finish();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].text, "part");
    assert!(s.finish().is_empty());
}

#[test]
fn splitter_bounds_a_line_without_newline() {
    let mut s = LineSplitter::default();
    let out = s.push(LogStream::Stdout, &vec![b'x'; MAX_LINE_BYTES + 1]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text.len(), MAX_LINE_BYTES + 1);
    assert!(s.finish().is_empty());
}

#[test]
fn splitter_replaces_invalid_utf8() {
    let mut s = LineSplitter::default();
    let out = s.push(LogStream::Stdout, b"a\xffb\n");
    assert_eq!(out[0].text, "a\u{fffd}b");
}

#[test]
fn cpu_percent_matches_docker_stats() {
    let v = json!({
        "cpu_stats": {"cpu_usage": {"total_usage": 300}, "system_cpu_usage": 2000, "online_cpus": 4},
        "precpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 1000}
    });
    assert!((cpu_percent(&v) - 80.0).abs() < 1e-9);
    // First sample has no previous reading.
    assert_eq!(
        cpu_percent(&json!({"cpu_stats": {}, "precpu_stats": {}})),
        0.0
    );
}

#[test]
fn stats_sample_sums_networks_and_block_io_and_excludes_cache() {
    let v = json!({
        "memory_stats": {"usage": 1000, "limit": 4000, "stats": {"inactive_file": 200}},
        "networks": {"eth0": {"rx_bytes": 10, "tx_bytes": 20}, "eth1": {"rx_bytes": 1, "tx_bytes": 2}},
        "blkio_stats": {"io_service_bytes_recursive": [
            {"op": "read", "value": 5}, {"op": "Write", "value": 7}, {"op": "total", "value": 12}
        ]},
        "pids_stats": {"current": 3}
    });
    let s = stats_sample(&v, 42);
    assert_eq!(
        (
            s.time,
            s.memory_used,
            s.memory_limit,
            s.net_rx,
            s.net_tx,
            s.block_read,
            s.block_write,
            s.pids
        ),
        (42, 800, 4000, 11, 22, 5, 7, 3)
    );
}

#[test]
fn log_options_default_to_following_the_last_1000_lines() {
    let o: LogOptions = serde_json::from_value(json!({})).unwrap();
    assert_eq!(o, LogOptions::default());
    assert!(o.follow && o.stdout && o.stderr && !o.timestamps);
    assert_eq!(o.tail, Some(1000));
    let all: LogOptions = serde_json::from_value(json!({"tail": null, "follow": false})).unwrap();
    assert_eq!((all.tail, all.follow), (None, false));
}

#[test]
fn exec_defaults_to_a_bash_then_sh_probe() {
    let o = ExecOptions::default();
    assert_eq!((o.cols, o.rows), (80, 24));
    let cmd = o.command();
    assert_eq!(cmd[0], "/bin/sh");
    assert!(cmd[2].contains("bash") && cmd[2].contains("exec sh"));
    let custom = ExecOptions {
        cmd: Some(vec!["ls".into()]),
        ..Default::default()
    };
    assert_eq!(custom.command(), ["ls"]);
    let empty = ExecOptions {
        cmd: Some(vec![]),
        ..Default::default()
    };
    assert_eq!(empty.command(), o.command());
}
```

Append to `crates/fleetops-core/tests/docker_types.rs`:

```diff
--- a/crates/fleetops-core/tests/docker_types.rs
+++ b/crates/fleetops-core/tests/docker_types.rs
@@ -148,3 +148,25 @@ fn env_name_and_url_rules() {
         assert!(validate_env_url(bad).is_err(), "{bad}");
     }
 }
+
+#[test]
+fn image_references_split_into_repo_and_tag() {
+    use fleetops_core::docker::split_image_reference as split;
+    let ok = |r: &str, repo: &str, tag: &str| {
+        assert_eq!(
+            split(r).unwrap(),
+            (repo.to_string(), tag.to_string()),
+            "{r}"
+        )
+    };
+    ok("nginx", "nginx", "latest");
+    ok("nginx:1.27", "nginx", "1.27");
+    ok(" alpine:3.20 ", "alpine", "3.20");
+    ok("ghcr.io/org/app:2", "ghcr.io/org/app", "2");
+    ok("registry:5000/team/app", "registry:5000/team/app", "latest");
+    ok("registry:5000/team/app:v1", "registry:5000/team/app", "v1");
+    ok("alpine@sha256:abc", "alpine@sha256:abc", "");
+    for bad in ["", "  ", "nginx:", ":1", "has space:1", "a\nb"] {
+        assert!(matches!(split(bad), Err(Error::Invalid(_))), "{bad:?}");
+    }
+}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p fleetops-core --test streams --test docker_types`
Expected: FAIL to compile with `unresolved import fleetops_core::streams` and `cannot find function split_image_reference`.

- [ ] **Step 4: Implement**

Add `pub mod streams;` to `crates/fleetops-core/src/lib.rs`, after `pub mod recreate;`.

`crates/fleetops-core/src/streams.rs`:

```rust
//! Long-lived Docker streams: events, stats, logs, exec, and image
//! pull/build progress. Implementations return `'static` boxed streams so
//! the websocket layer can own them independently of the request.

use crate::Result;
use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, pin::Pin};

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T>> + Send>>;

/// One Docker event, reduced to what the UI needs to patch its caches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerEvent {
    /// container | image | volume | network | daemon | ...
    pub kind: String,
    /// create | start | die | destroy | pull | ...
    pub action: String,
    pub id: String,
    /// The actor's `name` attribute when Docker sends one.
    pub name: Option<String>,
    pub attributes: BTreeMap<String, String>,
    /// Unix seconds.
    pub time: i64,
}

/// One stats sample for a container, with CPU already turned into a
/// percentage of one core times the number of cores (as `docker stats`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSample {
    /// Unix milliseconds.
    pub time: i64,
    pub cpu_percent: f64,
    pub memory_used: u64,
    pub memory_limit: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub pids: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub stream: LogStream,
    /// One line without its trailing newline. With `timestamps`, Docker's
    /// RFC 3339 timestamp and a space come first.
    pub text: String,
}

pub const LOG_TAIL_DEFAULT: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LogOptions {
    pub follow: bool,
    /// Lines from the end to start with; `None` means the whole log.
    pub tail: Option<u32>,
    /// Unix seconds.
    pub since: Option<i64>,
    pub timestamps: bool,
    pub stdout: bool,
    pub stderr: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            follow: true,
            tail: Some(LOG_TAIL_DEFAULT),
            since: None,
            timestamps: false,
            stdout: true,
            stderr: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ExecOptions {
    /// `None` auto-detects a shell (bash, falling back to sh).
    pub cmd: Option<Vec<String>>,
    pub user: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            cmd: None,
            user: None,
            cols: 80,
            rows: 24,
        }
    }
}

impl ExecOptions {
    /// The command to run: the caller's, or a shell probe that prefers bash.
    pub fn command(&self) -> Vec<String> {
        match &self.cmd {
            Some(cmd) if !cmd.is_empty() => cmd.clone(),
            _ => vec![
                "/bin/sh".into(),
                "-c".into(),
                "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi".into(),
            ],
        }
    }
}

/// A running interactive exec with a TTY.
pub struct ExecSession {
    pub exec_id: String,
    /// Raw terminal output. Reading it slowly slows the process down; that
    /// is the backpressure path.
    pub output: BoxStream<Vec<u8>>,
    /// Terminal input. Dropping it closes the process's stdin.
    pub input: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// What `docker build` is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildContext {
    /// A tar archive of the build context.
    Tar(Vec<u8>),
    /// A lone Dockerfile; the context is otherwise empty.
    Dockerfile(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    pub context: BuildContext,
    /// `repo:tag` to apply to the result.
    pub tag: Option<String>,
    /// Dockerfile path inside the context; default `Dockerfile`.
    pub dockerfile: Option<String>,
}

#[async_trait]
pub trait StreamOps: Send + Sync {
    /// Docker events from now on. Ends when the daemon connection drops.
    fn events(&self) -> BoxStream<DockerEvent>;
    /// About one sample per second until the container stops.
    fn stats(&self, id: &str) -> BoxStream<StatsSample>;
    fn logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    async fn exec_start(&self, id: &str, opts: &ExecOptions) -> Result<ExecSession>;
    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<()>;
    /// `None` while the process is still running.
    async fn exec_exit_code(&self, exec_id: &str) -> Result<Option<i64>>;
    /// Docker's pull progress messages, verbatim.
    fn pull_image(&self, image: &str, tag: &str) -> BoxStream<Value>;
    /// Docker's build messages, verbatim.
    fn build_image(&self, spec: BuildSpec) -> BoxStream<Value>;
}

/// Splits Docker's framed output into whole lines, keeping a partial line
/// per stream until its newline arrives. TTY containers send arbitrary
/// chunks, and long lines arrive in 16 KiB pieces.
#[derive(Default)]
pub struct LineSplitter {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Longest partial line kept before it is emitted anyway.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

impl LineSplitter {
    pub fn push(&mut self, stream: LogStream, chunk: &[u8]) -> Vec<LogLine> {
        let buf = match stream {
            LogStream::Stdout => &mut self.stdout,
            LogStream::Stderr => &mut self.stderr,
        };
        buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(to_line(stream, &line));
        }
        if buf.len() > MAX_LINE_BYTES {
            out.push(to_line(stream, buf));
            buf.clear();
        }
        out
    }

    /// Whatever is left once the stream ends.
    pub fn finish(&mut self) -> Vec<LogLine> {
        let mut out = Vec::new();
        for (stream, buf) in [
            (LogStream::Stdout, &mut self.stdout),
            (LogStream::Stderr, &mut self.stderr),
        ] {
            if !buf.is_empty() {
                out.push(to_line(stream, buf));
                buf.clear();
            }
        }
        out
    }
}

fn to_line(stream: LogStream, bytes: &[u8]) -> LogLine {
    LogLine {
        stream,
        text: String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// `docker stats` CPU percentage from one stats JSON sample (which carries
/// the previous reading in `precpu_stats`).
pub fn cpu_percent(stats: &Value) -> f64 {
    let cpu = &stats["cpu_stats"];
    let pre = &stats["precpu_stats"];
    let total = |v: &Value| v["cpu_usage"]["total_usage"].as_f64().unwrap_or(0.0);
    let system = |v: &Value| v["system_cpu_usage"].as_f64().unwrap_or(0.0);
    let cpu_delta = total(cpu) - total(pre);
    let system_delta = system(cpu) - system(pre);
    let cpus = cpu["online_cpus"]
        .as_f64()
        .filter(|n| *n > 0.0)
        .or_else(|| {
            cpu["cpu_usage"]["percpu_usage"]
                .as_array()
                .map(|a| a.len() as f64)
        })
        .unwrap_or(1.0);
    if cpu_delta > 0.0 && system_delta > 0.0 {
        cpu_delta / system_delta * cpus * 100.0
    } else {
        0.0
    }
}

/// Converts Docker's stats JSON into a sample. Memory excludes page cache,
/// as `docker stats` does.
pub fn stats_sample(stats: &Value, time_ms: i64) -> StatsSample {
    let mem = &stats["memory_stats"];
    let usage = mem["usage"].as_u64().unwrap_or(0);
    let cache = mem["stats"]["inactive_file"]
        .as_u64()
        .or_else(|| mem["stats"]["total_inactive_file"].as_u64())
        .unwrap_or(0);
    let (mut rx, mut tx) = (0, 0);
    for net in stats["networks"].as_object().into_iter().flatten() {
        rx += net.1["rx_bytes"].as_u64().unwrap_or(0);
        tx += net.1["tx_bytes"].as_u64().unwrap_or(0);
    }
    let (mut read, mut write) = (0, 0);
    for io in stats["blkio_stats"]["io_service_bytes_recursive"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let v = io["value"].as_u64().unwrap_or(0);
        match io["op"].as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("read") => read += v,
            Some("write") => write += v,
            _ => {}
        }
    }
    StatsSample {
        time: time_ms,
        cpu_percent: cpu_percent(stats),
        memory_used: usage.saturating_sub(cache),
        memory_limit: mem["limit"].as_u64().unwrap_or(0),
        net_rx: rx,
        net_tx: tx,
        block_read: read,
        block_write: write,
        pids: stats["pids_stats"]["current"].as_u64().unwrap_or(0),
    }
}
```

In `crates/fleetops-core/src/docker.rs`, add `split_image_reference` just above `pub enum ContainerAction`:

```diff
--- a/crates/fleetops-core/src/docker.rs
+++ b/crates/fleetops-core/src/docker.rs
@@ -136,6 +136,31 @@ impl ContainerSpec {
     }
 }
 
+/// Splits `repo[:tag]` into repo and tag, defaulting the tag to `latest`.
+/// A colon before the last `/` is a registry port, not a tag. Digest
+/// references (`repo@sha256:...`) come back whole with an empty tag.
+pub fn split_image_reference(reference: &str) -> Result<(String, String)> {
+    let r = reference.trim();
+    if r.is_empty() || r.len() > 512 || r.chars().any(|c| c.is_whitespace() || c.is_control()) {
+        return Err(Error::Invalid(format!(
+            "invalid image reference '{reference}'"
+        )));
+    }
+    if r.contains('@') {
+        return Ok((r.to_string(), String::new()));
+    }
+    let slash = r.rfind('/').map_or(0, |i| i + 1);
+    match r[slash..].rfind(':') {
+        Some(i) if i > 0 && slash + i + 1 < r.len() => {
+            Ok((r[..slash + i].to_string(), r[slash + i + 1..].to_string()))
+        }
+        Some(_) => Err(Error::Invalid(format!(
+            "invalid image reference '{reference}'"
+        ))),
+        None => Ok((r.to_string(), "latest".into())),
+    }
+}
+
 #[derive(Debug, Clone, Copy, PartialEq, Eq)]
 pub enum ContainerAction {
     Start,
```

`DockerOps` does **not** include `StreamOps` yet. Task 5 adds it, once `LocalDocker` (Task 2) and the test fake (Task 5) both implement it, so every task leaves the workspace compiling.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-core && make lint`
Expected: `streams` 7 PASS, `docker_types` 6 PASS, all other core tests PASS, lint clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/fleetops-core
git commit -m "feat(core): stream types, StreamOps and image reference parsing"
```

---

### Task 2: bollard implementation of `StreamOps`

**Files:**
- Create: `crates/fleetops-docker/src/streams.rs`
- Modify: `crates/fleetops-docker/src/lib.rs`, `crates/fleetops-docker/src/error.rs`
- Test: unit tests in `streams.rs` and `error.rs`; `crates/fleetops-docker/tests/docker_it.rs` (extend)

**Interfaces:**
- Consumes: everything in `fleetops_core::streams` (Task 1), `LocalDocker` and `map_err` (P2a).
- Produces: `impl StreamOps for LocalDocker`. `map_err` now turns `bollard::errors::Error::DockerStreamError{error}` into `Error::Upstream{status: 500, message: error}`.
- bollard 0.21 facts the code relies on (verified against a live daemon):
  - Streams are lazy. `events()` passes `since = now`, so events that happen after the call and before the first poll are not lost.
  - `logs`, `events`, `stats` and `create_image` return `'static` streams. `build_image` borrows the client, so it runs on a task that owns a client clone and forwards results through a channel.
  - `BuildInfo` is not `Serialize`. `build_json` rebuilds Docker's field names (`stream`, `status`, `progressDetail`, `aux`).
  - A failing `RUN` step and a missing image both surface as a stream error. `pull` of a missing image fails with Docker's 404.
  - Exec uses `create_exec` with a TTY and `console_size`, then `start_exec` in attached mode. Input goes through an `mpsc` channel into a writer task, and dropping the sender closes stdin.

- [ ] **Step 1: Write the failing tests**

Create `crates/fleetops-docker/src/streams.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lines_splits_chunks_and_flushes_the_tail() {
        let chunks = vec![
            Ok((LogStream::Stdout, b"a\nb".to_vec())),
            Ok((LogStream::Stdout, b"c\n".to_vec())),
            Ok((LogStream::Stderr, b"tail".to_vec())),
        ];
        let got: Vec<_> = lines(stream::iter(chunks))
            .map(|l| {
                let l = l.unwrap();
                (l.stream, l.text)
            })
            .collect()
            .await;
        assert_eq!(
            got,
            [
                (LogStream::Stdout, "a".to_string()),
                (LogStream::Stdout, "bc".to_string()),
                (LogStream::Stderr, "tail".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn lines_stops_after_an_error() {
        let chunks = vec![
            Ok((LogStream::Stdout, b"x\n".to_vec())),
            Err(Error::EnvUnavailable("gone".into())),
            Ok((LogStream::Stdout, b"never\n".to_vec())),
        ];
        let got: Vec<_> = lines(stream::iter(chunks)).collect().await;
        assert_eq!(got.len(), 2);
        assert!(matches!(got[1], Err(Error::EnvUnavailable(_))));
    }

    #[test]
    fn dockerfile_tar_holds_one_dockerfile() {
        let t = dockerfile_tar(b"FROM alpine\n").unwrap();
        let mut a = tar::Archive::new(&t[..]);
        let mut entries = a.entries().unwrap();
        let mut e = entries.next().unwrap().unwrap();
        assert_eq!(e.path().unwrap().to_str(), Some("Dockerfile"));
        let mut body = String::new();
        std::io::Read::read_to_string(&mut e, &mut body).unwrap();
        assert_eq!(body, "FROM alpine\n");
        assert!(entries.next().is_none());
    }

    #[test]
    fn build_json_keeps_dockers_field_names() {
        let v = build_json(bollard::models::BuildInfo {
            stream: Some("Step 1/2\n".into()),
            progress_detail: Some(bollard::models::ProgressDetail {
                current: Some(1),
                total: Some(2),
            }),
            aux: Some(bollard::models::ImageId {
                id: Some("sha256:abc".into()),
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "stream": "Step 1/2\n",
                "progressDetail": {"current": 1, "total": 2},
                "aux": {"ID": "sha256:abc"}
            })
        );
    }
}
```

Add `mod streams;` to `crates/fleetops-docker/src/lib.rs`, after `mod resources;`.

Add this test to the `tests` module at the bottom of `crates/fleetops-docker/src/error.rs`:

```rust
    #[test]
    fn mid_stream_errors_are_dockers_own() {
        let e = map_err(bollard::errors::Error::DockerStreamError {
            error: "manifest unknown".into(),
        });
        assert!(
            matches!(e, Error::Upstream { status: 500, ref message } if message == "manifest unknown")
        );
    }
```

Append the daemon tests to `crates/fleetops-docker/tests/docker_it.rs`:

```diff
--- a/crates/fleetops-docker/tests/docker_it.rs
+++ b/crates/fleetops-docker/tests/docker_it.rs
@@ -435,3 +435,225 @@ async fn daemon_loss_mid_session_is_unavailable_then_recovers() {
         .unwrap();
     proxy.abort();
 }
+
+mod streams {
+    use super::*;
+    use fleetops_core::streams::{
+        BuildContext, BuildSpec, ExecOptions, LogOptions, LogStream, StreamOps,
+    };
+    use std::time::Duration;
+
+    async fn run_to_exit(d: &LocalDocker, name: &str, spec: ContainerSpec) -> String {
+        let id = d.create_container(&spec).await.unwrap();
+        d.container_action(&id, ContainerAction::Start)
+            .await
+            .unwrap();
+        for _ in 0..50 {
+            let v = d.inspect_container(&id).await.unwrap();
+            if v["State"]["Running"] == false {
+                return id;
+            }
+            tokio::time::sleep(Duration::from_millis(100)).await;
+        }
+        panic!("{name} did not exit");
+    }
+
+    #[tokio::test]
+    async fn events_report_container_lifecycle() {
+        let d = docker().await;
+        let mut events = d.events();
+        let name = unique("events");
+        let id = d.create_container(&sleeper(&name)).await.unwrap();
+        d.container_action(&id, ContainerAction::Start)
+            .await
+            .unwrap();
+        d.remove_container(&id, true, false).await.unwrap();
+        let mut seen = Vec::new();
+        while !seen.contains(&"destroy".to_string()) {
+            let e = tokio::time::timeout(Duration::from_secs(10), events.next())
+                .await
+                .expect("event within 10s")
+                .unwrap()
+                .unwrap();
+            if e.id == id {
+                assert_eq!(e.kind, "container");
+                assert_eq!(e.name.as_deref(), Some(name.as_str()));
+                assert!(e.time > 0);
+                seen.push(e.action);
+            }
+        }
+        for a in ["create", "start", "destroy"] {
+            assert!(seen.iter().any(|s| s == a), "{a} missing from {seen:?}");
+        }
+    }
+
+    #[tokio::test]
+    async fn stats_stream_yields_samples() {
+        let d = docker().await;
+        let name = unique("stats");
+        let id = d.create_container(&sleeper(&name)).await.unwrap();
+        d.container_action(&id, ContainerAction::Start)
+            .await
+            .unwrap();
+        let samples: Vec<_> = d.stats(&id).take(2).collect().await;
+        assert_eq!(samples.len(), 2);
+        let s = samples[1].as_ref().unwrap();
+        assert!(
+            s.memory_limit > 0 && s.memory_used > 0 && s.pids >= 1,
+            "{s:?}"
+        );
+        assert!(s.time > 1_600_000_000_000);
+        d.remove_container(&id, true, false).await.unwrap();
+    }
+
+    #[tokio::test]
+    async fn logs_come_back_as_lines_per_stream() {
+        let d = docker().await;
+        let name = unique("logs");
+        let mut spec = sleeper(&name);
+        spec.cmd = Some(vec![
+            "sh".into(),
+            "-c".into(),
+            "echo one; echo two >&2; printf 'no-newline'".into(),
+        ]);
+        let id = run_to_exit(&d, &name, spec).await;
+        let opts = LogOptions {
+            follow: false,
+            ..Default::default()
+        };
+        let lines: Vec<_> = d.logs(&id, &opts).map(|l| l.unwrap()).collect().await;
+        let got: Vec<_> = lines.iter().map(|l| (l.stream, l.text.as_str())).collect();
+        assert!(got.contains(&(LogStream::Stdout, "one")), "{got:?}");
+        assert!(got.contains(&(LogStream::Stderr, "two")), "{got:?}");
+        assert!(got.contains(&(LogStream::Stdout, "no-newline")), "{got:?}");
+        let tail = LogOptions {
+            follow: false,
+            tail: Some(1),
+            stderr: false,
+            ..Default::default()
+        };
+        let last: Vec<_> = d.logs(&id, &tail).map(|l| l.unwrap().text).collect().await;
+        assert_eq!(last, ["no-newline"]);
+        d.remove_container(&id, true, false).await.unwrap();
+    }
+
+    #[tokio::test]
+    async fn tty_container_logs_are_split_into_lines() {
+        let d = docker().await;
+        let name = unique("ttylogs");
+        let mut spec = sleeper(&name);
+        spec.cmd = Some(vec!["sh".into(), "-c".into(), "printf 'a\\nb\\n'".into()]);
+        spec.advanced = Some(serde_json::json!({"Tty": true}));
+        let id = run_to_exit(&d, &name, spec).await;
+        let opts = LogOptions {
+            follow: false,
+            ..Default::default()
+        };
+        let texts: Vec<_> = d.logs(&id, &opts).map(|l| l.unwrap().text).collect().await;
+        assert_eq!(texts, ["a", "b"]);
+        d.remove_container(&id, true, false).await.unwrap();
+    }
+
+    #[tokio::test]
+    async fn exec_runs_a_shell_with_input_resize_and_exit_code() {
+        let d = docker().await;
+        let name = unique("exec");
+        let id = d.create_container(&sleeper(&name)).await.unwrap();
+        d.container_action(&id, ContainerAction::Start)
+            .await
+            .unwrap();
+        let mut s = d.exec_start(&id, &ExecOptions::default()).await.unwrap();
+        d.exec_resize(&s.exec_id, 120, 40).await.unwrap();
+        s.input
+            .send(b"stty size; echo hi-$((40+2)); exit 3\n".to_vec())
+            .await
+            .unwrap();
+        let mut out = Vec::new();
+        while let Ok(Some(chunk)) =
+            tokio::time::timeout(Duration::from_secs(10), s.output.next()).await
+        {
+            out.extend(chunk.unwrap());
+        }
+        let text = String::from_utf8_lossy(&out);
+        assert!(text.contains("hi-42"), "{text}");
+        assert!(text.contains("40 120"), "resize reached the tty: {text}");
+        let mut code = None;
+        for _ in 0..20 {
+            code = d.exec_exit_code(&s.exec_id).await.unwrap();
+            if code.is_some() {
+                break;
+            }
+            tokio::time::sleep(Duration::from_millis(100)).await;
+        }
+        assert_eq!(code, Some(3));
+        d.remove_container(&id, true, false).await.unwrap();
+    }
+
+    #[tokio::test]
+    async fn exec_in_missing_container_is_upstream_404() {
+        let d = docker().await;
+        match d
+            .exec_start("fleetops-it-nope", &ExecOptions::default())
+            .await
+        {
+            Err(Error::Upstream { status: 404, .. }) => {}
+            Err(e) => panic!("expected 404, got {e:?}"),
+            Ok(_) => panic!("expected 404, got a session"),
+        }
+    }
+
+    #[tokio::test]
+    async fn pull_streams_progress_and_missing_image_fails() {
+        let d = docker().await;
+        let items: Vec<_> = d.pull_image("alpine", "3.20").collect().await;
+        assert!(!items.is_empty());
+        let last = items.last().unwrap().as_ref().unwrap();
+        assert!(
+            last["status"].as_str().unwrap().contains("alpine:3.20"),
+            "{last}"
+        );
+        let missing: Vec<_> = d
+            .pull_image("fleetops-it/does-not-exist", "latest")
+            .collect()
+            .await;
+        assert!(
+            matches!(missing.last(), Some(Err(Error::Upstream { .. }))),
+            "{missing:?}"
+        );
+    }
+
+    #[tokio::test]
+    async fn build_from_a_dockerfile_streams_output_and_tags() {
+        let d = docker().await;
+        let tag = format!("fleetops-it-build:{}", std::process::id());
+        let spec = BuildSpec {
+            context: BuildContext::Dockerfile(
+                b"FROM alpine:3.20\nRUN echo built-ok-$((6*7))\n".to_vec(),
+            ),
+            tag: Some(tag.clone()),
+            dockerfile: None,
+        };
+        let items: Vec<_> = d.build_image(spec).map(|r| r.unwrap()).collect().await;
+        let text: String = items.iter().filter_map(|v| v["stream"].as_str()).collect();
+        assert!(text.contains("built-ok-42"), "{text}");
+        let img = d.inspect_image(&tag).await.unwrap();
+        d.remove_image(img["Id"].as_str().unwrap(), true)
+            .await
+            .unwrap();
+    }
+
+    #[tokio::test]
+    async fn failing_build_step_is_an_error() {
+        let d = docker().await;
+        let spec = BuildSpec {
+            context: BuildContext::Dockerfile(b"FROM alpine:3.20\nRUN exit 7\n".to_vec()),
+            tag: None,
+            dockerfile: None,
+        };
+        let items: Vec<_> = d.build_image(spec).collect().await;
+        assert!(
+            matches!(items.last(), Some(Err(Error::Upstream { .. }))),
+            "{items:?}"
+        );
+    }
+}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-docker --lib`
Expected: FAIL to compile with `cannot find function lines` / `dockerfile_tar` / `build_json`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-docker/src/streams.rs`:

```rust
//! Events, stats, logs, exec and image pull/build streams.

use crate::{error::map_err, LocalDocker};
use async_trait::async_trait;
use bollard::container::LogOutput;
use bollard::exec::{StartExecOptions, StartExecResults};
use bollard::models::ExecConfig;
use bollard::query_parameters::{
    BuildImageOptionsBuilder, CreateImageOptionsBuilder, EventsOptions, LogsOptionsBuilder,
    ResizeExecOptionsBuilder, StatsOptionsBuilder,
};
use fleetops_core::streams::{
    stats_sample, BoxStream, BuildContext, BuildSpec, DockerEvent, ExecOptions, ExecSession,
    LineSplitter, LogLine, LogOptions, LogStream, StreamOps,
};
use fleetops_core::{now_unix, Error, Result};
use futures_util::{stream, Stream, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

fn to_json<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}

/// Turns a channel receiver into a stream.
fn receiver<T: Send + 'static>(rx: mpsc::Receiver<Result<T>>) -> BoxStream<T> {
    Box::pin(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }))
}

/// Splits framed log output into lines, flushing any partial line at the end.
fn lines<S>(inner: S) -> BoxStream<LogLine>
where
    S: Stream<Item = Result<(LogStream, Vec<u8>)>> + Send + 'static,
{
    struct St<S> {
        inner: std::pin::Pin<Box<S>>,
        split: LineSplitter,
        pending: VecDeque<LogLine>,
        done: bool,
    }
    let st = St {
        inner: Box::pin(inner),
        split: LineSplitter::default(),
        pending: VecDeque::new(),
        done: false,
    };
    Box::pin(stream::unfold(st, |mut st| async move {
        loop {
            if let Some(line) = st.pending.pop_front() {
                return Some((Ok(line), st));
            }
            if st.done {
                return None;
            }
            match st.inner.next().await {
                Some(Ok((stream, chunk))) => st.pending.extend(st.split.push(stream, &chunk)),
                Some(Err(e)) => {
                    st.done = true;
                    return Some((Err(e), st));
                }
                None => {
                    st.done = true;
                    st.pending.extend(st.split.finish());
                }
            }
        }
    }))
}

fn attributes_name(a: &std::collections::BTreeMap<String, String>) -> Option<String> {
    a.get("name").cloned()
}

fn output_bytes(o: LogOutput) -> (LogStream, Vec<u8>) {
    match o {
        LogOutput::StdErr { message } => (LogStream::Stderr, message.to_vec()),
        LogOutput::StdOut { message }
        | LogOutput::Console { message }
        | LogOutput::StdIn { message } => (LogStream::Stdout, message.to_vec()),
    }
}

/// bollard's `BuildInfo` is not `Serialize`; rebuild Docker's JSON shape.
fn build_json(b: bollard::models::BuildInfo) -> Result<Value> {
    let mut v = serde_json::Map::new();
    let mut put = |k: &str, x: Option<Value>| {
        if let Some(x) = x {
            v.insert(k.into(), x);
        }
    };
    put("id", b.id.map(Value::String));
    put("stream", b.stream.map(Value::String));
    put("status", b.status.map(Value::String));
    put(
        "progressDetail",
        b.progress_detail.map(to_json).transpose()?,
    );
    put("aux", b.aux.map(to_json).transpose()?);
    Ok(Value::Object(v))
}

/// A tar holding just `Dockerfile`.
fn dockerfile_tar(dockerfile: &[u8]) -> Result<Vec<u8>> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(dockerfile.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(now_unix() as u64);
    h.set_cksum();
    b.append_data(&mut h, "Dockerfile", dockerfile)
        .and_then(|_| b.into_inner())
        .map_err(|e| Error::Internal(format!("building context: {e}")))
}

#[async_trait]
impl StreamOps for LocalDocker {
    fn events(&self) -> BoxStream<DockerEvent> {
        // Streams are lazy; `since` makes "from now on" mean when events() was
        // called, not when the stream is first polled.
        let opts = EventsOptions {
            since: Some(now_unix().to_string()),
            ..Default::default()
        };
        let s = self.client.events(Some(opts)).map(|r| {
            let m = r.map_err(map_err)?;
            let actor = m.actor.unwrap_or_default();
            let attributes: std::collections::BTreeMap<String, String> =
                actor.attributes.unwrap_or_default().into_iter().collect();
            Ok(DockerEvent {
                kind: m.typ.map(|t| t.to_string()).unwrap_or_default(),
                action: m.action.unwrap_or_default(),
                id: actor.id.unwrap_or_default(),
                name: attributes_name(&attributes),
                attributes,
                time: m.time.unwrap_or_default(),
            })
        });
        Box::pin(s)
    }

    fn stats(&self, id: &str) -> BoxStream<fleetops_core::streams::StatsSample> {
        let opts = StatsOptionsBuilder::new().stream(true).build();
        let s = self.client.stats(id, Some(opts)).map(|r| {
            let v = to_json(r.map_err(map_err)?)?;
            let time_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Ok(stats_sample(&v, time_ms))
        });
        Box::pin(s)
    }

    fn logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        let tail = opts
            .tail
            .map(|n| n.to_string())
            .unwrap_or_else(|| "all".into());
        let o = LogsOptionsBuilder::new()
            .follow(opts.follow)
            .stdout(opts.stdout)
            .stderr(opts.stderr)
            .timestamps(opts.timestamps)
            .since(opts.since.unwrap_or(0).clamp(0, i32::MAX as i64) as i32)
            .tail(&tail)
            .build();
        let s = self
            .client
            .logs(id, Some(o))
            .map(|r| r.map(output_bytes).map_err(map_err));
        lines(s)
    }

    async fn exec_start(&self, id: &str, opts: &ExecOptions) -> Result<ExecSession> {
        let config = ExecConfig {
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(true),
            cmd: Some(opts.command()),
            user: opts.user.clone(),
            console_size: Some(vec![opts.rows as usize, opts.cols as usize]),
            ..Default::default()
        };
        let created = self.client.create_exec(id, config).await.map_err(map_err)?;
        let started = self
            .client
            .start_exec(
                &created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(map_err)?;
        let StartExecResults::Attached { output, mut input } = started else {
            return Err(Error::Internal("exec started detached".into()));
        };
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
        tokio::spawn(async move {
            while let Some(buf) = rx.recv().await {
                if input.write_all(&buf).await.is_err() || input.flush().await.is_err() {
                    break;
                }
            }
            let _ = input.shutdown().await;
        });
        Ok(ExecSession {
            exec_id: created.id,
            output: Box::pin(output.map(|r| r.map(|o| output_bytes(o).1).map_err(map_err))),
            input: tx,
        })
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<()> {
        let o = ResizeExecOptionsBuilder::new()
            .w(cols as i32)
            .h(rows as i32)
            .build();
        self.client.resize_exec(exec_id, o).await.map_err(map_err)
    }

    async fn exec_exit_code(&self, exec_id: &str) -> Result<Option<i64>> {
        let r = self.client.inspect_exec(exec_id).await.map_err(map_err)?;
        Ok(if r.running == Some(true) {
            None
        } else {
            r.exit_code
        })
    }

    fn pull_image(&self, image: &str, tag: &str) -> BoxStream<Value> {
        let o = CreateImageOptionsBuilder::new()
            .from_image(image)
            .tag(tag)
            .build();
        let s = self
            .client
            .create_image(Some(o), None, None)
            .map(|r| r.map_err(map_err).and_then(to_json));
        Box::pin(s)
    }

    fn build_image(&self, spec: BuildSpec) -> BoxStream<Value> {
        let tar = match &spec.context {
            BuildContext::Tar(t) => Ok(t.clone()),
            BuildContext::Dockerfile(d) => dockerfile_tar(d),
        };
        let tar = match tar {
            Ok(t) => t,
            Err(e) => return Box::pin(stream::once(async move { Err(e) })),
        };
        let mut o = BuildImageOptionsBuilder::new()
            .dockerfile(spec.dockerfile.as_deref().unwrap_or("Dockerfile"))
            .rm(true);
        if let Some(t) = &spec.tag {
            o = o.t(t);
        }
        let opts = o.build();
        let client = self.client.clone();
        // build_image borrows the client, so it runs on a task that owns one.
        // Dropping the returned stream closes the channel and ends the task.
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut s = client.build_image(opts, None, Some(bollard::body_full(tar.into())));
            while let Some(item) = s.next().await {
                if tx
                    .send(item.map_err(map_err).and_then(build_json))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        receiver(rx)
    }
}
```

In `crates/fleetops-docker/src/error.rs`, add a `DockerStreamError` arm to `map_err`, just before the JSON-error arm:

```rust
        // Mid-stream failures of an accepted request (a pull whose layer
        // fails, a build step that exits non-zero) are Docker's own errors.
        B::DockerStreamError { error } => Error::Upstream {
            status: 500,
            message: error,
        },
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-docker && make lint`
Expected:
- The 4 `streams` unit tests and 3 `error` tests PASS, along with every other unit test.
- `docker_it` compiles to 0 tests without the feature.
- Lint is clean.

- [ ] **Step 5: Run the daemon tests**

Run: `make test-docker && cargo clippy -p fleetops-docker --features docker-it --all-targets -- -D warnings`
Expected:
- 21 PASS: P2a's 12, plus `streams::` `events_report_container_lifecycle`, `stats_stream_yields_samples`, `logs_come_back_as_lines_per_stream`, `tty_container_logs_are_split_into_lines`, `exec_runs_a_shell_with_input_resize_and_exit_code`, `exec_in_missing_container_is_upstream_404`, `pull_streams_progress_and_missing_image_fails`, `build_from_a_dockerfile_streams_output_and_tags` and `failing_build_step_is_an_error`.
- Clippy is clean.
- `docker ps -a --filter name=fleetops-it` and `docker images 'fleetops-it-build*'` both show nothing.

If there is no Docker daemon, record that in the ledger and continue; the unit tests are the gate.

- [ ] **Step 6: Commit**

```bash
git add crates/fleetops-docker
git commit -m "feat(docker): events, stats, logs, exec, pull and build streams"
```

---

### Task 3: Websocket wire protocol and per-subscription queues

**Files:**
- Create: `crates/fleetops-api/src/ws/mod.rs`, `crates/fleetops-api/src/ws/protocol.rs`, `crates/fleetops-api/src/ws/queue.rs`
- Modify: `crates/fleetops-api/Cargo.toml` (add `futures-util`), `crates/fleetops-api/src/lib.rs`
- Test: unit tests in `protocol.rs` and `queue.rs`

**Interfaces:**
- Produces (in `fleetops_api::ws::protocol`):
  - `Topic{Events,Stats,Logs,Exec,Task}`.
  - `ClientMsg`, tagged by `op`: `Sub{sub, topic, env?, id?, opts}`, `Unsub{sub}`, `Input{sub, data}` and `Resize{sub, cols, rows}`.
  - `Out{Text(String), Binary(Vec<u8>)}`.
  - Builders: `data(sub, Value)`, `gap(sub, n)`, `end(sub, reason, extra)`, `error(Option<sub>, code, message)` and `env_status(env, &status)`.
  - `encode_binary(sub, payload)` and `decode_binary(&[u8]) -> Option<(u32, &[u8])>`.
- Produces (in `fleetops_api::ws::queue`):
  - `QUEUE_CAPACITY = 1024` and `Policy{DropOldest, Gap, Block}`.
  - `SubQueue::new(sub, policy, capacity, ready: Arc<Notify>)`.
  - Producer side: `push(msg, weight) -> bool`, `push_wait(msg).await -> bool` and `finish(msg)`.
  - Consumer and lifecycle: `pop()`, `drained()`, `close()` and `sub()`.
  - Every push notifies `ready`, which wakes the connection's writer.

- [ ] **Step 1: Write the failing tests**

Add `futures-util.workspace = true` under `[dependencies]` in `crates/fleetops-api/Cargo.toml`, after `axum`. Add `pub mod ws;` to `crates/fleetops-api/src/lib.rs`, after `pub mod state;`.

`crates/fleetops-api/src/ws/mod.rs` (Task 5 extends it):

```rust
//! `/api/v1/ws`: one multiplexed websocket per browser tab (spec §5.2).

pub mod protocol;
pub mod queue;
```

Create `crates/fleetops-api/src/ws/protocol.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_client_op() {
        let sub: ClientMsg = serde_json::from_str(
            r#"{"op":"sub","sub":7,"topic":"logs","env":"local","id":"web","opts":{"tail":10}}"#,
        )
        .unwrap();
        assert_eq!(
            sub,
            ClientMsg::Sub {
                sub: 7,
                topic: Topic::Logs,
                env: Some("local".into()),
                id: Some("web".into()),
                opts: json!({"tail": 10})
            }
        );
        let bare: ClientMsg =
            serde_json::from_str(r#"{"op":"sub","sub":1,"topic":"events","env":"local"}"#).unwrap();
        assert!(matches!(
            bare,
            ClientMsg::Sub {
                id: None,
                opts: Value::Null,
                ..
            }
        ));
        for (s, want) in [
            (r#"{"op":"unsub","sub":3}"#, ClientMsg::Unsub { sub: 3 }),
            (
                r#"{"op":"input","sub":3,"data":"ls\n"}"#,
                ClientMsg::Input {
                    sub: 3,
                    data: "ls\n".into(),
                },
            ),
            (
                r#"{"op":"resize","sub":3,"cols":120,"rows":40}"#,
                ClientMsg::Resize {
                    sub: 3,
                    cols: 120,
                    rows: 40,
                },
            ),
        ] {
            assert_eq!(serde_json::from_str::<ClientMsg>(s).unwrap(), want);
        }
        for bad in [
            r#"{"op":"sub","sub":1,"topic":"nope"}"#,
            r#"{"op":"explode"}"#,
            r#"{"op":"unsub","sub":-1}"#,
            "not json",
        ] {
            assert!(serde_json::from_str::<ClientMsg>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn server_messages_have_the_documented_shape() {
        let text = |o: Out| match o {
            Out::Text(t) => serde_json::from_str::<Value>(&t).unwrap(),
            Out::Binary(_) => panic!("binary"),
        };
        assert_eq!(
            text(data(1, json!([1]))),
            json!({"sub": 1, "type": "data", "data": [1]})
        );
        assert_eq!(
            text(gap(2, 9)),
            json!({"sub": 2, "type": "gap", "dropped": 9})
        );
        assert_eq!(
            text(end(3, "exited", json!({"exit_code": 0}))),
            json!({"sub": 3, "type": "end", "reason": "exited", "exit_code": 0})
        );
        assert_eq!(
            text(error(None, "invalid", "bad frame")),
            json!({"sub": null, "type": "error", "code": "invalid", "message": "bad frame"})
        );
        assert_eq!(
            text(env_status("local", &json!({"state": "down"}))),
            json!({"type": "env_status", "env": "local", "status": {"state": "down"}})
        );
    }

    #[test]
    fn binary_frames_round_trip() {
        let f = encode_binary(0x0102_0304, b"hi");
        assert_eq!(f, [1, 2, 3, 4, b'h', b'i']);
        assert_eq!(decode_binary(&f), Some((0x0102_0304, &b"hi"[..])));
        assert_eq!(decode_binary(&[0, 0, 0, 5]), Some((5, &b""[..])));
        assert_eq!(decode_binary(&[0, 0, 1]), None);
    }
}
```

Create `crates/fleetops-api/src/ws/queue.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::protocol::data;
    use serde_json::json;
    use std::time::Duration;

    fn q(policy: Policy, cap: usize) -> SubQueue {
        SubQueue::new(1, policy, cap, Arc::new(Notify::new()))
    }

    fn n(i: u64) -> Out {
        data(1, json!(i))
    }

    fn drain(q: &SubQueue) -> Vec<Out> {
        std::iter::from_fn(|| q.pop()).collect()
    }

    #[test]
    fn drop_oldest_keeps_the_newest() {
        let q = q(Policy::DropOldest, 3);
        for i in 0..5 {
            assert!(q.push(n(i), 1));
        }
        assert_eq!(drain(&q), [n(2), n(3), n(4)]);
    }

    #[test]
    fn gap_drops_new_lines_and_reports_how_many() {
        let q = q(Policy::Gap, 3);
        for i in 0..3 {
            q.push(n(i), 10);
        }
        // Full: these batches (10 lines each) are dropped.
        q.push(n(3), 10);
        q.push(n(4), 10);
        assert_eq!(q.pop(), Some(n(0)));
        assert_eq!(q.pop(), Some(n(1)));
        // Room for gap + message now.
        q.push(n(5), 10);
        assert_eq!(drain(&q), [n(2), gap(1, 20), n(5)]);
    }

    #[test]
    fn gap_is_not_lost_when_nothing_follows() {
        let q = q(Policy::Gap, 1);
        q.push(n(0), 1);
        q.push(n(1), 4);
        assert_eq!(drain(&q), [n(0), gap(1, 4)]);
    }

    #[test]
    fn finish_goes_out_last_even_when_full() {
        let q = q(Policy::Gap, 1);
        q.push(n(0), 1);
        q.push(n(1), 2);
        q.finish(n(99));
        assert!(!q.drained());
        assert_eq!(drain(&q), [n(0), gap(1, 2), n(99)]);
        assert!(q.drained());
    }

    #[test]
    fn closed_queue_rejects_pushes() {
        let q = q(Policy::DropOldest, 2);
        q.push(n(0), 1);
        q.close();
        assert!(!q.push(n(1), 1));
        assert_eq!(q.pop(), None);
    }

    #[tokio::test]
    async fn block_waits_for_room() {
        let q = Arc::new(q(Policy::Block, 2));
        assert!(q.push_wait(n(0)).await);
        assert!(q.push_wait(n(1)).await);
        let q2 = q.clone();
        let blocked = tokio::spawn(async move { q2.push_wait(n(2)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!blocked.is_finished(), "producer must wait while full");
        assert_eq!(q.pop(), Some(n(0)));
        assert!(tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(drain(&q), [n(1), n(2)]);
    }

    #[tokio::test]
    async fn close_releases_a_blocked_producer() {
        let q = Arc::new(q(Policy::Block, 1));
        q.push_wait(n(0)).await;
        let q2 = q.clone();
        let blocked = tokio::spawn(async move { q2.push_wait(n(1)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        q.close();
        assert!(!tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap());
    }

    #[test]
    fn pushes_wake_the_writer() {
        let ready = Arc::new(Notify::new());
        let q = SubQueue::new(1, Policy::DropOldest, 4, ready.clone());
        q.push(n(0), 1);
        // A stored permit: the next notified() completes immediately.
        let fut = ready.notified();
        futures_util::FutureExt::now_or_never(fut).expect("writer was notified");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --lib ws::`
Expected: FAIL to compile with `cannot find type ClientMsg` / `SubQueue` / `Policy`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-api/src/ws/protocol.rs`:

```rust
//! Wire format of `/api/v1/ws` (spec §5.2). Text frames carry JSON; exec
//! I/O uses binary frames of `[u32 sub id, big-endian][payload]`.

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Topic {
    Events,
    Stats,
    Logs,
    Exec,
    Task,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ClientMsg {
    Sub {
        sub: u32,
        topic: Topic,
        #[serde(default)]
        env: Option<String>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        opts: Value,
    },
    Unsub {
        sub: u32,
    },
    /// Exec stdin as text, for clients that cannot send binary frames.
    Input {
        sub: u32,
        data: String,
    },
    Resize {
        sub: u32,
        cols: u16,
        rows: u16,
    },
}

/// A frame ready to go out on the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Text(String),
    Binary(Vec<u8>),
}

pub fn data(sub: u32, data: Value) -> Out {
    Out::Text(json!({"sub": sub, "type": "data", "data": data}).to_string())
}

pub fn gap(sub: u32, dropped: u64) -> Out {
    Out::Text(json!({"sub": sub, "type": "gap", "dropped": dropped}).to_string())
}

pub fn end(sub: u32, reason: &str, extra: Value) -> Out {
    let mut v = json!({"sub": sub, "type": "end", "reason": reason});
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    Out::Text(v.to_string())
}

/// `sub` is None for errors about a frame that named no valid subscription.
pub fn error(sub: Option<u32>, code: &str, message: &str) -> Out {
    Out::Text(json!({"sub": sub, "type": "error", "code": code, "message": message}).to_string())
}

pub fn env_status(env: &str, status: &Value) -> Out {
    Out::Text(json!({"type": "env_status", "env": env, "status": status}).to_string())
}

pub fn encode_binary(sub: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + payload.len());
    v.extend_from_slice(&sub.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_binary(frame: &[u8]) -> Option<(u32, &[u8])> {
    let (id, payload) = frame.split_at_checked(4)?;
    Some((u32::from_be_bytes(id.try_into().ok()?), payload))
}
```

Prepend to `crates/fleetops-api/src/ws/queue.rs`:

```rust
//! One bounded outbound queue per subscription (spec §5.2 backpressure).
//! Producers never wait on the socket, except exec, which waits for room
//! so that Docker's output is read only as fast as the client takes it.

use super::protocol::{gap, Out};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

pub const QUEUE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// events, stats, task: the newest state matters most.
    DropOldest,
    /// logs: drop new lines and tell the client how many with `gap`.
    Gap,
    /// exec: the producer waits for room.
    Block,
}

struct Inner {
    items: VecDeque<Out>,
    /// Log lines dropped since the last `gap` went out.
    dropped: u64,
    closed: bool,
    /// The producer is done; the queue goes away once drained.
    finished: bool,
}

pub struct SubQueue {
    sub: u32,
    policy: Policy,
    capacity: usize,
    inner: Mutex<Inner>,
    /// Wakes the connection's writer.
    ready: Arc<Notify>,
    /// Wakes a blocked exec producer.
    space: Notify,
}

impl SubQueue {
    pub fn new(sub: u32, policy: Policy, capacity: usize, ready: Arc<Notify>) -> Self {
        Self {
            sub,
            policy,
            capacity,
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
                dropped: 0,
                closed: false,
                finished: false,
            }),
            ready,
            space: Notify::new(),
        }
    }

    pub fn sub(&self) -> u32 {
        self.sub
    }

    /// Queues `msg`, which stands for `weight` log lines (1 otherwise).
    /// Returns false once the subscription is closed, so producers stop.
    pub fn push(&self, msg: Out, weight: u64) -> bool {
        let mut q = self.inner.lock().unwrap();
        if q.closed {
            return false;
        }
        match self.policy {
            Policy::DropOldest | Policy::Block => {
                if q.items.len() >= self.capacity {
                    q.items.pop_front();
                }
                q.items.push_back(msg);
            }
            Policy::Gap => {
                // Room for this message, plus a `gap` ahead of it if lines
                // were lost since the last one went out.
                let need = if q.dropped > 0 { 2 } else { 1 };
                if q.items.len() + need > self.capacity {
                    q.dropped += weight;
                    return true;
                }
                if q.dropped > 0 {
                    let n = std::mem::take(&mut q.dropped);
                    q.items.push_back(gap(self.sub, n));
                }
                q.items.push_back(msg);
            }
        }
        drop(q);
        self.ready.notify_one();
        true
    }

    /// Waits until there is room, then queues `msg`. Returns false once the
    /// subscription is closed.
    pub async fn push_wait(&self, msg: Out) -> bool {
        loop {
            let space = self.space.notified();
            {
                let mut q = self.inner.lock().unwrap();
                if q.closed {
                    return false;
                }
                if q.items.len() < self.capacity {
                    q.items.push_back(msg);
                    drop(q);
                    self.ready.notify_one();
                    return true;
                }
            }
            space.await;
        }
    }

    /// Queues the subscription's last message (`end` or `error`) regardless
    /// of capacity, and marks the producer finished.
    pub fn finish(&self, msg: Out) {
        let mut q = self.inner.lock().unwrap();
        if q.closed {
            return;
        }
        if q.dropped > 0 {
            let n = std::mem::take(&mut q.dropped);
            q.items.push_back(gap(self.sub, n));
        }
        q.items.push_back(msg);
        q.finished = true;
        drop(q);
        self.ready.notify_one();
    }

    pub fn pop(&self) -> Option<Out> {
        let mut q = self.inner.lock().unwrap();
        let item = q
            .items
            .pop_front()
            .or_else(|| (q.dropped > 0).then(|| gap(self.sub, std::mem::take(&mut q.dropped))));
        drop(q);
        if item.is_some() {
            self.space.notify_one();
        }
        item
    }

    /// True when the producer finished and everything has been sent.
    pub fn drained(&self) -> bool {
        let q = self.inner.lock().unwrap();
        q.finished && q.items.is_empty()
    }

    /// Unsubscribe: drop pending messages and stop producers.
    pub fn close(&self) {
        let mut q = self.inner.lock().unwrap();
        q.closed = true;
        q.items.clear();
        drop(q);
        self.space.notify_waiters();
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api --lib ws:: && make lint`
Expected: 3 `protocol` + 8 `queue` tests PASS, lint clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/fleetops-api
git commit -m "feat(api): websocket wire protocol and per-subscription queues"
```

---

### Task 4: Background task manager

**Files:**
- Create: `crates/fleetops-api/src/tasks.rs`
- Modify: `crates/fleetops-api/src/lib.rs`
- Test: unit tests in `tasks.rs`

**Interfaces:**
- Consumes: `fleetops_core::streams::BoxStream` (Task 1).
- Produces (in `fleetops_api::tasks`):
  - Limits: `TASK_LOG_MAX = 2000`, `TASK_RETENTION = 600 s` and `MAX_RUNNING_TASKS = 8`.
  - `TaskStatus{Running,Succeeded,Failed}`.
  - `TaskInfo{id, kind, env, target, status, error, created_at, finished_at}`.
  - `TaskEvent{Progress(Value), Done(TaskInfo)}`.
  - `Task{owner}`, with `info()` and `feed()`. `feed()` returns `TaskFeed{backlog, done, live}`, taken under one lock.
  - `NewTask{kind, env, target, owner}`.
  - `TaskManager::spawn(NewTask, BoxStream<Value>, on_done) -> Result<Arc<Task>>`:
    - A stream error fails the task, and so does a progress message whose `error` or `errorDetail.message` is set.
    - Returns `Conflict` when 8 tasks are already running.
    - Finished tasks older than 10 minutes are pruned.
  - `TaskManager::get(id)`.

- [ ] **Step 1: Write the failing tests**

Add `pub mod tasks;` to `crates/fleetops-api/src/lib.rs`, after `pub mod state;`. Create `crates/fleetops-api/src/tasks.rs` containing only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use serde_json::json;

    fn new_task() -> NewTask {
        NewTask {
            kind: "image.pull",
            env: "local".into(),
            target: "alpine:3.20".into(),
            owner: 1,
        }
    }

    fn items(v: Vec<Result<Value>>) -> BoxStream<Value> {
        Box::pin(stream::iter(v))
    }

    async fn wait_done(t: &Task) -> TaskInfo {
        for _ in 0..100 {
            let i = t.info();
            if i.status != TaskStatus::Running {
                return i;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task did not finish");
    }

    #[tokio::test]
    async fn successful_stream_succeeds_and_keeps_progress() {
        let m = TaskManager::default();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let t = m
            .spawn(
                new_task(),
                items(vec![Ok(json!({"status": "a"})), Ok(json!({"status": "b"}))]),
                |info| async move {
                    let _ = done_tx.send(info);
                },
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Succeeded);
        assert!(info.finished_at.is_some());
        assert_eq!(done_rx.await.unwrap().status, TaskStatus::Succeeded);
        let feed = t.feed();
        assert_eq!(
            feed.backlog,
            [json!({"status": "a"}), json!({"status": "b"})]
        );
        assert_eq!(feed.done.unwrap().status, TaskStatus::Succeeded);
        assert!(m.get(&info.id).is_some());
    }

    #[tokio::test]
    async fn stream_error_or_error_message_fails_the_task() {
        let m = TaskManager::default();
        let t = m
            .spawn(
                new_task(),
                items(vec![Err(Error::Upstream {
                    status: 404,
                    message: "manifest unknown".into(),
                })]),
                |_| async {},
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Failed);
        assert!(info.error.unwrap().contains("manifest unknown"));
        let t = m
            .spawn(
                new_task(),
                items(vec![
                    Ok(json!({"stream": "Step 1"})),
                    Ok(json!({"errorDetail": {"message": "returned a non-zero code: 7"}})),
                    Ok(json!({"stream": "never read"})),
                ]),
                |_| async {},
            )
            .unwrap();
        let info = wait_done(&t).await;
        assert_eq!(info.status, TaskStatus::Failed);
        assert!(info.error.unwrap().contains("non-zero code: 7"));
        assert_eq!(t.feed().backlog.len(), 2);
    }

    #[tokio::test]
    async fn backlog_is_capped() {
        let m = TaskManager::default();
        let many = (0..TASK_LOG_MAX + 5).map(|i| Ok(json!(i))).collect();
        let t = m.spawn(new_task(), items(many), |_| async {}).unwrap();
        wait_done(&t).await;
        let backlog = t.feed().backlog;
        assert_eq!(backlog.len(), TASK_LOG_MAX);
        assert_eq!(backlog[0], json!(5));
    }

    #[tokio::test]
    async fn running_tasks_are_limited() {
        let m = TaskManager::default();
        for _ in 0..MAX_RUNNING_TASKS {
            m.spawn(new_task(), Box::pin(stream::pending()), |_| async {})
                .unwrap();
        }
        assert!(matches!(
            m.spawn(new_task(), Box::pin(stream::pending()), |_| async {}),
            Err(Error::Conflict(_))
        ));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --lib tasks::`
Expected: FAIL to compile with `cannot find type TaskManager` / `NewTask`.

- [ ] **Step 3: Implement**

Prepend to `crates/fleetops-api/src/tasks.rs`:

```rust
//! Background tasks (image pull and build): started by a REST call that
//! returns `202 {task_id}` at once, followed over the websocket `task` topic
//! or polled with `GET /tasks/{id}`.

use fleetops_core::streams::BoxStream;
use fleetops_core::{now_unix, Error, Result};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;

/// Progress messages kept per task for late subscribers.
pub const TASK_LOG_MAX: usize = 2000;
/// How long a finished task stays queryable.
pub const TASK_RETENTION: Duration = Duration::from_secs(600);
/// Running tasks allowed at once across the server.
pub const MAX_RUNNING_TASKS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskInfo {
    pub id: String,
    /// `image.pull` or `image.build`.
    pub kind: String,
    pub env: String,
    /// What the task works on, e.g. the image reference.
    pub target: String,
    pub status: TaskStatus,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum TaskEvent {
    Progress(Value),
    Done(TaskInfo),
}

pub struct Task {
    pub owner: i64,
    state: Mutex<TaskState>,
    tx: broadcast::Sender<TaskEvent>,
}

struct TaskState {
    info: TaskInfo,
    log: VecDeque<Value>,
}

/// What a subscriber gets: everything so far, then live events.
pub struct TaskFeed {
    pub backlog: Vec<Value>,
    /// Set when the task had already finished.
    pub done: Option<TaskInfo>,
    pub live: broadcast::Receiver<TaskEvent>,
}

impl Task {
    pub fn info(&self) -> TaskInfo {
        self.state.lock().unwrap().info.clone()
    }

    /// Backlog and live receiver taken under one lock, so nothing is missed
    /// or seen twice.
    pub fn feed(&self) -> TaskFeed {
        let st = self.state.lock().unwrap();
        TaskFeed {
            backlog: st.log.iter().cloned().collect(),
            done: (st.info.status != TaskStatus::Running).then(|| st.info.clone()),
            live: self.tx.subscribe(),
        }
    }

    fn progress(&self, v: Value) {
        let mut st = self.state.lock().unwrap();
        if st.log.len() >= TASK_LOG_MAX {
            st.log.pop_front();
        }
        st.log.push_back(v.clone());
        let _ = self.tx.send(TaskEvent::Progress(v));
    }

    fn finish(&self, outcome: &Result<()>) -> TaskInfo {
        let mut st = self.state.lock().unwrap();
        st.info.status = if outcome.is_ok() {
            TaskStatus::Succeeded
        } else {
            TaskStatus::Failed
        };
        st.info.error = outcome.as_ref().err().map(|e| e.to_string());
        st.info.finished_at = Some(now_unix());
        let info = st.info.clone();
        let _ = self.tx.send(TaskEvent::Done(info.clone()));
        info
    }
}

#[derive(Default)]
pub struct TaskManager {
    tasks: Mutex<HashMap<String, Arc<Task>>>,
}

pub struct NewTask {
    pub kind: &'static str,
    pub env: String,
    pub target: String,
    pub owner: i64,
}

impl TaskManager {
    /// Runs `stream` to completion in the background. `on_done` runs with
    /// the outcome afterwards (for the audit log). A stream error, or a
    /// progress message carrying Docker's `error` field, fails the task.
    pub fn spawn<F, Fut>(
        &self,
        new: NewTask,
        mut stream: BoxStream<Value>,
        on_done: F,
    ) -> Result<Arc<Task>>
    where
        F: FnOnce(TaskInfo) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let mut tasks = self.tasks.lock().unwrap();
        let now = now_unix();
        tasks.retain(|_, t| {
            t.info()
                .finished_at
                .is_none_or(|f| now - f < TASK_RETENTION.as_secs() as i64)
        });
        let running = tasks
            .values()
            .filter(|t| t.info().status == TaskStatus::Running)
            .count();
        if running >= MAX_RUNNING_TASKS {
            return Err(Error::Conflict(format!(
                "{MAX_RUNNING_TASKS} tasks are already running; try again when one finishes"
            )));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let task = Arc::new(Task {
            owner: new.owner,
            state: Mutex::new(TaskState {
                info: TaskInfo {
                    id: id.clone(),
                    kind: new.kind.to_string(),
                    env: new.env,
                    target: new.target,
                    status: TaskStatus::Running,
                    error: None,
                    created_at: now,
                    finished_at: None,
                },
                log: VecDeque::new(),
            }),
            tx: broadcast::channel(256).0,
        });
        tasks.insert(id, task.clone());
        let t = task.clone();
        tokio::spawn(async move {
            let mut outcome = Ok(());
            while let Some(item) = stream.next().await {
                match item {
                    Ok(v) => {
                        if let Some(msg) =
                            v["error"].as_str().or(v["errorDetail"]["message"].as_str())
                        {
                            outcome = Err(Error::Upstream {
                                status: 500,
                                message: msg.to_string(),
                            });
                            t.progress(v);
                            break;
                        }
                        t.progress(v);
                    }
                    Err(e) => {
                        outcome = Err(e);
                        break;
                    }
                }
            }
            let info = t.finish(&outcome);
            on_done(info).await;
        });
        Ok(task)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        self.tasks.lock().unwrap().get(id).cloned()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api --lib tasks:: && make lint`
Expected: 4 `tasks` tests PASS, lint clean.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): background task manager with backlog and audit hook"
```

---

### Task 5: The `/ws` endpoint, connections, hubs and the `events` topic

**Files:**
- Modify: `Cargo.toml` (axum `ws` feature, `tokio-tungstenite`), `crates/fleetops-api/Cargo.toml` (dev `tokio-tungstenite`)
- Modify: `crates/fleetops-core/src/docker.rs` (`DockerOps` includes `StreamOps`)
- Modify: `crates/fleetops-api/src/state.rs`, `crates/fleetops-api/src/routes/mod.rs`, `crates/fleetops-api/src/ws/mod.rs`
- Create: `crates/fleetops-api/src/ws/hub.rs`, `crates/fleetops-api/src/ws/conn.rs`, `crates/fleetops-api/src/ws/topics.rs`
- Modify: `crates/fleetops-api/tests/common/fake_docker.rs`, `crates/fleetops-api/tests/common/mod.rs`
- Test: `crates/fleetops-api/tests/ws.rs`, `crates/fleetops-api/tests/ws_events.rs`

**Interfaces:**
- Consumes: Tasks 1–4. `CurrentUser`, `authorize`, `EnvRegistry` and the audit table come from earlier plans.
- Produces:
  - `AppState` gains `hubs: Arc<Hubs>` and `tasks: Arc<TaskManager>`.
  - `ApiConfig` gains `ws_backoff` (1 s), `ws_session_check` (30 s) and `ws_queue_capacity` (1024).
  - Endpoint: `GET /api/v1/ws` (`ws::handler`), guarded by `ws::origin_allowed(&HeaderMap)`.
  - `ws::hub`:
    - `EventHub::subscribe(env, status_tx, backoff)`.
    - `StatsHub::subscribe(ops, id) -> (Arc<SharedStats>, Receiver<Option<StatsSample>>)` and `StatsHub::open()`.
    - `Hubs::{events(&env), stats(&env), status()}`, keyed by environment *instance*.
    - `StatusChange{env, status}`.
  - `ws::conn`: `run(state, user, token_hash, socket)`, `MAX_SUBS = 256`, `MAX_FRAME_BYTES = 1 MiB` and `pub(super) Sub{queue, task}`.
  - `ws::topics`: `Ctx{s, cu, sub, ready}` and `events(&Ctx, env, id)`. Later tasks add `stats`, `logs`, `exec` and `task`, and the dispatch in `conn::start` gains their arms.
  - Test support:
    - `FakeDocker` implements `StreamOps` through `FakeStreams`: `emit_event`, `open_event_streams`, `end_event_streams`, `emit_stats`, `open_stats`, `end_stats`, `set_logs`, `emit_log`, the `flood` exec, `hold_pulls`/`release_pulls`, and `resizes`/`builds`/`pulls`.
    - Constructors `event()`, `sample()` and `line()`.
    - `TestApp::serve() -> SocketAddr`, `ws_connect` and `Ws{open, send, send_text, send_binary, frame, json, until, quiet}`.
    - Helpers `ws_sub(n, topic, id, opts)`, `is(v, sub, type)`, `eventually(what, cond)` and `ws_config()`, which sets 10 ms backoff and 50 ms session checks.
- Design notes:
  - Each connection has a reader and a writer. The reader owns the subscriptions; the writer drains the control queue plus one message per subscription per round, so no subscription can starve the others.
  - A queue whose producer called `finish()` is removed once drained, which frees its sub id.
  - The events hub keeps one Docker stream while anyone listens, and checks every second whether anyone still does.
  - Before each reconnect the hub checks `env.status()`, and it announces only up/down *transitions*.
  - Hubs are keyed by `(env id, Arc pointer)`. An environment re-created under the same name, or given a new URL, is a new instance with fresh streams.

- [ ] **Step 1: Include `StreamOps` in `DockerOps` and give the fake streams**

Workspace `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -17,7 +17,7 @@ anyhow = "1"
 argon2 = { version = "0.5", features = ["std"] }
 async-trait = "0.1"
 bollard = "0.21"
-axum = "0.8"
+axum = { version = "0.8", features = ["ws"] }
 axum-server = { version = "0.7", features = ["tls-rustls-no-provider"] }
 clap = { version = "4", features = ["derive", "env"] }
 futures-util = "0.3"
@@ -36,6 +36,7 @@ subtle = "2"
 tar = "0.4"
 tempfile = "3"
 thiserror = "2"
+tokio-tungstenite = "0.29"
 tokio = { version = "1", features = ["full"] }
 tower = { version = "0.5", features = ["util"] }
 tracing = "0.1"
```

`crates/fleetops-api/Cargo.toml`:

```diff
--- a/crates/fleetops-api/Cargo.toml
+++ b/crates/fleetops-api/Cargo.toml
@@ -23,4 +23,5 @@ uuid.workspace = true
 [dev-dependencies]
 async-trait.workspace = true
 http-body-util.workspace = true
+tokio-tungstenite.workspace = true
 tower.workspace = true
```

`crates/fleetops-core/src/docker.rs`:

```diff
--- a/crates/fleetops-core/src/docker.rs
+++ b/crates/fleetops-core/src/docker.rs
@@ -346,6 +346,12 @@ pub trait NetworkOps: Send + Sync {
 }
 
 /// Everything a Docker-kind environment can do.
-pub trait DockerOps: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps {}
+pub trait DockerOps:
+    SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps + crate::streams::StreamOps
+{
+}
 
-impl<T: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps> DockerOps for T {}
+impl<T> DockerOps for T where
+    T: SystemOps + ContainerOps + ImageOps + VolumeOps + NetworkOps + crate::streams::StreamOps
+{
+}
```

`crates/fleetops-api/tests/common/fake_docker.rs`. The fields go on `FakeDocker`; the rest is appended to the end of the file:

```diff
--- a/crates/fleetops-api/tests/common/fake_docker.rs
+++ b/crates/fleetops-api/tests/common/fake_docker.rs
@@ -25,6 +25,9 @@ pub struct FakeDocker {
     pub state: Mutex<FakeState>,
     /// When false, the environment reports Down and `docker()` is None.
     pub reachable: AtomicBool,
+    pub streams: Mutex<FakeStreams>,
+    pub hold_pulls: AtomicBool,
+    pub release_pulls: Arc<Notify>,
 }
 
 fn not_found(kind: &str, id: &str) -> Error {
@@ -407,3 +410,265 @@ impl EnvFactory for FakeFactory {
         }))
     }
 }
+
+// ---- streams ---------------------------------------------------------------
+
+use fleetops_core::streams::{
+    BoxStream, BuildContext, BuildSpec, DockerEvent, ExecOptions, ExecSession, LogLine, LogOptions,
+    LogStream, StatsSample, StreamOps,
+};
+use futures_util::{stream, StreamExt};
+use std::collections::HashMap;
+use std::sync::atomic::AtomicUsize;
+use tokio::sync::{mpsc, Notify};
+
+type Tx<T> = mpsc::UnboundedSender<Result<T>>;
+
+/// Hand-driven streams: tests push events, stats and log lines and can end
+/// streams to simulate the daemon going away.
+#[derive(Default)]
+pub struct FakeStreams {
+    events: Vec<Tx<DockerEvent>>,
+    stats: Vec<(String, Tx<StatsSample>)>,
+    logs: HashMap<String, Vec<LogLine>>,
+    follow: Vec<(String, Tx<LogLine>)>,
+    pub resizes: Vec<(String, u16, u16)>,
+    pub builds: Vec<BuildSpec>,
+    pub pulls: Vec<String>,
+    execs: usize,
+}
+
+fn rx_stream<T: Send + 'static>(rx: mpsc::UnboundedReceiver<Result<T>>) -> BoxStream<T> {
+    Box::pin(stream::unfold(rx, |mut rx| async move {
+        rx.recv().await.map(|v| (v, rx))
+    }))
+}
+
+pub fn event(id: &str, action: &str) -> DockerEvent {
+    DockerEvent {
+        kind: "container".into(),
+        action: action.into(),
+        id: id.into(),
+        name: Some(format!("{id}-name")),
+        attributes: Default::default(),
+        time: 1,
+    }
+}
+
+pub fn sample(cpu: f64) -> StatsSample {
+    StatsSample {
+        time: 1,
+        cpu_percent: cpu,
+        memory_used: 10,
+        memory_limit: 100,
+        net_rx: 0,
+        net_tx: 0,
+        block_read: 0,
+        block_write: 0,
+        pids: 1,
+    }
+}
+
+impl FakeDocker {
+    fn streams(&self) -> std::sync::MutexGuard<'_, FakeStreams> {
+        self.streams.lock().unwrap()
+    }
+
+    pub fn emit_event(&self, e: DockerEvent) {
+        self.streams()
+            .events
+            .retain(|tx| tx.send(Ok(e.clone())).is_ok());
+    }
+
+    /// Event streams currently open (not yet dropped by their consumer).
+    pub fn open_event_streams(&self) -> usize {
+        let mut s = self.streams();
+        s.events.retain(|tx| !tx.is_closed());
+        s.events.len()
+    }
+
+    /// Ends every open events stream, as a daemon restart would.
+    pub fn end_event_streams(&self) {
+        self.streams().events.clear();
+    }
+
+    pub fn emit_stats(&self, id: &str, s: StatsSample) {
+        self.streams()
+            .stats
+            .retain(|(i, tx)| i != id || tx.send(Ok(s.clone())).is_ok());
+    }
+
+    pub fn open_stats(&self, id: &str) -> usize {
+        let mut s = self.streams();
+        s.stats.retain(|(_, tx)| !tx.is_closed());
+        s.stats.iter().filter(|(i, _)| i == id).count()
+    }
+
+    pub fn end_stats(&self, id: &str) {
+        self.streams().stats.retain(|(i, _)| i != id);
+    }
+
+    pub fn set_logs(&self, id: &str, lines: Vec<LogLine>) {
+        self.streams().logs.insert(id.into(), lines);
+    }
+
+    pub fn emit_log(&self, id: &str, line: LogLine) {
+        self.streams()
+            .follow
+            .retain(|(i, tx)| i != id || tx.send(Ok(line.clone())).is_ok());
+    }
+}
+
+pub fn line(stream: LogStream, text: &str) -> LogLine {
+    LogLine {
+        stream,
+        text: text.into(),
+    }
+}
+
+/// Chunks pulled from the `flood` exec so far, across all sessions.
+pub static FLOOD_CHUNKS: AtomicUsize = AtomicUsize::new(0);
+pub const FLOOD_CHUNK: usize = 16 * 1024;
+
+#[async_trait]
+impl StreamOps for FakeDocker {
+    fn events(&self) -> BoxStream<DockerEvent> {
+        let (tx, rx) = mpsc::unbounded_channel();
+        self.streams().events.push(tx);
+        rx_stream(rx)
+    }
+
+    fn stats(&self, id: &str) -> BoxStream<StatsSample> {
+        let (tx, rx) = mpsc::unbounded_channel();
+        self.streams().stats.push((id.into(), tx));
+        rx_stream(rx)
+    }
+
+    fn logs(&self, id: &str, opts: &LogOptions) -> BoxStream<LogLine> {
+        let mut preset: Vec<LogLine> = self
+            .streams()
+            .logs
+            .get(id)
+            .cloned()
+            .unwrap_or_default()
+            .into_iter()
+            .filter(|l| match l.stream {
+                LogStream::Stdout => opts.stdout,
+                LogStream::Stderr => opts.stderr,
+            })
+            .collect();
+        if let Some(n) = opts.tail {
+            let skip = preset.len().saturating_sub(n as usize);
+            preset.drain(..skip);
+        }
+        let head = stream::iter(preset.into_iter().map(Ok));
+        if opts.follow {
+            let (tx, rx) = mpsc::unbounded_channel();
+            self.streams().follow.push((id.into(), tx));
+            Box::pin(head.chain(rx_stream(rx)))
+        } else {
+            Box::pin(head)
+        }
+    }
+
+    /// Echoes input back as `echo:<input>`; `exit\n` ends the session with
+    /// code 0. The command `flood` streams output forever, lazily.
+    async fn exec_start(&self, id: &str, opts: &ExecOptions) -> Result<ExecSession> {
+        self.container(id)
+            .ok_or_else(|| not_found("container", id))?;
+        let exec_id = {
+            let mut s = self.streams();
+            s.execs += 1;
+            format!("exec{}", s.execs)
+        };
+        self.call(format!("exec {id} {}", opts.command().join(" ")));
+        let (input, mut input_rx) = mpsc::channel::<Vec<u8>>(64);
+        if opts.cmd.as_deref() == Some(&["flood".to_string()][..]) {
+            let output = stream::repeat_with(|| {
+                FLOOD_CHUNKS.fetch_add(1, Ordering::SeqCst);
+                Ok(vec![b'x'; FLOOD_CHUNK])
+            });
+            return Ok(ExecSession {
+                exec_id,
+                output: Box::pin(output),
+                input,
+            });
+        }
+        let (out_tx, out_rx) = mpsc::unbounded_channel();
+        let _ = out_tx.send(Ok(b"$ ".to_vec()));
+        tokio::spawn(async move {
+            while let Some(buf) = input_rx.recv().await {
+                if buf == b"exit\n" {
+                    break;
+                }
+                let mut echo = b"echo:".to_vec();
+                echo.extend(buf);
+                if out_tx.send(Ok(echo)).is_err() {
+                    break;
+                }
+            }
+        });
+        Ok(ExecSession {
+            exec_id,
+            output: rx_stream(out_rx),
+            input,
+        })
+    }
+
+    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> Result<()> {
+        self.streams().resizes.push((exec_id.into(), cols, rows));
+        Ok(())
+    }
+
+    async fn exec_exit_code(&self, _exec_id: &str) -> Result<Option<i64>> {
+        Ok(Some(0))
+    }
+
+    /// `missing` fails with Docker's 404. While `hold_pulls` is set, a pull
+    /// sends its first message and then waits for `release_pulls`.
+    fn pull_image(&self, image: &str, tag: &str) -> BoxStream<Value> {
+        let reference = format!("{image}:{tag}");
+        self.streams().pulls.push(reference.clone());
+        let hold = self.hold_pulls.load(Ordering::SeqCst);
+        let release = self.release_pulls.clone();
+        let missing = image == "missing";
+        Box::pin(
+            stream::iter([Ok(json!({"status": format!("Pulling from {image}")}))]).chain(
+                stream::once(async move {
+                    if hold {
+                        release.notified().await;
+                    }
+                    if missing {
+                        vec![Err(Error::Upstream {
+                            status: 404,
+                            message: "manifest unknown".into(),
+                        })]
+                    } else {
+                        vec![
+                            Ok(json!({"status": "Download complete", "id": "layer1"})),
+                            Ok(json!({"status": format!("Status: Downloaded newer image for {reference}")})),
+                        ]
+                    }
+                })
+                .flat_map(stream::iter),
+            ),
+        )
+    }
+
+    /// A Dockerfile containing `fail` fails the way a failing RUN step does.
+    fn build_image(&self, spec: BuildSpec) -> BoxStream<Value> {
+        let fails = matches!(&spec.context, BuildContext::Dockerfile(d) if String::from_utf8_lossy(d).contains("fail"));
+        self.streams().builds.push(spec);
+        let tail = if fails {
+            Ok(
+                json!({"errorDetail": {"message": "The command '/bin/sh -c fail' returned a non-zero code: 1"}}),
+            )
+        } else {
+            Ok(json!({"aux": {"ID": "sha256:built"}}))
+        };
+        Box::pin(stream::iter([
+            Ok(json!({"stream": "Step 1/1 : FROM alpine\n"})),
+            tail,
+        ]))
+    }
+}
```

Run: `cargo test --workspace`
Expected: everything still PASSES. `LocalDocker` has implemented `StreamOps` since Task 2, and the fake does now.

- [ ] **Step 2: Write the failing tests**

Append the websocket helpers to `crates/fleetops-api/tests/common/mod.rs`:

```diff
--- a/crates/fleetops-api/tests/common/mod.rs
+++ b/crates/fleetops-api/tests/common/mod.rs
@@ -184,3 +184,150 @@ impl TestApp {
         .await
     }
 }
+
+// ---- websocket ---------------------------------------------------------------
+
+use futures_util::{SinkExt, StreamExt};
+use tokio_tungstenite::tungstenite::client::IntoClientRequest;
+pub use tokio_tungstenite::tungstenite::Message as WsMessage;
+
+pub type WsStream =
+    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
+
+/// How long a websocket test waits for any one frame.
+pub const WS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
+
+impl TestApp {
+    /// Serves the app on an ephemeral port; websockets need a real socket.
+    pub async fn serve(&self) -> SocketAddr {
+        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
+        let addr = listener.local_addr().unwrap();
+        let app = self.router.clone();
+        tokio::spawn(async move {
+            axum::serve(
+                listener,
+                app.into_make_service_with_connect_info::<SocketAddr>(),
+            )
+            .await
+            .unwrap();
+        });
+        addr
+    }
+}
+
+/// Opens `/api/v1/ws` as `session`, with an optional Origin header.
+pub async fn ws_connect(
+    addr: SocketAddr,
+    session: Option<&Session>,
+    origin: Option<&str>,
+) -> Result<WsStream, tokio_tungstenite::tungstenite::Error> {
+    let mut r = format!("ws://{addr}/api/v1/ws")
+        .into_client_request()
+        .unwrap();
+    if let Some(s) = session {
+        r.headers_mut().insert("cookie", s.cookie.parse().unwrap());
+    }
+    if let Some(o) = origin {
+        r.headers_mut().insert("origin", o.parse().unwrap());
+    }
+    tokio_tungstenite::connect_async(r).await.map(|(ws, _)| ws)
+}
+
+pub struct Ws(pub WsStream);
+
+impl Ws {
+    pub async fn open(addr: SocketAddr, session: &Session) -> Ws {
+        Ws(ws_connect(addr, Some(session), None)
+            .await
+            .expect("ws connect"))
+    }
+
+    pub async fn send(&mut self, v: Value) {
+        self.0
+            .send(WsMessage::Text(v.to_string().into()))
+            .await
+            .unwrap();
+    }
+
+    pub async fn send_text(&mut self, t: &str) {
+        self.0
+            .send(WsMessage::Text(t.to_string().into()))
+            .await
+            .unwrap();
+    }
+
+    pub async fn send_binary(&mut self, b: Vec<u8>) {
+        self.0.send(WsMessage::Binary(b.into())).await.unwrap();
+    }
+
+    /// The next frame, failing the test after `WS_WAIT`. None once closed.
+    pub async fn frame(&mut self) -> Option<WsMessage> {
+        loop {
+            match tokio::time::timeout(WS_WAIT, self.0.next())
+                .await
+                .expect("websocket frame within WS_WAIT")
+            {
+                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
+                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return None,
+                Some(Ok(m)) => return Some(m),
+            }
+        }
+    }
+
+    /// The next JSON frame, skipping binary frames.
+    pub async fn json(&mut self) -> Value {
+        loop {
+            match self.frame().await.expect("socket closed") {
+                WsMessage::Text(t) => return serde_json::from_str(&t).unwrap(),
+                _ => continue,
+            }
+        }
+    }
+
+    /// JSON frames until one matches `pred`, which is returned.
+    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
+        loop {
+            let v = self.json().await;
+            if pred(&v) {
+                return v;
+            }
+        }
+    }
+
+    /// Asserts nothing arrives for `ms` milliseconds.
+    pub async fn quiet(&mut self, ms: u64) {
+        if let Ok(Some(Ok(m))) =
+            tokio::time::timeout(std::time::Duration::from_millis(ms), self.0.next()).await
+        {
+            panic!("expected silence, got {m:?}");
+        }
+    }
+}
+
+/// A `sub` frame for environment `local`.
+pub fn ws_sub(n: u32, topic: &str, id: Option<&str>, opts: Value) -> Value {
+    json!({"op": "sub", "sub": n, "topic": topic, "env": "local", "id": id, "opts": opts})
+}
+
+/// `{"sub": n, "type": t}`?
+pub fn is(v: &Value, sub: u32, t: &str) -> bool {
+    v["sub"] == sub && v["type"] == t
+}
+
+/// Polls `cond` for up to WS_WAIT.
+pub async fn eventually(what: &str, cond: impl Fn() -> bool) {
+    let start = std::time::Instant::now();
+    while !cond() {
+        assert!(start.elapsed() < WS_WAIT, "timed out waiting for {what}");
+        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
+    }
+}
+
+/// Websocket-friendly config: fast reconnects and session checks.
+pub fn ws_config() -> ApiConfig {
+    ApiConfig {
+        ws_backoff: std::time::Duration::from_millis(10),
+        ws_session_check: std::time::Duration::from_millis(50),
+        ..ApiConfig::default()
+    }
+}
```

`crates/fleetops-api/tests/ws.rs`:

```rust
mod common;

use common::*;
use serde_json::json;

#[tokio::test]
async fn websocket_requires_a_session_and_a_matching_origin() {
    let t = app().await;
    let admin = t.with_local().await;
    let addr = t.serve().await;
    let err = ws_connect(addr, None, None).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    let err = ws_connect(addr, Some(&admin), Some("http://evil.example"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
    let ok = format!("http://{addr}");
    assert!(ws_connect(addr, Some(&admin), Some(&ok)).await.is_ok());
}

#[tokio::test]
async fn bad_frames_get_errors_without_closing_the_socket() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send_text("not json").await;
    let e = ws.json().await;
    assert_eq!(
        (e["type"].as_str(), e["code"].as_str()),
        (Some("error"), Some("invalid"))
    );
    assert!(e["sub"].is_null());
    ws.send_binary(vec![1, 2]).await;
    assert_eq!(ws.json().await["code"], "invalid");
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events"}))
        .await;
    let e = ws.json().await;
    assert!(
        is(&e, 1, "error") && e["code"] == "invalid",
        "env is required: {e}"
    );
    ws.send(json!({"op": "sub", "sub": 2, "topic": "events", "env": "nope"}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 2, "error") && e["code"] == "not_found", "{e}");
    // Still usable.
    ws.send(json!({"op": "sub", "sub": 5, "topic": "events", "env": "local"}))
        .await;
    ws.quiet(100).await;
}

#[tokio::test]
async fn sub_ids_are_unique_per_connection_and_reusable_after_unsub() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    let sub = json!({"op": "sub", "sub": 9, "topic": "events", "env": "local"});
    ws.send(sub.clone()).await;
    ws.send(sub.clone()).await;
    let e = ws.json().await;
    assert!(is(&e, 9, "error") && e["code"] == "conflict", "{e}");
    ws.send(json!({"op": "unsub", "sub": 9})).await;
    ws.send(sub).await;
    ws.quiet(100).await;
}

#[tokio::test]
async fn unreachable_environment_is_env_unavailable() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake
        .reachable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events", "env": "local"}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 1, "error") && e["code"] == "env_unavailable", "{e}");
}

#[tokio::test]
async fn logging_out_closes_open_websockets() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    let r = t
        .send(req("POST", "/api/v1/auth/logout", None, Some(&admin)))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    let e = ws.json().await;
    assert_eq!(
        (e["type"].as_str(), e["code"].as_str()),
        (Some("error"), Some("unauthorized"))
    );
    assert!(ws.frame().await.is_none(), "socket closes after the error");
}

#[tokio::test]
async fn deleted_user_loses_the_websocket() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let r = t.create_user(&admin, "bob", "bob-password", "user").await;
    let bob_id = r.json["id"].as_i64().unwrap();
    let bob = session_from(&t.login("bob", "bob-password").await);
    let mut ws = Ws::open(t.serve().await, &bob).await;
    let r = t
        .send(req(
            "DELETE",
            &format!("/api/v1/users/{bob_id}"),
            None,
            Some(&admin),
        ))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    assert_eq!(ws.json().await["code"], "unauthorized");
}
```

`crates/fleetops-api/tests/ws_events.rs`:

```rust
mod common;

use common::fake_docker::event;
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn events_fan_out_from_one_shared_stream_and_filter_by_id() {
    let t = app().await;
    let admin = t.with_local().await;
    let addr = t.serve().await;
    let (mut a, mut b) = (Ws::open(addr, &admin).await, Ws::open(addr, &admin).await);
    a.send(ws_sub(1, "events", None, Value::Null)).await;
    b.send(ws_sub(1, "events", None, Value::Null)).await;
    b.send(ws_sub(2, "events", Some("c2"), Value::Null)).await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    // Subscriptions attach asynchronously; keep emitting until all see one.
    let fake = t.fake.clone();
    let emitter = tokio::spawn(async move {
        loop {
            fake.emit_event(event("c1", "start"));
            fake.emit_event(event("c2", "die"));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    for ws in [&mut a, &mut b] {
        let e = ws.until(|v| is(v, 1, "data")).await;
        assert_eq!(e["data"]["id"], "c1");
        assert_eq!(e["data"]["action"], "start");
        assert_eq!(e["data"]["name"], "c1-name");
    }
    for _ in 0..3 {
        let only_c2 = b.until(|v| is(v, 2, "data")).await;
        assert_eq!(only_c2["data"]["id"], "c2");
    }
    emitter.abort();
    assert_eq!(
        t.fake.open_event_streams(),
        1,
        "one Docker stream for everyone"
    );
    drop((a, b));
    eventually("events stream closed after last subscriber", || {
        t.fake.open_event_streams() == 0
    })
    .await;
}

#[tokio::test]
async fn dropped_event_stream_reports_down_then_up_and_resumes() {
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, Value::Null)).await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    t.fake.reachable.store(false, Ordering::SeqCst);
    t.fake.end_event_streams();
    let down = ws.until(|v| v["type"] == "env_status").await;
    assert_eq!(down["env"], "local");
    assert_eq!(down["status"]["state"], "down");
    t.fake.reachable.store(true, Ordering::SeqCst);
    let up = ws.until(|v| v["type"] == "env_status").await;
    assert_eq!(up["status"]["state"], "up");
    eventually("reconnected", || t.fake.open_event_streams() == 1).await;
    let fake = t.fake.clone();
    let emitter = tokio::spawn(async move {
        loop {
            fake.emit_event(event("c9", "create"));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let e = ws.until(|v| is(v, 1, "data")).await;
    emitter.abort();
    assert_eq!(e["data"]["id"], "c9", "the same subscription survives");
}

#[tokio::test]
async fn recreated_environment_gets_a_fresh_docker_stream() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, Value::Null)).await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    let r = t
        .send(req("DELETE", "/api/v1/envs/local", None, Some(&admin)))
        .await;
    assert_eq!(r.status, axum::http::StatusCode::NO_CONTENT);
    t.add_env(&admin, "local").await;
    ws.send(ws_sub(2, "events", None, Value::Null)).await;
    eventually("a second stream for the new instance", || {
        t.fake.open_event_streams() == 2
    })
    .await;
    ws.send(json!({"op": "unsub", "sub": 1})).await;
    eventually("old instance's stream released", || {
        t.fake.open_event_streams() == 1
    })
    .await;
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test ws --test ws_events`
Expected: FAIL to compile with `struct ApiConfig has no field named ws_backoff`.

- [ ] **Step 4: Implement**

`crates/fleetops-api/src/state.rs`:

```diff
--- a/crates/fleetops-api/src/state.rs
+++ b/crates/fleetops-api/src/state.rs
@@ -1,6 +1,8 @@
 use crate::{
     auth::{limiter::LoginLimiter, setup::SetupGuard},
     envs::EnvRegistry,
+    tasks::TaskManager,
+    ws::hub::Hubs,
 };
 use fleetops_core::env::EnvFactory;
 use fleetops_store::Store;
@@ -19,6 +21,13 @@ pub struct ApiConfig {
     /// Send HSTS on TLS responses. Off by default because the HTTP listener
     /// is always on and HSTS would break it for browsers.
     pub hsts: bool,
+    /// First reconnect delay for dropped Docker event streams; doubles up
+    /// to 30 s.
+    pub ws_backoff: Duration,
+    /// How often an open websocket re-checks its session.
+    pub ws_session_check: Duration,
+    /// Messages buffered per websocket subscription (spec default 1024).
+    pub ws_queue_capacity: usize,
 }
 
 impl Default for ApiConfig {
@@ -26,6 +35,9 @@ impl Default for ApiConfig {
         Self {
             setup_window: Duration::from_secs(600),
             hsts: false,
+            ws_backoff: Duration::from_secs(1),
+            ws_session_check: Duration::from_secs(30),
+            ws_queue_capacity: crate::ws::queue::QUEUE_CAPACITY,
         }
     }
 }
@@ -37,6 +49,8 @@ pub struct AppState {
     pub limiter: Arc<LoginLimiter>,
     pub setup: Arc<SetupGuard>,
     pub envs: Arc<EnvRegistry>,
+    pub hubs: Arc<Hubs>,
+    pub tasks: Arc<TaskManager>,
 }
 
 impl AppState {
@@ -49,6 +63,8 @@ impl AppState {
             limiter: Arc::new(LoginLimiter::new(LOGIN_MAX_FAILURES, LOGIN_WINDOW)),
             setup: Arc::new(setup),
             envs: Arc::new(EnvRegistry::new(factory)),
+            hubs: Arc::new(Hubs::default()),
+            tasks: Arc::new(TaskManager::default()),
         }
     }
 }
```

`crates/fleetops-api/src/ws/hub.rs`:

```rust
//! Shared Docker streams (spec §5.2 fan-out): one events stream per
//! environment and one stats stream per container, however many clients
//! watch them. Each stops when its last subscriber leaves.

use fleetops_core::env::{EnvStatus, Environment};
use fleetops_core::streams::{DockerEvent, StatsSample};
use futures_util::StreamExt;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::broadcast;

pub const EVENTS_BUFFER: usize = 1024;
pub const STATS_BUFFER: usize = 16;
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How often an idle events stream checks whether anyone still listens.
const IDLE_CHECK: Duration = Duration::from_secs(1);

/// An environment's status changed; every connection forwards it.
#[derive(Debug, Clone)]
pub struct StatusChange {
    pub env: String,
    pub status: Value,
}

pub struct EventHub {
    tx: broadcast::Sender<DockerEvent>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl EventHub {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(EVENTS_BUFFER).0,
            task: Mutex::new(None),
        }
    }

    /// Subscribes to `env`'s events, starting the shared stream if needed.
    pub fn subscribe(
        &self,
        env: Arc<dyn Environment>,
        status: broadcast::Sender<StatusChange>,
        backoff: Duration,
    ) -> broadcast::Receiver<DockerEvent> {
        let rx = self.tx.subscribe();
        let mut task = self.task.lock().unwrap();
        if task.as_ref().is_none_or(|t| t.is_finished()) {
            *task = Some(tokio::spawn(run_events(
                env,
                self.tx.clone(),
                status,
                backoff,
            )));
        }
        rx
    }
}

fn status_json(s: &EnvStatus) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// Keeps one events stream open while anyone listens, reconnecting with
/// exponential backoff and announcing up/down transitions.
async fn run_events(
    env: Arc<dyn Environment>,
    tx: broadcast::Sender<DockerEvent>,
    status: broadcast::Sender<StatusChange>,
    initial_backoff: Duration,
) {
    let mut backoff = initial_backoff;
    // Assume up: the first failure is a transition worth announcing.
    let mut up = true;
    let announce = |s: &EnvStatus| {
        let _ = status.send(StatusChange {
            env: env.name().to_string(),
            status: status_json(s),
        });
    };
    while tx.receiver_count() > 0 {
        let current = env.status().await;
        let ops = match (&current, env.docker()) {
            (EnvStatus::Up { .. }, Some(ops)) => ops,
            _ => {
                if up {
                    up = false;
                    announce(&current);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        if !up {
            up = true;
            announce(&current);
        }
        let mut events = ops.events();
        loop {
            tokio::select! {
                item = events.next() => match item {
                    Some(Ok(e)) => {
                        backoff = initial_backoff;
                        let _ = tx.send(e);
                    }
                    // Dropped: go round and reconnect.
                    Some(Err(_)) | None => break,
                },
                _ = tokio::time::sleep(IDLE_CHECK) => {
                    if tx.receiver_count() == 0 {
                        return;
                    }
                }
            }
        }
        if tx.receiver_count() > 0 {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

/// One container's shared stats stream. `None` on the channel means the
/// stream ended (the container stopped or the daemon went away).
pub struct SharedStats {
    tx: broadcast::Sender<Option<StatsSample>>,
    task: tokio::task::AbortHandle,
}

impl Drop for SharedStats {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SharedStats {
    pub fn subscribe(&self) -> broadcast::Receiver<Option<StatsSample>> {
        self.tx.subscribe()
    }
}

#[derive(Default)]
pub struct StatsHub {
    streams: Mutex<HashMap<String, Weak<SharedStats>>>,
}

impl StatsHub {
    /// A handle to `id`'s shared stats stream plus a receiver on it. The
    /// stream lives while any handle does.
    pub fn subscribe(
        &self,
        ops: Arc<dyn fleetops_core::docker::DockerOps>,
        id: &str,
    ) -> (Arc<SharedStats>, broadcast::Receiver<Option<StatsSample>>) {
        let mut map = self.streams.lock().unwrap();
        map.retain(|_, w| w.strong_count() > 0);
        if let Some(shared) = map.get(id).and_then(Weak::upgrade) {
            if !shared.task.is_finished() {
                let rx = shared.subscribe();
                return (shared, rx);
            }
        }
        let (tx, rx) = broadcast::channel(STATS_BUFFER);
        let producer = tx.clone();
        let mut stream = ops.stats(id);
        let task = tokio::spawn(async move {
            while let Some(Ok(sample)) = stream.next().await {
                let _ = producer.send(Some(sample));
            }
            let _ = producer.send(None);
        })
        .abort_handle();
        let shared = Arc::new(SharedStats { tx, task });
        map.insert(id.to_string(), Arc::downgrade(&shared));
        (shared, rx)
    }

    /// Open shared streams, for tests and metrics.
    pub fn open(&self) -> usize {
        self.streams
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.upgrade().is_some_and(|s| !s.task.is_finished()))
            .count()
    }
}

/// Identifies one live environment instance. Re-creating an environment
/// under the same name, or changing its URL, builds a new instance, so its
/// subscribers get a fresh Docker connection rather than the old one.
type EnvKey = (i64, usize);

fn key(env: &Arc<dyn Environment>) -> EnvKey {
    (env.id(), Arc::as_ptr(env) as *const () as usize)
}

/// Per-environment hubs.
#[derive(Default)]
pub struct Hubs {
    events: Mutex<HashMap<EnvKey, Arc<EventHub>>>,
    stats: Mutex<HashMap<EnvKey, Arc<StatsHub>>>,
    status: Mutex<Option<broadcast::Sender<StatusChange>>>,
}

impl Hubs {
    pub fn events(&self, env: &Arc<dyn Environment>) -> Arc<EventHub> {
        let mut map = self.events.lock().unwrap();
        // Forget hubs nobody listens to any more.
        map.retain(|_, h| h.tx.receiver_count() > 0);
        map.entry(key(env))
            .or_insert_with(|| Arc::new(EventHub::new()))
            .clone()
    }

    pub fn stats(&self, env: &Arc<dyn Environment>) -> Arc<StatsHub> {
        let mut map = self.stats.lock().unwrap();
        map.retain(|_, h| h.open() > 0);
        map.entry(key(env)).or_default().clone()
    }

    /// The bus every connection listens on for `env_status`.
    pub fn status(&self) -> broadcast::Sender<StatusChange> {
        self.status
            .lock()
            .unwrap()
            .get_or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }
}
```

`crates/fleetops-api/src/ws/topics.rs`. Tasks 6–9 extend this file:

```rust
//! The five subscription topics. Each resolves and authorises what it
//! needs, starts a producer task that fills the subscription's queue, and
//! returns the subscription.

use super::conn::Sub;
use super::protocol;
use super::queue::{Policy, SubQueue};
use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    AppState,
};
use fleetops_core::docker::DockerOps;
use fleetops_core::env::Environment;
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{broadcast::error::RecvError, Notify};

/// What every topic needs from the connection.
pub(super) struct Ctx<'a> {
    pub s: &'a AppState,
    pub cu: &'a CurrentUser,
    pub sub: u32,
    pub ready: Arc<Notify>,
}

impl Ctx<'_> {
    fn queue(&self, policy: Policy) -> Arc<SubQueue> {
        let cap = self.s.config.ws_queue_capacity.max(2);
        Arc::new(SubQueue::new(self.sub, policy, cap, self.ready.clone()))
    }
}

fn required<'a>(v: &'a Option<String>, what: &str) -> Result<&'a str> {
    v.as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Invalid(format!("{what} is required")))
}

/// Resolves the environment, authorises, and returns its Docker ops.
fn docker_for(
    s: &AppState,
    cu: &CurrentUser,
    env: &Option<String>,
    action: Action,
) -> Result<(Arc<dyn Environment>, Arc<dyn DockerOps>)> {
    let name = required(env, "env")?;
    let env = s.envs.get(name)?;
    authorize(&cu.user, Some(env.id()), action)?;
    let ops = env
        .docker()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok((env, ops))
}

/// Docker events for the environment, optionally only those whose actor id
/// or name is `id`. One shared Docker stream per environment.
pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, _) = docker_for(s, cu, &env, Action::DockerRead)?;
    let q = c.queue(Policy::DropOldest);
    let mut rx = s
        .hubs
        .events(&env)
        .subscribe(env, s.hubs.status(), s.config.ws_backoff);
    let filter = id.filter(|i| !i.is_empty());
    let qq = q.clone();
    let task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(e) => {
                    if filter
                        .as_ref()
                        .is_some_and(|f| *f != e.id && e.name.as_ref() != Some(f))
                    {
                        continue;
                    }
                    let v = serde_json::to_value(&e).unwrap_or(Value::Null);
                    if !qq.push(protocol::data(sub, v), 1) {
                        return;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    qq.finish(protocol::end(sub, "closed", Value::Null));
                    return;
                }
            }
        }
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
    })
}
```

`crates/fleetops-api/src/ws/conn.rs`. Topics that do not exist yet answer `topic … is not available yet`, and exec input and resize answer `not_found` until Task 8:

```rust
//! One websocket connection: a reader that handles client frames and owns
//! the subscriptions, and a writer that drains every subscription's queue
//! fairly onto the socket.

use super::protocol::{self, decode_binary, ClientMsg, Out, Topic};
use super::queue::{Policy, SubQueue};
use super::topics;
use crate::{
    auth::{extract::CurrentUser, SESSION_IDLE_SECS, SESSION_TOUCH_SECS},
    authz::{authorize, Action},
    AppState,
};
use axum::extract::ws::{Message, WebSocket};
use fleetops_core::{now_unix, Error, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{broadcast::error::RecvError, Notify};

/// Subscriptions one connection may hold at once.
pub const MAX_SUBS: usize = 256;
/// Largest client frame accepted.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// A live subscription. Dropping it (unsub, or the connection closing)
/// stops its producer.
pub(super) struct Sub {
    pub queue: Arc<SubQueue>,
    pub task: tokio::task::AbortHandle,
}

impl Drop for Sub {
    fn drop(&mut self) {
        self.queue.close();
        self.task.abort();
    }
}

/// State shared by reader and writer.
struct Shared {
    subs: Mutex<HashMap<u32, Sub>>,
    /// Connection-level messages: errors and env_status.
    control: Arc<SubQueue>,
    ready: Arc<Notify>,
    closing: AtomicBool,
}

impl Shared {
    fn say(&self, msg: Out) {
        self.control.push(msg, 1);
    }

    fn fail(&self, sub: Option<u32>, e: &Error) {
        self.say(protocol::error(sub, e.code(), &e.to_string()));
    }
}

pub async fn run(s: AppState, cu: CurrentUser, token_hash: String, socket: WebSocket) {
    let (mut sink, mut incoming) = socket.split();
    let ready = Arc::new(Notify::new());
    let shared = Arc::new(Shared {
        subs: Mutex::new(HashMap::new()),
        control: Arc::new(SubQueue::new(0, Policy::DropOldest, 256, ready.clone())),
        ready,
        closing: AtomicBool::new(false),
    });

    let gone = Arc::new(Notify::new());
    let writer = {
        let shared = shared.clone();
        let gone = gone.clone();
        tokio::spawn(async move {
            write_loop(&shared, &mut sink).await;
            gone.notify_one();
        })
    };

    let mut status = s.hubs.status().subscribe();
    let mut check = tokio::time::interval(s.config.ws_session_check);
    check.tick().await;
    loop {
        tokio::select! {
            frame = incoming.next() => match frame {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(msg) => handle(&s, &cu, &shared, msg).await,
                    Err(e) => shared.fail(None, &Error::Invalid(format!("bad frame: {e}"))),
                },
                Some(Ok(Message::Binary(b))) => match decode_binary(&b) {
                    Some((sub, payload)) => input(&shared, sub, payload.to_vec()),
                    None => shared.fail(None, &Error::Invalid("binary frame shorter than 4 bytes".into())),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            change = status.recv() => match change {
                Ok(c) => {
                    if authorize(&cu.user, None, Action::EnvRead).is_ok() {
                        shared.say(protocol::env_status(&c.env, &c.status));
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {}
            },
            _ = check.tick() => {
                if !session_still_valid(&s, &token_hash, cu.user.id).await {
                    shared.fail(None, &Error::Unauthorized("session ended".into()));
                    break;
                }
            }
            // The writer stopped: the socket is gone.
            _ = gone.notified() => break,
        }
    }
    // Stop producers, flush what is queued for the client, then close.
    shared.subs.lock().unwrap().clear();
    shared.closing.store(true, Ordering::SeqCst);
    shared.ready.notify_one();
    let abort = writer.abort_handle();
    if tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .is_err()
    {
        abort.abort();
    }
}

/// Sends queued messages until the socket fails or the connection closes.
async fn write_loop<S>(shared: &Shared, sink: &mut S)
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    loop {
        let wake = shared.ready.notified();
        let mut sent = false;
        if let Some(m) = shared.control.pop() {
            if send(sink, m).await.is_err() {
                return;
            }
            sent = true;
        }
        // One message per subscription per round keeps it fair.
        let queues: Vec<_> = shared
            .subs
            .lock()
            .unwrap()
            .values()
            .map(|s| s.queue.clone())
            .collect();
        for q in queues {
            if let Some(m) = q.pop() {
                if send(sink, m).await.is_err() {
                    return;
                }
                sent = true;
            }
            if q.drained() {
                let mut subs = shared.subs.lock().unwrap();
                if subs
                    .get(&q.sub())
                    .is_some_and(|s| Arc::ptr_eq(&s.queue, &q))
                {
                    subs.remove(&q.sub());
                }
            }
        }
        if !sent {
            if shared.closing.load(Ordering::SeqCst) {
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
            wake.await;
        }
    }
}

async fn send<S>(sink: &mut S, m: Out) -> std::result::Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    sink.send(match m {
        Out::Text(t) => Message::Text(t.into()),
        Out::Binary(b) => Message::Binary(b.into()),
    })
    .await
}

/// Re-checks the session behind a long-lived connection, so logging out
/// or revoking a session also ends its websockets. Keeps the session from
/// idling out while a tab is open.
async fn session_still_valid(s: &AppState, token_hash: &str, user_id: i64) -> bool {
    let now = now_unix();
    let Ok(Some(session)) = s.store.find_session(token_hash).await else {
        return false;
    };
    if session.user_id != user_id || !session.is_active(now, SESSION_IDLE_SECS) {
        return false;
    }
    if now - session.last_seen_at >= SESSION_TOUCH_SECS {
        let _ = s.store.touch_session(session.id, now).await;
    }
    s.store.get_user(user_id).await.is_ok()
}

/// Exec input; until the exec topic exists no subscription takes any.
fn input(shared: &Shared, sub: u32, _data: Vec<u8>) {
    shared.fail(
        Some(sub),
        &Error::NotFound(format!("no exec subscription {sub}")),
    );
}

async fn handle(s: &AppState, cu: &CurrentUser, shared: &Arc<Shared>, msg: ClientMsg) {
    match msg {
        ClientMsg::Sub {
            sub,
            topic,
            env,
            id,
            opts,
        } => {
            {
                let subs = shared.subs.lock().unwrap();
                if subs.contains_key(&sub) {
                    drop(subs);
                    return shared
                        .fail(Some(sub), &Error::Conflict(format!("sub {sub} is in use")));
                }
                if subs.len() >= MAX_SUBS {
                    drop(subs);
                    return shared.fail(
                        Some(sub),
                        &Error::Conflict(format!(
                            "at most {MAX_SUBS} subscriptions per connection"
                        )),
                    );
                }
            }
            match start(s, cu, shared, sub, (topic, env, id, opts)).await {
                Ok(entry) => {
                    shared.subs.lock().unwrap().insert(sub, entry);
                }
                Err(e) => shared.fail(Some(sub), &e),
            }
        }
        ClientMsg::Unsub { sub } => {
            shared.subs.lock().unwrap().remove(&sub);
        }
        ClientMsg::Input { sub, data } => input(shared, sub, data.into_bytes()),
        ClientMsg::Resize { sub, .. } => shared.fail(
            Some(sub),
            &Error::NotFound(format!("no exec subscription {sub}")),
        ),
    }
}

async fn start(
    s: &AppState,
    cu: &CurrentUser,
    shared: &Shared,
    sub: u32,
    msg: (Topic, Option<String>, Option<String>, Value),
) -> Result<Sub> {
    let (topic, env, id, _opts) = msg;
    let c = topics::Ctx {
        s,
        cu,
        sub,
        ready: shared.ready.clone(),
    };
    match topic {
        Topic::Events => topics::events(&c, env, id).await,
        other => Err(Error::Invalid(format!(
            "topic {other:?} is not available yet"
        ))),
    }
}
```

Replace `crates/fleetops-api/src/ws/mod.rs` with:

```rust
//! `/api/v1/ws`: one multiplexed websocket per browser tab (spec §5.2).

pub mod conn;
pub mod hub;
pub mod protocol;
pub mod queue;
pub mod topics;

use crate::{
    auth::{cookie::read_cookie, cookie::SESSION_COOKIE, extract::CurrentUser, token::hash_token},
    ApiError, AppState,
};
use axum::{
    extract::{State, WebSocketUpgrade},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};
use fleetops_core::Error;

/// Browsers always send `Origin` on websocket requests; it must name this
/// server, so other sites cannot ride the session cookie. Non-browser
/// clients may omit it.
pub fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin.to_str().ok().and_then(|o| o.split_once("://")), host) {
        (Some((_, authority)), Some(host)) => authority.eq_ignore_ascii_case(host),
        _ => false,
    }
}

pub async fn handler(
    State(s): State<AppState>,
    cu: CurrentUser,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers) {
        return ApiError(Error::Forbidden(
            "websocket Origin does not match this server".into(),
        ))
        .into_response();
    }
    // CurrentUser already validated this cookie.
    let token_hash = read_cookie(&headers, SESSION_COOKIE)
        .map(|t| hash_token(&t))
        .unwrap_or_default();
    ws.max_message_size(conn::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| conn::run(s, cu, token_hash, socket))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, v.parse().unwrap());
        }
        m
    }

    #[test]
    fn origin_must_match_host() {
        assert!(origin_allowed(&h(&[("host", "fleet.lan:9000")])));
        assert!(origin_allowed(&h(&[
            ("host", "fleet.lan:9000"),
            ("origin", "http://fleet.lan:9000")
        ])));
        assert!(origin_allowed(&h(&[
            ("host", "Fleet.LAN:9443"),
            ("origin", "https://fleet.lan:9443")
        ])));
        for bad in [
            "http://evil.example",
            "http://fleet.lan:9001",
            "null",
            "fleet.lan:9000",
        ] {
            assert!(
                !origin_allowed(&h(&[("host", "fleet.lan:9000"), ("origin", bad)])),
                "{bad}"
            );
        }
        assert!(!origin_allowed(&h(&[("origin", "http://fleet.lan:9000")])));
    }
}
```

`crates/fleetops-api/src/routes/mod.rs`:

```diff
--- a/crates/fleetops-api/src/routes/mod.rs
+++ b/crates/fleetops-api/src/routes/mod.rs
@@ -27,6 +27,7 @@ pub fn api() -> Router<AppState> {
         .route("/users", get(users::list).post(users::create))
         .route("/users/{id}", patch(users::update).delete(users::remove))
         .route("/audit", get(audit_log::list))
+        .route("/ws", get(crate::ws::handler))
         .merge(env_routes())
         .fallback(not_found)
 }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `ws` 6 PASS, `ws_events` 3 PASS, the `origin_must_match_host` unit test PASSES, and every earlier API test still PASSES. Lint is clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-core crates/fleetops-api
git commit -m "feat(api): /ws endpoint with shared events hub, env_status and session re-checks"
```

---

### Task 6: The `stats` topic

**Files:**
- Modify: `crates/fleetops-api/src/ws/topics.rs`, `crates/fleetops-api/src/ws/conn.rs`
- Test: `crates/fleetops-api/tests/ws_stats.rs`

**Interfaces:**
- Consumes: `StatsHub` (Task 5) and `StreamOps::stats` (Task 1).
- Produces:
  - `topics::stats(&Ctx, env, id)`, which needs `DockerRead`.
  - Subscribers get `data` for each `StatsSample`, and `end` with reason `ended` when the container stops.
  - Each subscription task holds an `Arc<SharedStats>`, so the shared Docker stream stops when the last subscription goes.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-api/tests/ws_stats.rs`:

```rust
mod common;

use common::fake_docker::sample;
use common::*;
use serde_json::{json, Value};

#[tokio::test]
async fn stats_streams_are_shared_per_container_and_released() {
    let t = app().await;
    let admin = t.with_local().await;
    let addr = t.serve().await;
    let (mut a, mut b) = (Ws::open(addr, &admin).await, Ws::open(addr, &admin).await);
    a.send(ws_sub(1, "stats", Some("c1"), Value::Null)).await;
    b.send(ws_sub(7, "stats", Some("c1"), Value::Null)).await;
    eventually("stats stream", || t.fake.open_stats("c1") == 1).await;
    let fake = t.fake.clone();
    let emitter = tokio::spawn(async move {
        loop {
            fake.emit_stats("c1", sample(12.5));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    assert_eq!(
        a.until(|v| is(v, 1, "data")).await["data"]["cpu_percent"],
        12.5
    );
    assert_eq!(
        b.until(|v| is(v, 7, "data")).await["data"]["memory_limit"],
        100
    );
    emitter.abort();
    assert_eq!(t.fake.open_stats("c1"), 1, "shared across subscribers");
    a.send(json!({"op": "unsub", "sub": 1})).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(t.fake.open_stats("c1"), 1, "still one subscriber");
    b.send(json!({"op": "unsub", "sub": 7})).await;
    eventually("stats stream released", || t.fake.open_stats("c1") == 0).await;
}

#[tokio::test]
async fn stats_end_when_the_container_stops() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(3, "stats", Some("c1"), Value::Null)).await;
    eventually("stats stream", || t.fake.open_stats("c1") == 1).await;
    t.fake.end_stats("c1");
    let end = ws.until(|v| is(v, 3, "end")).await;
    assert_eq!(end["reason"], "ended");
    // The id is free again.
    ws.send(ws_sub(3, "stats", Some("c1"), Value::Null)).await;
    eventually("stats stream again", || t.fake.open_stats("c1") == 1).await;
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test ws_stats`
Expected: both FAIL with `timed out waiting for stats stream`, because the subscription is refused with `topic Stats is not available yet`.

- [ ] **Step 3: Implement**

Append to `crates/fleetops-api/src/ws/topics.rs`:

```diff
--- a/crates/fleetops-api/src/ws/topics.rs
+++ b/crates/fleetops-api/src/ws/topics.rs
@@ -94,3 +94,35 @@ pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>)
         task: task.abort_handle(),
     })
 }
+
+/// Stats samples for container `id`, shared with every other subscriber.
+pub(super) async fn stats(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
+    let (s, cu, sub) = (c.s, c.cu, c.sub);
+    let (env, ops) = docker_for(s, cu, &env, Action::DockerRead)?;
+    let id = required(&id, "id")?.to_string();
+    let (handle, mut rx) = s.hubs.stats(&env).subscribe(ops, &id);
+    let q = c.queue(Policy::DropOldest);
+    let qq = q.clone();
+    let task = tokio::spawn(async move {
+        let _keep_stream_alive = handle;
+        loop {
+            match rx.recv().await {
+                Ok(Some(sample)) => {
+                    let v = serde_json::to_value(&sample).unwrap_or(Value::Null);
+                    if !qq.push(protocol::data(sub, v), 1) {
+                        return;
+                    }
+                }
+                Err(RecvError::Lagged(_)) => continue,
+                Ok(None) | Err(RecvError::Closed) => {
+                    qq.finish(protocol::end(sub, "ended", Value::Null));
+                    return;
+                }
+            }
+        }
+    });
+    Ok(Sub {
+        queue: q,
+        task: task.abort_handle(),
+    })
+}
```

Add the arm to `start` in `crates/fleetops-api/src/ws/conn.rs`:

```diff
--- a/crates/fleetops-api/src/ws/conn.rs
+++ b/crates/fleetops-api/src/ws/conn.rs
@@ -276,6 +276,7 @@ async fn start(
     };
     match topic {
         Topic::Events => topics::events(&c, env, id).await,
+        Topic::Stats => topics::stats(&c, env, id).await,
         other => Err(Error::Invalid(format!(
             "topic {other:?} is not available yet"
         ))),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `ws_stats` 2 PASS, everything else still PASSES, lint clean.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): shared, ref-counted stats over the websocket"
```

---

### Task 7: The `logs` topic with batching and `gap`

**Files:**
- Modify: `crates/fleetops-api/src/ws/topics.rs`, `crates/fleetops-api/src/ws/conn.rs`
- Test: `crates/fleetops-api/tests/ws_logs.rs`

**Interfaces:**
- Consumes: `StreamOps::logs` and `LogOptions` (Task 1), and `Policy::Gap` (Task 3).
- Produces:
  - `topics::logs(&Ctx, env, id, opts)`, which needs `DockerRead`. `opts` is a `LogOptions` JSON object; `null` means the defaults.
  - Each `data` message is an array of `{stream, text}`, flushed every `LOG_FLUSH` (50 ms) or at `LOG_BATCH_MAX` (500) lines, whichever comes first.
  - The subscription ends with `end` reason `eof`, or with `error` if the stream fails.
  - When the queue is full, lines are dropped and reported as `gap`. The producer never waits.
  - Constants `topics::LOG_FLUSH` and `topics::LOG_BATCH_MAX`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-api/tests/ws_logs.rs`:

```rust
mod common;

use common::fake_docker::line;
use common::*;
use fleetops_core::streams::LogStream;
use serde_json::{json, Value};

fn lines_in(v: &Value) -> Vec<String> {
    v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn logs_arrive_in_batches_then_end() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.set_logs(
        "c1",
        vec![
            line(LogStream::Stdout, "one"),
            line(LogStream::Stderr, "two"),
            line(LogStream::Stdout, "three"),
        ],
    );
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(
        1,
        "logs",
        Some("c1"),
        json!({"follow": false, "tail": null}),
    ))
    .await;
    let batch = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(lines_in(&batch), ["one", "two", "three"], "one batch");
    assert_eq!(batch["data"][1]["stream"], "stderr");
    assert_eq!(ws.json().await["type"], "end");
    ws.send(ws_sub(
        2,
        "logs",
        Some("c1"),
        json!({"tail": 1, "stderr": false}),
    ))
    .await;
    assert_eq!(lines_in(&ws.until(|v| is(v, 2, "data")).await), ["three"]);
    // Following: a burst of lines is batched, not sent one frame each.
    for i in 0..20 {
        t.fake
            .emit_log("c1", line(LogStream::Stdout, &format!("live-{i}")));
    }
    let mut got = Vec::new();
    let mut frames = 0;
    while got.len() < 20 {
        got.extend(lines_in(&ws.until(|v| is(v, 2, "data")).await));
        frames += 1;
    }
    assert_eq!(got.len(), 20);
    assert!(frames < 20, "20 lines took {frames} frames");
}

#[tokio::test]
async fn slow_log_reader_gets_a_gap_instead_of_stalling() {
    let t = app_with(fleetops_api::ApiConfig {
        ws_queue_capacity: 4,
        ..ws_config()
    })
    .await;
    let admin = t.with_local().await;
    const N: usize = 300_000;
    t.fake.set_logs(
        "c1",
        (0..N)
            .map(|i| line(LogStream::Stdout, &format!("line {i:06} of a long log")))
            .collect(),
    );
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(
        1,
        "logs",
        Some("c1"),
        json!({"follow": false, "tail": null}),
    ))
    .await;
    // Not reading: the queue fills and the producer must not wait.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (mut seen, mut dropped, mut gaps) = (0usize, 0u64, 0);
    loop {
        let v = ws.json().await;
        match v["type"].as_str() {
            Some("data") => seen += v["data"].as_array().unwrap().len(),
            Some("gap") => {
                gaps += 1;
                dropped += v["dropped"].as_u64().unwrap();
            }
            Some("end") => break,
            other => panic!("unexpected {other:?}: {v}"),
        }
    }
    assert!(gaps > 0, "a slow reader sees gap");
    assert_eq!(
        seen as u64 + dropped,
        N as u64,
        "every line is sent or counted"
    );
}

#[tokio::test]
async fn logs_subscription_is_validated() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(3, "logs", None, Value::Null)).await;
    let e = ws.json().await;
    assert!(is(&e, 3, "error") && e["message"].as_str().unwrap().contains("id is required"));
    ws.send(ws_sub(4, "logs", Some("c1"), json!({"tail": "x"})))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 4, "error") && e["code"] == "invalid", "{e}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test ws_logs`
Expected: all 3 FAIL, because every logs subscription is refused with `topic Logs is not available yet`.

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/ws/topics.rs`. The import block, the constants, `opts()` and `logs()` change:

```diff
--- a/crates/fleetops-api/src/ws/topics.rs
+++ b/crates/fleetops-api/src/ws/topics.rs
@@ -12,11 +12,18 @@ use crate::{
 };
 use fleetops_core::docker::DockerOps;
 use fleetops_core::env::Environment;
+use fleetops_core::streams::LogOptions;
 use fleetops_core::{Error, Result};
-use serde_json::Value;
-use std::sync::Arc;
+use futures_util::StreamExt;
+use serde_json::{json, Value};
+use std::{sync::Arc, time::Duration};
 use tokio::sync::{broadcast::error::RecvError, Notify};
 
+/// Logs are flushed to the client at most this often per subscription.
+pub const LOG_FLUSH: Duration = Duration::from_millis(50);
+/// Most lines in one logs `data` message.
+pub const LOG_BATCH_MAX: usize = 500;
+
 /// What every topic needs from the connection.
 pub(super) struct Ctx<'a> {
     pub s: &'a AppState,
@@ -38,6 +45,13 @@ fn required<'a>(v: &'a Option<String>, what: &str) -> Result<&'a str> {
         .ok_or_else(|| Error::Invalid(format!("{what} is required")))
 }
 
+fn opts<T: serde::de::DeserializeOwned + Default>(v: Value) -> Result<T> {
+    if v.is_null() {
+        return Ok(T::default());
+    }
+    serde_json::from_value(v).map_err(|e| Error::Invalid(format!("bad opts: {e}")))
+}
+
 /// Resolves the environment, authorises, and returns its Docker ops.
 fn docker_for(
     s: &AppState,
@@ -126,3 +140,58 @@ pub(super) async fn stats(c: &Ctx<'_>, env: Option<String>, id: Option<String>)
         task: task.abort_handle(),
     })
 }
+
+/// Container `id`'s logs as batches of lines, at most every `LOG_FLUSH`.
+pub(super) async fn logs(
+    c: &Ctx<'_>,
+    env: Option<String>,
+    id: Option<String>,
+    opts_v: Value,
+) -> Result<Sub> {
+    let (s, cu, sub) = (c.s, c.cu, c.sub);
+    let (_, ops) = docker_for(s, cu, &env, Action::DockerRead)?;
+    let id = required(&id, "id")?;
+    let o: LogOptions = opts(opts_v)?;
+    let mut lines = ops.logs(id, &o);
+    let q = c.queue(Policy::Gap);
+    let qq = q.clone();
+    let task = tokio::spawn(async move {
+        loop {
+            let first = match lines.next().await {
+                Some(Ok(l)) => l,
+                Some(Err(e)) => {
+                    return qq.finish(protocol::error(Some(sub), e.code(), &e.to_string()))
+                }
+                None => return qq.finish(protocol::end(sub, "eof", Value::Null)),
+            };
+            let mut batch = vec![first];
+            let deadline = tokio::time::Instant::now() + LOG_FLUSH;
+            let mut ended = None;
+            while batch.len() < LOG_BATCH_MAX {
+                match tokio::time::timeout_at(deadline, lines.next()).await {
+                    Ok(Some(Ok(l))) => batch.push(l),
+                    Ok(Some(Err(e))) => {
+                        ended = Some(protocol::error(Some(sub), e.code(), &e.to_string()));
+                        break;
+                    }
+                    Ok(None) => {
+                        ended = Some(protocol::end(sub, "eof", Value::Null));
+                        break;
+                    }
+                    Err(_) => break,
+                }
+            }
+            let n = batch.len() as u64;
+            if !qq.push(protocol::data(sub, json!(batch)), n) {
+                return;
+            }
+            if let Some(last) = ended {
+                return qq.finish(last);
+            }
+        }
+    });
+    Ok(Sub {
+        queue: q,
+        task: task.abort_handle(),
+    })
+}
```

`crates/fleetops-api/src/ws/conn.rs`:

```diff
--- a/crates/fleetops-api/src/ws/conn.rs
+++ b/crates/fleetops-api/src/ws/conn.rs
@@ -267,7 +267,7 @@ async fn start(
     sub: u32,
     msg: (Topic, Option<String>, Option<String>, Value),
 ) -> Result<Sub> {
-    let (topic, env, id, _opts) = msg;
+    let (topic, env, id, opts) = msg;
     let c = topics::Ctx {
         s,
         cu,
@@ -277,6 +277,7 @@ async fn start(
     match topic {
         Topic::Events => topics::events(&c, env, id).await,
         Topic::Stats => topics::stats(&c, env, id).await,
+        Topic::Logs => topics::logs(&c, env, id, opts).await,
         other => Err(Error::Invalid(format!(
             "topic {other:?} is not available yet"
         ))),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `ws_logs` 3 PASS. `slow_log_reader_gets_a_gap_instead_of_stalling` checks that every one of 300,000 lines is either delivered or counted in a `gap`. Everything else still PASSES, lint clean.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): batched logs over the websocket with gap on overflow"
```

---

### Task 8: The `exec` topic: interactive terminal with true backpressure

**Files:**
- Modify: `crates/fleetops-api/src/ws/topics.rs`, `crates/fleetops-api/src/ws/conn.rs`
- Test: `crates/fleetops-api/tests/ws_exec.rs`

**Interfaces:**
- Consumes: `StreamOps::{exec_start, exec_resize, exec_exit_code}` and `ExecOptions` (Task 1), and `Policy::Block` (Task 3).
- Produces:
  - `topics::exec(&Ctx, env, id, opts)`, which needs `DockerWrite`. Every attempt is audited as `container.exec`, with `detail.cmd` and, on failure, `detail.error`.
  - Output goes out as binary frames `[sub][bytes]` through `push_wait`, so a slow client pauses reads from Docker.
  - Input arrives as binary frames or as `{op:"input", data}`, and goes into the session's `mpsc` sender with `try_send`. When that buffer is full the client gets `conflict` "terminal input is backed up".
  - `{op:"resize"}` calls `exec_resize`.
  - When the process exits: `{type:"end", reason:"exited", exit_code}`.
  - `conn::ExecCtl{input, exec_id, ops}`, and `Sub` gains `exec: Option<ExecCtl>`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-api/tests/ws_exec.rs`:

```rust
mod common;

use common::fake_docker::{event, summary};
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn exec_streams_binary_io_with_resize_and_is_audited() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.add(summary("c1", "web", "running", None));
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(
        4,
        "exec",
        Some("c1"),
        json!({"cols": 100, "rows": 30}),
    ))
    .await;
    let expect_bin = |m: WsMessage| match m {
        WsMessage::Binary(b) => {
            assert_eq!(&b[..4], &4u32.to_be_bytes());
            String::from_utf8(b[4..].to_vec()).unwrap()
        }
        other => panic!("expected binary, got {other:?}"),
    };
    assert_eq!(expect_bin(ws.frame().await.unwrap()), "$ ");
    let mut frame = 4u32.to_be_bytes().to_vec();
    frame.extend(b"ls\n");
    ws.send_binary(frame).await;
    assert_eq!(expect_bin(ws.frame().await.unwrap()), "echo:ls\n");
    ws.send(json!({"op": "input", "sub": 4, "data": "pwd\n"}))
        .await;
    assert_eq!(expect_bin(ws.frame().await.unwrap()), "echo:pwd\n");
    ws.send(json!({"op": "resize", "sub": 4, "cols": 120, "rows": 40}))
        .await;
    eventually("resize", || {
        t.fake
            .streams
            .lock()
            .unwrap()
            .resizes
            .contains(&("exec1".into(), 120, 40))
    })
    .await;
    ws.send(json!({"op": "input", "sub": 4, "data": "exit\n"}))
        .await;
    let end = ws.until(|v| is(v, 4, "end")).await;
    assert_eq!(
        (end["reason"].as_str(), end["exit_code"].as_i64()),
        (Some("exited"), Some(0))
    );
    let r = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let exec = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "container.exec")
        .unwrap()
        .clone();
    assert_eq!(exec["result"], "success");
    assert_eq!(exec["target_id"], "c1");
    assert!(exec["environment_id"].is_i64());
    assert!(exec["detail"]["cmd"][2].as_str().unwrap().contains("bash"));
}

#[tokio::test]
async fn exec_errors_and_input_for_unknown_subs() {
    let t = app().await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "exec", Some("ghost"), Value::Null)).await;
    let e = ws.json().await;
    assert!(is(&e, 1, "error") && e["code"] == "upstream", "{e}");
    ws.send(json!({"op": "input", "sub": 1, "data": "x"})).await;
    assert_eq!(ws.json().await["code"], "not_found");
    ws.send(json!({"op": "resize", "sub": 1, "cols": 1, "rows": 1}))
        .await;
    assert_eq!(ws.json().await["code"], "not_found");
    let r = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let exec = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "container.exec")
        .unwrap()
        .clone();
    assert_eq!(exec["result"], "failure");
}

#[tokio::test]
async fn exec_output_waits_for_a_slow_client() {
    use common::fake_docker::{FLOOD_CHUNK, FLOOD_CHUNKS};
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.add(summary("c1", "web", "running", None));
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "exec", Some("c1"), json!({"cmd": ["flood"]})))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let before = FLOOD_CHUNKS.load(Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let stalled = FLOOD_CHUNKS.load(Ordering::SeqCst);
    // Queue (1024) plus socket buffers; unbounded reading would be millions.
    assert!(
        stalled < 1024 + (64 << 20) / FLOOD_CHUNK,
        "read {stalled} chunks"
    );
    assert_eq!(before, stalled, "no reads while the client is not reading");
    for _ in 0..2000 {
        ws.frame().await.unwrap();
    }
    assert!(
        FLOOD_CHUNKS.load(Ordering::SeqCst) > stalled,
        "reading resumes"
    );
}

#[tokio::test]
async fn other_connections_keep_flowing_while_one_is_slow() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.add(summary("c1", "web", "running", None));
    let addr = t.serve().await;
    let mut slow = Ws::open(addr, &admin).await;
    slow.send(ws_sub(1, "exec", Some("c1"), json!({"cmd": ["flood"]})))
        .await;
    let mut fast = Ws::open(addr, &admin).await;
    fast.send(ws_sub(1, "events", None, Value::Null)).await;
    eventually("events stream", || t.fake.open_event_streams() == 1).await;
    let fake = t.fake.clone();
    let emitter = tokio::spawn(async move {
        loop {
            fake.emit_event(event("c1", "start"));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    assert_eq!(fast.until(|v| is(v, 1, "data")).await["data"]["id"], "c1");
    emitter.abort();
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test ws_exec`
Expected: 3 FAIL, 1 PASS. The 3 failures get `topic Exec is not available yet`; for example, `exec_streams_binary_io_with_resize_and_is_audited` reports `expected binary, got Text(...)`. `other_connections_keep_flowing_while_one_is_slow` already passes, because the slow exec subscription is refused. It becomes a real check once exec exists, and it must still pass in Step 4.

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/ws/topics.rs`. The import block changes, and `exec()` and `audit_exec()` are appended:

```diff
--- a/crates/fleetops-api/src/ws/topics.rs
+++ b/crates/fleetops-api/src/ws/topics.rs
@@ -2,8 +2,8 @@
 //! needs, starts a producer task that fills the subscription's queue, and
 //! returns the subscription.
 
-use super::conn::Sub;
-use super::protocol;
+use super::conn::{ExecCtl, Sub};
+use super::protocol::{self, encode_binary, Out};
 use super::queue::{Policy, SubQueue};
 use crate::{
     auth::extract::CurrentUser,
@@ -12,8 +12,9 @@ use crate::{
 };
 use fleetops_core::docker::DockerOps;
 use fleetops_core::env::Environment;
-use fleetops_core::streams::LogOptions;
-use fleetops_core::{Error, Result};
+use fleetops_core::streams::{ExecOptions, LogOptions};
+use fleetops_core::{now_unix, Error, Result};
+use fleetops_store::NewAudit;
 use futures_util::StreamExt;
 use serde_json::{json, Value};
 use std::{sync::Arc, time::Duration};
@@ -106,6 +107,7 @@ pub(super) async fn events(c: &Ctx<'_>, env: Option<String>, id: Option<String>)
     Ok(Sub {
         queue: q,
         task: task.abort_handle(),
+        exec: None,
     })
 }
 
@@ -138,6 +140,7 @@ pub(super) async fn stats(c: &Ctx<'_>, env: Option<String>, id: Option<String>)
     Ok(Sub {
         queue: q,
         task: task.abort_handle(),
+        exec: None,
     })
 }
 
@@ -193,5 +196,88 @@ pub(super) async fn logs(
     Ok(Sub {
         queue: q,
         task: task.abort_handle(),
+        exec: None,
     })
 }
+
+/// An interactive TTY in container `id`: output as binary frames, input
+/// through `input` frames, resized with `resize`.
+pub(super) async fn exec(
+    c: &Ctx<'_>,
+    env: Option<String>,
+    id: Option<String>,
+    opts_v: Value,
+) -> Result<Sub> {
+    let (s, cu, sub) = (c.s, c.cu, c.sub);
+    let (env, ops) = docker_for(s, cu, &env, Action::DockerWrite)?;
+    let id = required(&id, "id")?.to_string();
+    let o: ExecOptions = opts(opts_v)?;
+    let session = ops.exec_start(&id, &o).await;
+    // Only the error text crosses the await: the session is not Sync.
+    let failure = session.as_ref().err().map(|e| e.to_string());
+    audit_exec(s, cu, env.id(), &id, &o, failure).await;
+    let session = session?;
+    let q = c.queue(Policy::Block);
+    let qq = q.clone();
+    let mut output = session.output;
+    let exec_id = session.exec_id.clone();
+    let task_ops = ops.clone();
+    let task = tokio::spawn(async move {
+        while let Some(chunk) = output.next().await {
+            let Ok(bytes) = chunk else { break };
+            // Waits while the client is behind: Docker's output is
+            // read no faster than the socket drains it.
+            if !qq.push_wait(Out::Binary(encode_binary(sub, &bytes))).await {
+                return;
+            }
+        }
+        let mut code = None;
+        for _ in 0..20 {
+            code = task_ops.exec_exit_code(&exec_id).await.ok().flatten();
+            if code.is_some() {
+                break;
+            }
+            tokio::time::sleep(Duration::from_millis(50)).await;
+        }
+        qq.finish(protocol::end(sub, "exited", json!({ "exit_code": code })));
+    });
+    Ok(Sub {
+        queue: q,
+        task: task.abort_handle(),
+        exec: Some(ExecCtl {
+            input: session.input,
+            exec_id: session.exec_id,
+            ops,
+        }),
+    })
+}
+
+/// Exec is root-equivalent access to the container, so every attempt is
+/// audited with the command.
+async fn audit_exec(
+    s: &AppState,
+    cu: &CurrentUser,
+    env_id: i64,
+    id: &str,
+    o: &ExecOptions,
+    failure: Option<String>,
+) {
+    let (result, mut detail) = match failure {
+        None => ("success", json!({})),
+        Some(e) => ("failure", json!({ "error": e })),
+    };
+    detail["cmd"] = json!(o.command());
+    let entry = NewAudit {
+        ts: now_unix(),
+        user_id: Some(cu.user.id),
+        environment_id: Some(env_id),
+        action: "container.exec",
+        target_type: Some("container"),
+        target_id: Some(id),
+        result,
+        detail: Some(detail),
+    };
+    if let Err(e) = s.store.insert_audit(&entry).await {
+        tracing::error!(error = %e, "failed to write audit log");
+    }
+}
```

`crates/fleetops-api/src/ws/conn.rs`. This adds `ExecCtl`, the `exec` field, and real `input` and `resize` handling:

```diff
--- a/crates/fleetops-api/src/ws/conn.rs
+++ b/crates/fleetops-api/src/ws/conn.rs
@@ -11,6 +11,7 @@ use crate::{
     AppState,
 };
 use axum::extract::ws::{Message, WebSocket};
+use fleetops_core::docker::DockerOps;
 use fleetops_core::{now_unix, Error, Result};
 use futures_util::{SinkExt, StreamExt};
 use serde_json::Value;
@@ -22,18 +23,25 @@ use std::{
     },
     time::Duration,
 };
-use tokio::sync::{broadcast::error::RecvError, Notify};
+use tokio::sync::{broadcast::error::RecvError, mpsc, Notify};
 
 /// Subscriptions one connection may hold at once.
 pub const MAX_SUBS: usize = 256;
 /// Largest client frame accepted.
 pub const MAX_FRAME_BYTES: usize = 1 << 20;
 
+pub(super) struct ExecCtl {
+    pub input: mpsc::Sender<Vec<u8>>,
+    pub exec_id: String,
+    pub ops: Arc<dyn DockerOps>,
+}
+
 /// A live subscription. Dropping it (unsub, or the connection closing)
 /// stops its producer.
 pub(super) struct Sub {
     pub queue: Arc<SubQueue>,
     pub task: tokio::task::AbortHandle,
+    pub exec: Option<ExecCtl>,
 }
 
 impl Drop for Sub {
@@ -208,12 +216,22 @@ async fn session_still_valid(s: &AppState, token_hash: &str, user_id: i64) -> bo
     s.store.get_user(user_id).await.is_ok()
 }
 
-/// Exec input; until the exec topic exists no subscription takes any.
-fn input(shared: &Shared, sub: u32, _data: Vec<u8>) {
-    shared.fail(
-        Some(sub),
-        &Error::NotFound(format!("no exec subscription {sub}")),
-    );
+fn input(shared: &Shared, sub: u32, data: Vec<u8>) {
+    let subs = shared.subs.lock().unwrap();
+    match subs.get(&sub).and_then(|s| s.exec.as_ref()) {
+        Some(ctl) => {
+            if ctl.input.try_send(data).is_err() {
+                shared.fail(
+                    Some(sub),
+                    &Error::Conflict("terminal input is backed up".into()),
+                );
+            }
+        }
+        None => shared.fail(
+            Some(sub),
+            &Error::NotFound(format!("no exec subscription {sub}")),
+        ),
+    }
 }
 
 async fn handle(s: &AppState, cu: &CurrentUser, shared: &Arc<Shared>, msg: ClientMsg) {
@@ -253,10 +271,28 @@ async fn handle(s: &AppState, cu: &CurrentUser, shared: &Arc<Shared>, msg: Clien
             shared.subs.lock().unwrap().remove(&sub);
         }
         ClientMsg::Input { sub, data } => input(shared, sub, data.into_bytes()),
-        ClientMsg::Resize { sub, .. } => shared.fail(
-            Some(sub),
-            &Error::NotFound(format!("no exec subscription {sub}")),
-        ),
+        ClientMsg::Resize { sub, cols, rows } => {
+            let target = shared
+                .subs
+                .lock()
+                .unwrap()
+                .get(&sub)
+                .and_then(|s| s.exec.as_ref().map(|c| (c.ops.clone(), c.exec_id.clone())));
+            match target {
+                Some((ops, exec_id)) => {
+                    let shared = shared.clone();
+                    tokio::spawn(async move {
+                        if let Err(e) = ops.exec_resize(&exec_id, cols, rows).await {
+                            shared.fail(Some(sub), &e);
+                        }
+                    });
+                }
+                None => shared.fail(
+                    Some(sub),
+                    &Error::NotFound(format!("no exec subscription {sub}")),
+                ),
+            }
+        }
     }
 }
 
@@ -278,6 +314,7 @@ async fn start(
         Topic::Events => topics::events(&c, env, id).await,
         Topic::Stats => topics::stats(&c, env, id).await,
         Topic::Logs => topics::logs(&c, env, id, opts).await,
+        Topic::Exec => topics::exec(&c, env, id, opts).await,
         other => Err(Error::Invalid(format!(
             "topic {other:?} is not available yet"
         ))),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test -p fleetops-api && make lint`
Expected: `ws_exec` 4 PASS, everything else still PASSES, lint clean. Run `cargo test -p fleetops-api` twice more; the timing-sensitive tests must stay green.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): interactive exec over the websocket with backpressure and audit"
```

---

### Task 9: Image pull and build as background tasks, and the `task` topic

**Files:**
- Modify: `Cargo.toml` (axum `multipart` feature)
- Create: `crates/fleetops-api/src/routes/docker/image_tasks.rs`, `crates/fleetops-api/src/routes/tasks.rs`
- Modify: `crates/fleetops-api/src/routes/docker/mod.rs`, `crates/fleetops-api/src/routes/mod.rs`, `crates/fleetops-api/src/ws/topics.rs`, `crates/fleetops-api/src/ws/conn.rs`
- Test: `crates/fleetops-api/tests/tasks.rs`

**Interfaces:**
- Consumes: `TaskManager` (Task 4), `StreamOps::{pull_image, build_image}` and `split_image_reference` (Task 1), and `ctx()` (P2a).
- Produces:
  - `POST /envs/{env}/images/pull {image, tag?}` → `202 {task_id}` (`DockerWrite`). A bad reference gives 400.
  - `POST /envs/{env}/images/build` (multipart) → `202 {task_id}`:
    - Fields are `context` (a tar) or `dockerfile` (a lone Dockerfile), plus optional `tag` and `dockerfile_path`.
    - Sending both `context` and `dockerfile`, neither, an unknown field or a bad tag gives 400.
    - The body limit is `BUILD_UPLOAD_MAX` (100 MiB).
  - `GET /tasks/{id}` → `TaskInfo`. Other users' tasks give 404 for non-admins.
  - `topics::task(&Ctx, id)`: replays the backlog, then goes live. It ends with `end` reason `succeeded` or `failed`, with `task: TaskInfo`.
  - Audit rows `image.pull` / `image.build`, target the image reference, written when the task finishes. The result is `success`, or `failure` with `detail.error`.
  - Running more than 8 tasks at once gives 409.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-api/tests/tasks.rs`:

```rust
mod common;

use axum::{body::Body, extract::ConnectInfo, http::Request, http::StatusCode};
use common::*;
use fleetops_core::streams::BuildContext;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;

async fn pull(t: &TestApp, s: &Session, image: &str) -> Resp {
    t.send(req(
        "POST",
        "/api/v1/envs/local/images/pull",
        Some(json!({"image": image})),
        Some(s),
    ))
    .await
}

async fn wait_task(t: &TestApp, s: &Session, id: &str) -> Value {
    for _ in 0..200 {
        let r = t
            .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(s)))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
        if r.json["status"] != "running" {
            return r.json;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("task {id} did not finish");
}

/// A multipart/form-data request with text or file parts.
fn multipart(uri: &str, parts: &[(&str, &[u8])], s: &Session) -> Request<Body> {
    let boundary = "fleetopsboundary7";
    let mut body = Vec::new();
    for (name, data) in parts {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend(*data);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").as_bytes());
    let mut r = Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", &s.cookie)
        .header("x-csrf-token", &s.csrf)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    r
}

#[tokio::test]
async fn pull_runs_as_a_task_and_is_audited() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = pull(&t, &admin, "nginx:1.27").await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    let id = r.json["task_id"].as_str().unwrap().to_string();
    let done = wait_task(&t, &admin, &id).await;
    assert_eq!(done["status"], "succeeded");
    assert_eq!(
        (done["kind"].as_str(), done["target"].as_str()),
        (Some("image.pull"), Some("nginx:1.27"))
    );
    assert_eq!(done["env"], "local");
    assert!(done["finished_at"].is_i64());
    assert_eq!(t.fake.streams.lock().unwrap().pulls, ["nginx:1.27"]);
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let row = audit
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "image.pull")
        .unwrap()
        .clone();
    assert_eq!(
        (row["result"].as_str(), row["target_id"].as_str()),
        (Some("success"), Some("nginx:1.27"))
    );
    assert!(row["environment_id"].is_i64());
}

#[tokio::test]
async fn failed_pull_fails_the_task_with_dockers_message() {
    let t = app().await;
    let admin = t.with_local().await;
    let id = pull(&t, &admin, "missing").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let done = wait_task(&t, &admin, &id).await;
    assert_eq!(done["status"], "failed");
    assert!(
        done["error"].as_str().unwrap().contains("manifest unknown"),
        "{done}"
    );
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let row = audit
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "image.pull")
        .unwrap()
        .clone();
    assert_eq!(row["result"], "failure");
    assert!(row["detail"]["error"]
        .as_str()
        .unwrap()
        .contains("manifest unknown"));
}

#[tokio::test]
async fn pull_input_is_validated() {
    let t = app().await;
    let admin = t.with_local().await;
    for bad in ["", "has space", "nginx:"] {
        let r = pull(&t, &admin, bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}: {:?}", r.json);
    }
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine", "tag": "3.20"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let id = r.json["task_id"].as_str().unwrap().to_string();
    wait_task(&t, &admin, &id).await;
    assert_eq!(t.fake.streams.lock().unwrap().pulls, ["alpine:3.20"]);
}

#[tokio::test]
async fn task_topic_replays_the_backlog_then_follows_to_the_end() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.hold_pulls.store(true, Ordering::SeqCst);
    let id = pull(&t, &admin, "alpine").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Let the first progress message land before subscribing.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "task", "id": id}))
        .await;
    let first = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(
        first["data"]["status"], "Pulling from alpine",
        "backlog first"
    );
    t.fake.release_pulls.notify_one();
    let mut statuses = Vec::new();
    let end = loop {
        let v = ws.json().await;
        if is(&v, 1, "end") {
            break v;
        }
        statuses.push(v["data"]["status"].as_str().unwrap().to_string());
    };
    assert_eq!(
        statuses.last().unwrap(),
        "Status: Downloaded newer image for alpine:latest"
    );
    assert_eq!(end["reason"], "succeeded");
    assert_eq!(end["task"]["id"], id.as_str());
    // A finished task still replays, then ends at once.
    ws.send(json!({"op": "sub", "sub": 2, "topic": "task", "id": id}))
        .await;
    let mut n = 0;
    loop {
        let v = ws.json().await;
        if is(&v, 2, "end") {
            break;
        }
        n += 1;
    }
    assert_eq!(n, 3);
}

#[tokio::test]
async fn tasks_are_private_to_their_owner_and_admins() {
    let t = app().await;
    let admin = t.with_local().await;
    t.create_user(&admin, "bob", "bob-password", "user").await;
    t.create_user(&admin, "eve", "eve-password", "user").await;
    let bob = session_from(&t.login("bob", "bob-password").await);
    let eve = session_from(&t.login("eve", "eve-password").await);
    let id = pull(&t, &bob, "alpine").await.json["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_task(&t, &bob, &id).await;
    wait_task(&t, &admin, &id).await;
    let r = t
        .send(req("GET", &format!("/api/v1/tasks/{id}"), None, Some(&eve)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let mut ws = Ws::open(t.serve().await, &eve).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "task", "id": id}))
        .await;
    let e = ws.json().await;
    assert!(is(&e, 1, "error") && e["code"] == "not_found", "{e}");
    let r = t
        .send(req("GET", "/api/v1/tasks/nope", None, Some(&bob)))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn too_many_running_tasks_is_a_conflict() {
    let t = app().await;
    let admin = t.with_local().await;
    t.fake.hold_pulls.store(true, Ordering::SeqCst);
    for _ in 0..fleetops_api::tasks::MAX_RUNNING_TASKS {
        assert_eq!(
            pull(&t, &admin, "alpine").await.status,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        pull(&t, &admin, "alpine").await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn build_from_an_uploaded_dockerfile_or_context() {
    let t = app().await;
    let admin = t.with_local().await;
    let uri = "/api/v1/envs/local/images/build";
    let r = t
        .send(multipart(
            uri,
            &[("dockerfile", b"FROM alpine\n"), ("tag", b"app:1")],
            &admin,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    let done = wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    assert_eq!(
        (done["status"].as_str(), done["target"].as_str()),
        (Some("succeeded"), Some("app:1"))
    );
    let r = t
        .send(multipart(
            uri,
            &[
                ("context", b"tar-bytes"),
                ("dockerfile_path", b"docker/Dockerfile"),
            ],
            &admin,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    let builds = t.fake.streams.lock().unwrap().builds.clone();
    assert_eq!(
        builds[0].context,
        BuildContext::Dockerfile(b"FROM alpine\n".to_vec())
    );
    assert_eq!(builds[0].tag.as_deref(), Some("app:1"));
    assert_eq!(builds[1].context, BuildContext::Tar(b"tar-bytes".to_vec()));
    assert_eq!(builds[1].dockerfile.as_deref(), Some("docker/Dockerfile"));
    assert_eq!(builds[1].tag, None);
}

#[tokio::test]
async fn failing_build_step_fails_the_task() {
    let t = app().await;
    let admin = t.with_local().await;
    let r = t
        .send(multipart(
            "/api/v1/envs/local/images/build",
            &[("dockerfile", b"RUN fail")],
            &admin,
        ))
        .await;
    let done = wait_task(&t, &admin, r.json["task_id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert!(
        done["error"].as_str().unwrap().contains("non-zero code"),
        "{done}"
    );
}

#[tokio::test]
async fn build_upload_is_validated() {
    let t = app().await;
    let admin = t.with_local().await;
    let uri = "/api/v1/envs/local/images/build";
    for parts in [
        &[][..],
        &[("tag", &b"x:1"[..])][..],
        &[("dockerfile", &b"FROM a"[..]), ("context", &b"t"[..])][..],
        &[("dockerfile", &b"FROM a"[..]), ("tag", &b"bad tag"[..])][..],
        &[("surprise", &b"?"[..])][..],
    ] {
        let r = t.send(multipart(uri, parts, &admin)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json);
        assert_eq!(r.json["code"], "invalid");
    }
    // Mutations need CSRF like every other route.
    let mut r = multipart(uri, &[("dockerfile", b"FROM a")], &admin);
    r.headers_mut().remove("x-csrf-token");
    assert_eq!(t.send(r).await.status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test tasks`
Expected: FAIL. For example, `pull_runs_as_a_task_and_is_audited` gets `left: 405, right: 202`: `POST /images/pull` matches `/images/{id}`, which has no POST.

- [ ] **Step 3: Implement**

Workspace `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -17,7 +17,7 @@ anyhow = "1"
 argon2 = { version = "0.5", features = ["std"] }
 async-trait = "0.1"
 bollard = "0.21"
-axum = { version = "0.8", features = ["ws"] }
+axum = { version = "0.8", features = ["ws", "multipart"] }
 axum-server = { version = "0.7", features = ["tls-rustls-no-provider"] }
 clap = { version = "4", features = ["derive", "env"] }
 futures-util = "0.3"
```

`crates/fleetops-api/src/routes/docker/image_tasks.rs`:

```rust
//! Image pull and build: long-running, so they start a background task and
//! answer `202 {task_id}` at once (spec §5.1).

use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    tasks::{NewTask, TaskInfo, TaskStatus},
    ApiResult, AppState,
};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use fleetops_core::docker::split_image_reference;
use fleetops_core::streams::{BoxStream, BuildContext, BuildSpec};
use fleetops_core::{now_unix, Error};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::{json, Value};

/// Largest accepted build upload.
pub const BUILD_UPLOAD_MAX: usize = 100 * 1024 * 1024;

#[derive(Deserialize)]
pub struct PullBody {
    /// `nginx`, `nginx:1.27`, `ghcr.io/org/app:2` or `repo@sha256:...`.
    pub image: String,
    /// Overrides any tag in `image`.
    #[serde(default)]
    pub tag: Option<String>,
}

/// Starts `stream` as a task and audits its outcome when it finishes.
fn start_task(
    s: &AppState,
    user_id: i64,
    env: (&str, i64),
    kind: &'static str,
    target: String,
    stream: BoxStream<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let store = s.store.clone();
    let (env_name, env_id) = env;
    let task = s.tasks.spawn(
        NewTask {
            kind,
            env: env_name.to_string(),
            target: target.clone(),
            owner: user_id,
        },
        stream,
        move |info: TaskInfo| async move {
            let (result, detail) = match info.status {
                TaskStatus::Succeeded => ("success", Some(json!({ "task_id": info.id }))),
                _ => (
                    "failure",
                    Some(json!({ "task_id": info.id, "error": info.error })),
                ),
            };
            let entry = NewAudit {
                ts: now_unix(),
                user_id: Some(user_id),
                environment_id: Some(env_id),
                action: kind,
                target_type: Some("image"),
                target_id: Some(&target),
                result,
                detail,
            };
            if let Err(e) = store.insert_audit(&entry).await {
                tracing::error!(error = %e, "failed to write audit log");
            }
        },
    )?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "task_id": task.info().id })),
    ))
}

pub async fn pull(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<PullBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let (image, parsed_tag) = split_image_reference(&body.image)?;
    let tag = body.tag.filter(|t| !t.is_empty()).unwrap_or(parsed_tag);
    let target = if tag.is_empty() {
        image.clone()
    } else {
        format!("{image}:{tag}")
    };
    let stream = c.ops.pull_image(&image, &tag);
    start_task(
        &s,
        c.user.user.id,
        (c.env.name(), c.env.id()),
        "image.pull",
        target,
        stream,
    )
}

fn bad(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("bad upload: {e}"))
}

/// Multipart fields: `context` (a tar of the build context) or `dockerfile`
/// (a lone Dockerfile), plus optional `tag` and `dockerfile_path`.
pub async fn build(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    mut form: Multipart,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let (mut context, mut tag, mut dockerfile) = (None, None, None);
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(bad)?;
        match name.as_str() {
            "context" | "dockerfile" if context.is_some() => {
                return Err(Error::Invalid(
                    "send either `context` or `dockerfile`, not both".into(),
                )
                .into())
            }
            "context" => context = Some(BuildContext::Tar(bytes.to_vec())),
            "dockerfile" => context = Some(BuildContext::Dockerfile(bytes.to_vec())),
            "tag" => tag = Some(String::from_utf8(bytes.to_vec()).map_err(bad)?),
            "dockerfile_path" => dockerfile = Some(String::from_utf8(bytes.to_vec()).map_err(bad)?),
            other => return Err(Error::Invalid(format!("unexpected field `{other}`")).into()),
        }
    }
    let context = context
        .ok_or_else(|| Error::Invalid("a `context` tar or a `dockerfile` is required".into()))?;
    let tag = tag.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(t) = &tag {
        split_image_reference(t)?;
    }
    let target = tag.clone().unwrap_or_else(|| "(untagged)".into());
    let stream = c.ops.build_image(BuildSpec {
        context,
        tag,
        dockerfile: dockerfile.filter(|d| !d.is_empty()),
    });
    start_task(
        &s,
        c.user.user.id,
        (c.env.name(), c.env.id()),
        "image.build",
        target,
        stream,
    )
}
```

`crates/fleetops-api/src/routes/tasks.rs`:

```rust
use crate::{auth::extract::CurrentUser, extract::ApiPath, tasks::TaskInfo, ApiResult, AppState};
use axum::{extract::State, Json};
use fleetops_core::{Error, Role};

/// A task's current state. Other users' tasks look missing to non-admins.
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<TaskInfo>> {
    let task = s
        .tasks
        .get(&id)
        .filter(|t| t.owner == cu.user.id || cu.user.role == Role::Admin)
        .ok_or_else(|| Error::NotFound(format!("task '{id}'")))?;
    Ok(Json(task.info()))
}
```

`crates/fleetops-api/src/routes/docker/mod.rs`:

```diff
--- a/crates/fleetops-api/src/routes/docker/mod.rs
+++ b/crates/fleetops-api/src/routes/docker/mod.rs
@@ -2,6 +2,7 @@
 
 pub mod compose;
 pub mod containers;
+pub mod image_tasks;
 pub mod images;
 pub mod networks;
 pub mod system;
```

`crates/fleetops-api/src/routes/mod.rs`:

```diff
--- a/crates/fleetops-api/src/routes/mod.rs
+++ b/crates/fleetops-api/src/routes/mod.rs
@@ -2,10 +2,12 @@ pub mod audit_log;
 pub mod auth;
 pub mod docker;
 pub mod envs;
+pub mod tasks;
 pub mod users;
 
 use crate::{error::ApiError, AppState};
 use axum::{
+    extract::DefaultBodyLimit,
     routing::{delete, get, patch, post, put},
     Json, Router,
 };
@@ -28,12 +30,15 @@ pub fn api() -> Router<AppState> {
         .route("/users/{id}", patch(users::update).delete(users::remove))
         .route("/audit", get(audit_log::list))
         .route("/ws", get(crate::ws::handler))
+        .route("/tasks/{id}", get(tasks::get))
         .merge(env_routes())
         .fallback(not_found)
 }
 
 fn env_routes() -> Router<AppState> {
-    use docker::{compose, containers as c, images as i, networks as n, system, volumes as v};
+    use docker::{
+        compose, containers as c, image_tasks, images as i, networks as n, system, volumes as v,
+    };
     Router::new()
         .route("/envs", get(envs::list).post(envs::create))
         .route("/envs/{env}", patch(envs::update).delete(envs::remove))
@@ -61,6 +66,11 @@ fn env_routes() -> Router<AppState> {
         .route("/envs/{env}/containers/{id}/{action}", post(c::action))
         .route("/envs/{env}/images", get(i::list))
         .route("/envs/{env}/images/prune", post(i::prune))
+        .route("/envs/{env}/images/pull", post(image_tasks::pull))
+        .route(
+            "/envs/{env}/images/build",
+            post(image_tasks::build).layer(DefaultBodyLimit::max(image_tasks::BUILD_UPLOAD_MAX)),
+        )
         .route("/envs/{env}/images/{id}", get(i::inspect).delete(i::remove))
         .route("/envs/{env}/images/{id}/history", get(i::history))
         .route("/envs/{env}/images/{id}/tag", post(i::tag))
```

`crates/fleetops-api/src/ws/topics.rs`:

```diff
--- a/crates/fleetops-api/src/ws/topics.rs
+++ b/crates/fleetops-api/src/ws/topics.rs
@@ -8,12 +8,13 @@ use super::queue::{Policy, SubQueue};
 use crate::{
     auth::extract::CurrentUser,
     authz::{authorize, Action},
+    tasks::{TaskEvent, TaskStatus},
     AppState,
 };
 use fleetops_core::docker::DockerOps;
 use fleetops_core::env::Environment;
 use fleetops_core::streams::{ExecOptions, LogOptions};
-use fleetops_core::{now_unix, Error, Result};
+use fleetops_core::{now_unix, Error, Result, Role};
 use fleetops_store::NewAudit;
 use futures_util::StreamExt;
 use serde_json::{json, Value};
@@ -252,6 +253,55 @@ pub(super) async fn exec(
     })
 }
 
+/// A background task's progress: everything so far, then live, then `end`.
+pub(super) async fn task(c: &Ctx<'_>, id: Option<String>) -> Result<Sub> {
+    let (s, cu, sub) = (c.s, c.cu, c.sub);
+    let id = required(&id, "id")?;
+    let task = s
+        .tasks
+        .get(id)
+        .filter(|t| t.owner == cu.user.id || cu.user.role == Role::Admin)
+        .ok_or_else(|| Error::NotFound(format!("task '{id}'")))?;
+    let feed = task.feed();
+    let q = c.queue(Policy::DropOldest);
+    let qq = q.clone();
+    let done = move |info: crate::tasks::TaskInfo| {
+        let reason = match info.status {
+            TaskStatus::Succeeded => "succeeded",
+            _ => "failed",
+        };
+        protocol::end(sub, reason, json!({ "task": info }))
+    };
+    let handle = tokio::spawn(async move {
+        let mut live = feed.live;
+        for v in feed.backlog {
+            qq.push(protocol::data(sub, v), 1);
+        }
+        if let Some(info) = feed.done {
+            return qq.finish(done(info));
+        }
+        loop {
+            match live.recv().await {
+                Ok(TaskEvent::Progress(v)) => {
+                    if !qq.push(protocol::data(sub, v), 1) {
+                        return;
+                    }
+                }
+                Ok(TaskEvent::Done(info)) => return qq.finish(done(info)),
+                Err(RecvError::Lagged(_)) => continue,
+                Err(RecvError::Closed) => {
+                    return qq.finish(protocol::end(sub, "closed", Value::Null))
+                }
+            }
+        }
+    });
+    Ok(Sub {
+        queue: q,
+        task: handle.abort_handle(),
+        exec: None,
+    })
+}
+
 /// Exec is root-equivalent access to the container, so every attempt is
 /// audited with the command.
 async fn audit_exec(
```

`crates/fleetops-api/src/ws/conn.rs`. Every topic now exists, so the fallback arm goes:

```diff
--- a/crates/fleetops-api/src/ws/conn.rs
+++ b/crates/fleetops-api/src/ws/conn.rs
@@ -315,8 +315,6 @@ async fn start(
         Topic::Stats => topics::stats(&c, env, id).await,
         Topic::Logs => topics::logs(&c, env, id, opts).await,
         Topic::Exec => topics::exec(&c, env, id, opts).await,
-        other => Err(Error::Invalid(format!(
-            "topic {other:?} is not available yet"
-        ))),
+        Topic::Task => topics::task(&c, id).await,
     }
 }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo fmt --all && cargo test --workspace && make lint`
Expected: `tasks` 9 PASS and the whole workspace is green: 214 tests when this plan was verified. Lint is clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-api
git commit -m "feat(api): image pull/build as background tasks with the task topic"
```

---

### Task 10: End to end against the real daemon

**Files:**
- Modify: `crates/fleetops-server/Cargo.toml` (`docker-it` feature, dev dependencies), `Makefile` (`test-docker` also runs this)
- Test: `crates/fleetops-server/tests/live.rs`

**Interfaces:**
- Consumes: everything above, through `fleetops_api::app` with the real `DockerEnvFactory`.
- Produces: `make test-docker` runs both the `fleetops-docker` daemon tests and one live test. That test drives REST plus every websocket topic (events, logs, stats, exec, task) against the real daemon. This is a verification task with no new production code, so the test is expected to pass on its first run.

- [ ] **Step 1: Add the feature and the test**

`crates/fleetops-server/Cargo.toml`:

```diff
--- a/crates/fleetops-server/Cargo.toml
+++ b/crates/fleetops-server/Cargo.toml
@@ -8,6 +8,10 @@ rust-version.workspace = true
 name = "fleetops"
 path = "src/main.rs"
 
+[features]
+# End-to-end tests against a real Docker daemon (see `make test-docker`).
+docker-it = []
+
 [dependencies]
 fleetops-core.workspace = true
 fleetops-store.workspace = true
@@ -27,6 +31,8 @@ tracing.workspace = true
 tracing-subscriber.workspace = true
 
 [dev-dependencies]
+futures-util.workspace = true
 http-body-util.workspace = true
 tempfile.workspace = true
+tokio-tungstenite.workspace = true
 tower.workspace = true
```

`Makefile`. The recipe line is TAB-indented:

```diff
--- a/Makefile
+++ b/Makefile
@@ -10,7 +10,7 @@ test:
 # Integration tests against the Docker daemon of the current docker context.
 test-docker:
 	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
-		cargo test -p fleetops-docker --features docker-it --test docker_it
+		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && cargo test -p fleetops-server --features docker-it --test live'
 
 lint:
 	cargo fmt --all --check
```

`crates/fleetops-server/tests/live.rs`:

```rust
//! End to end against a real Docker daemon: REST plus every websocket topic
//! through the full app. Enable with `--features docker-it` (see
//! `make test-docker`).
#![cfg(feature = "docker-it")]

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    Router,
};
use fleetops_api::{envs::bootstrap_local, ApiConfig, AppState};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tower::ServiceExt;

struct Live {
    router: Router,
    cookie: String,
    csrf: String,
}

impl Live {
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .header("x-csrf-token", &self.csrf);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        let mut r = b
            .body(body.map_or(Body::empty(), |v| Body::from(v.to_string())))
            .unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        let res = self.router.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

async fn start() -> (Live, SocketAddr) {
    let host = std::env::var("DOCKER_HOST").expect("DOCKER_HOST (make test-docker sets it)");
    let store = Store::in_memory().await.unwrap();
    let state = AppState::new(
        store.clone(),
        ApiConfig::default(),
        Arc::new(DockerEnvFactory),
    );
    bootstrap_local(&store, &state.envs, &host)
        .await
        .unwrap()
        .expect("local environment registered");
    let router = fleetops_api::app(state, Router::new());
    let mut r = Request::post("/api/v1/auth/setup")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"username": "admin", "password": "correct-horse"}).to_string(),
        ))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let res = router.clone().oneshot(r).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let live = Live {
        router: router.clone(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (live, addr)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(ws: &mut Socket) -> Message {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("frame within 30s")
            .expect("socket open")
            .unwrap();
        if !matches!(m, Message::Ping(_) | Message::Pong(_)) {
            return m;
        }
    }
}

/// JSON frames until `pred` matches.
async fn until(ws: &mut Socket, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        if let Message::Text(t) = next(ws).await {
            let v: Value = serde_json::from_str(&t).unwrap();
            if pred(&v) {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn every_topic_works_against_the_real_daemon() {
    let (live, addr) = start().await;
    let mut req = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("cookie", live.cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let send = |v: Value| Message::Text(v.to_string().into());

    ws.send(send(
        json!({"op": "sub", "sub": 1, "topic": "events", "env": "local"}),
    ))
    .await
    .unwrap();
    let name = format!("fleetops-live-{}", std::process::id());
    let (status, created) = live
        .call(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({
                "name": name, "image": "alpine:3.20",
                "cmd": ["sh", "-c", "echo hello-live; exec sleep 300"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let started = until(&mut ws, |v| {
        v["sub"] == 1 && v["data"]["id"] == id.as_str() && v["data"]["action"] == "start"
    })
    .await;
    assert_eq!(started["data"]["name"], name.as_str());

    ws.send(send(
        json!({"op": "sub", "sub": 2, "topic": "logs", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let logs = until(&mut ws, |v| v["sub"] == 2 && v["type"] == "data").await;
    assert_eq!(logs["data"][0]["text"], "hello-live");

    ws.send(send(
        json!({"op": "sub", "sub": 3, "topic": "stats", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let stats = until(&mut ws, |v| v["sub"] == 3 && v["type"] == "data").await;
    assert!(
        stats["data"]["memory_limit"].as_u64().unwrap() > 0,
        "{stats}"
    );
    ws.send(send(json!({"op": "unsub", "sub": 3})))
        .await
        .unwrap();

    ws.send(send(
        json!({"op": "sub", "sub": 4, "topic": "exec", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let mut input = 4u32.to_be_bytes().to_vec();
    input.extend(b"echo exec-$((2+3)); exit\n");
    ws.send(Message::Binary(input.into())).await.unwrap();
    let mut out = Vec::new();
    let end = loop {
        match next(&mut ws).await {
            Message::Binary(b) if b[..4] == 4u32.to_be_bytes() => out.extend(&b[4..]),
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["sub"] == 4 && v["type"] == "end" {
                    break v;
                }
            }
            _ => {}
        }
    };
    assert!(String::from_utf8_lossy(&out).contains("exec-5"));
    assert_eq!(end["exit_code"], 0, "{end}");

    let (status, task) = live
        .call(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine:3.20"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    ws.send(send(
        json!({"op": "sub", "sub": 5, "topic": "task", "id": task["task_id"]}),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, |v| v["sub"] == 5 && v["type"] == "end").await;
    assert_eq!(done["reason"], "succeeded", "{done}");

    let (status, _) = live
        .call(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}?force=true"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    until(&mut ws, |v| {
        v["sub"] == 1 && v["data"]["id"] == id.as_str() && v["data"]["action"] == "destroy"
    })
    .await;
}
```

- [ ] **Step 2: Run it against the daemon**

Run: `make test-docker && cargo clippy -p fleetops-docker -p fleetops-server --features docker-it --all-targets -- -D warnings`
Expected: `docker_it` 21 PASS, then `live` 1 PASS (`every_topic_works_against_the_real_daemon`). Clippy is clean, and `docker ps -a --filter name=fleetops-live` shows nothing.

To confirm the test can fail, temporarily change `"echo exec-$((2+3)); exit\n"` to `"exit\n"` and re-run: the `exec-5` assertion must FAIL. Then revert the change.

- [ ] **Step 3: Commit**

```bash
git add Cargo.lock Makefile crates/fleetops-server
git commit -m "test(server): every websocket topic end to end against the real daemon"
```

---

## Deferred (tracked)

- **P2c:** utoipa OpenAPI covering every P1/P2a/P2b handler at `/api/v1/openapi.json` (the websocket documented as a single path), a `fleetops --openapi` flag that prints it, and the generated TS client (`pnpm gen:api`, P3).
- **P3 (web):**
  - Client reconnect with backoff and resubscribe of every active sub, plus refetch of affected queries after `env_status` up.
  - Container list sparklines built on `stats`.
  - The Vite dev proxy must keep the `Host` header consistent with `Origin` for `/api/v1/ws` (`changeOrigin: false`).
- Existing subscriptions on an environment that is later replaced (URL change, delete and re-create) keep their original Docker connection until they resubscribe; new subscriptions use the new instance. Ending them with `end{reason:"env_changed"}` needs registry change notifications.
- No limit on websocket connections per user, and no rate limit on `sub` frames. Both are bounded only by `MAX_SUBS` and `MAX_FRAME_BYTES` per connection.
- Build uploads are buffered in memory, up to 100 MiB each, with at most 8 running tasks. Streaming the multipart body straight to Docker would remove the buffer.
- Still-open minors from P1 and P2a:
  - `__Host-` cookie on TLS
  - rate limit on current-password guesses
  - audit on logout
  - `request_id` on the static `/api/*` 404
  - compose actions return 200 when the daemon vanishes mid-action
  - create and duplicate leave the container behind when start fails
  - misleading "too large" message and 400 status on directory downloads
  - Content-Disposition strips non-ASCII characters
  - rollback failures are swallowed
  - env audit rows lack the environment id
  - one bad env row stops startup
  - `local` is re-registered after all envs are deleted
  - names over about 104 characters break recreate
