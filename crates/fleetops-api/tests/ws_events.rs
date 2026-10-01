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

#[tokio::test]
async fn events_stream_closes_even_while_events_keep_flowing() {
    // A busy host (health checks, CI) never goes quiet; the shared stream
    // must still stop when its last subscriber leaves.
    let t = app().await;
    let admin = t.with_local().await;
    let addr = t.serve().await;
    let fake = t.fake.clone();
    let emitter = tokio::spawn(async move {
        loop {
            fake.emit_event(event("busy", "exec_start"));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });
    for round in 0..3 {
        let mut ws = Ws::open(addr, &admin).await;
        ws.send(ws_sub(1, "events", None, Value::Null)).await;
        ws.until(|v| is(v, 1, "data")).await;
        drop(ws);
        eventually(&format!("stream closed after round {round}"), || {
            t.fake.open_event_streams() == 0
        })
        .await;
    }
    emitter.abort();
}

#[tokio::test]
async fn quick_restart_is_announced_even_when_status_never_fails() {
    // Docker restarted between two status checks: the stream dropped, but
    // every check the hub makes finds Docker up again.
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(ws_sub(1, "events", None, Value::Null)).await;
    for round in 0..2 {
        eventually("events stream", || t.fake.open_event_streams() == 1).await;
        t.fake.end_event_streams();
        let down = ws.until(|v| v["type"] == "env_status").await;
        assert_eq!(down["status"]["state"], "down", "round {round}");
        let up = ws.until(|v| v["type"] == "env_status").await;
        assert_eq!(up["status"]["state"], "up", "round {round}");
    }
}

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
    let up = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        ws.until(|v| v["type"] == "env_status"),
    )
    .await
    .expect("the backoff was not cut short");
    assert_eq!(up["status"]["state"], "up");
}
