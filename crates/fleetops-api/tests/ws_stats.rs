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
