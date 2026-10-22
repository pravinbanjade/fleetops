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

fn to_event(m: bollard::models::EventMessage) -> DockerEvent {
    let actor = m.actor.unwrap_or_default();
    let attributes: std::collections::BTreeMap<String, String> =
        actor.attributes.unwrap_or_default().into_iter().collect();
    DockerEvent {
        kind: m.typ.map(|t| t.to_string()).unwrap_or_default(),
        action: m.action.unwrap_or_default(),
        id: actor.id.unwrap_or_default(),
        name: attributes_name(&attributes),
        attributes,
        time: m.time.unwrap_or_default(),
    }
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
        let s = self
            .client
            .events(Some(opts))
            .map(|r| r.map(to_event).map_err(map_err));
        Box::pin(s)
    }

    async fn recent_events(&self, since: i64) -> Result<Vec<DockerEvent>> {
        // With `until`, Docker replays what it holds and ends the stream once
        // that time has passed. Times are whole seconds, so `until` is the next
        // second: this second's events are included, at the cost of a wait of
        // under a second.
        let opts = EventsOptions {
            since: Some(since.to_string()),
            until: Some((now_unix() + 1).to_string()),
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut s = self.client.events(Some(opts));
        while let Some(m) = s.next().await {
            out.push(to_event(m.map_err(map_err)?));
        }
        Ok(out)
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
        if !spec.args.is_empty() {
            o = o.buildargs(&spec.args.clone().into_iter().collect());
        }
        if let Some(t) = &spec.target {
            o = o.target(t);
        }
        if !spec.labels.is_empty() {
            o = o.labels(&spec.labels.clone().into_iter().collect());
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
