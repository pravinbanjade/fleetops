mod common;

use common::{fake_swarm::connected_swarm as swarm, *};
use serde_json::json;

fn sub(n: u32, topic: &str, env: &str, id: Option<&str>, opts: serde_json::Value) -> serde_json::Value {
    json!({"op": "sub", "sub": n, "topic": topic, "env": env, "id": id, "opts": opts})
}

#[tokio::test]
async fn swarm_events_reach_subscribers_filtered() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "events", "prod", None, json!({}))).await;
    let ev = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(ev["data"]["kind"], "service");
    assert_eq!(ev["data"]["action"], "update");
    // The container start that followed is the manager's own: not passed on.
    let more = tokio::time::timeout(std::time::Duration::from_millis(500), ws.until(|v| is(v, 1, "data"))).await;
    assert!(more.is_err(), "{more:?}");
}

#[tokio::test]
async fn service_logs_stream_over_the_websocket() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "logs", "prod", Some("service/s1"), json!({"follow": false}))).await;
    let batch = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(batch["data"][0]["text"], "hello from blog_web.1");
}

#[tokio::test]
async fn container_logs_on_a_node_view_work() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "logs", "prod@n2", Some("c0ffee"), json!({"follow": false}))).await;
    let batch = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(batch["data"][0]["text"], "hello from blog_web.1");
}

#[tokio::test]
async fn a_bad_swarm_logs_id_is_rejected() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "logs", "prod", Some("c0ffee"), json!({}))).await;
    let err = ws.until(|v| v["type"] == "error").await;
    assert!(err.to_string().contains("a swarm logs id is service/<id>"), "{err}");
}
