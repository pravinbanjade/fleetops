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
