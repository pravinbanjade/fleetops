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
