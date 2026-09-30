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
