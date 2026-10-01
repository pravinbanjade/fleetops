//! Long-lived Docker streams: events, stats, logs, exec, and image
//! pull/build progress. Implementations return `'static` boxed streams so
//! the websocket layer can own them independently of the request.

use crate::Result;
use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, pin::Pin};
use utoipa::ToSchema;

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T>> + Send>>;

/// One Docker event, reduced to what the UI needs to patch its caches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogLine {
    pub stream: LogStream,
    /// One line without its trailing newline. With `timestamps`, Docker's
    /// RFC 3339 timestamp and a space come first.
    pub text: String,
}

pub const LOG_TAIL_DEFAULT: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
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
    /// The events Docker still holds from `since` (Unix seconds) up to now,
    /// oldest first. Docker keeps only its most recent events in memory.
    async fn recent_events(&self, since: i64) -> Result<Vec<DockerEvent>>;
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
