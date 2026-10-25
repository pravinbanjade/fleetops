mod common;

use common::{fake_swarm::connected_swarm as swarm, *};
use serde_json::json;

fn sub(
    n: u32,
    topic: &str,
    env: &str,
    id: Option<&str>,
    opts: serde_json::Value,
) -> serde_json::Value {
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
    // The container start that followed is the manager's own: not passed
    // on. The swarm network's creation is.
    let ev = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(ev["data"]["kind"], "network", "{ev}");
    let more = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        ws.until(|v| is(v, 1, "data")),
    )
    .await;
    assert!(more.is_err(), "{more:?}");
}

#[tokio::test]
async fn service_logs_stream_over_the_websocket() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(
        1,
        "logs",
        "prod",
        Some("service/s1"),
        json!({"follow": false}),
    ))
    .await;
    let batch = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(batch["data"][0]["text"], "hello from blog_web.1");
}

#[tokio::test]
async fn container_logs_on_a_node_view_work() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(
        1,
        "logs",
        "prod@n2",
        Some("c0ffee"),
        json!({"follow": false}),
    ))
    .await;
    let batch = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(batch["data"][0]["text"], "hello from blog_web.1");
}

#[tokio::test]
async fn a_bad_swarm_logs_id_is_rejected() {
    let sw = swarm().await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "logs", "prod", Some("c0ffee"), json!({})))
        .await;
    let err = ws.until(|v| v["type"] == "error").await;
    assert!(
        err.to_string().contains("a swarm logs id is service/<id>"),
        "{err}"
    );
}

fn fast() -> fleetops_api::ApiConfig {
    fleetops_api::ApiConfig {
        swarm_task_poll: std::time::Duration::from_millis(100),
        ..fleetops_api::ApiConfig::default()
    }
}

fn task_polls(sw: &common::fake_swarm::Swarm) -> usize {
    sw.daemon
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.contains("/tasks"))
        .count()
}

#[tokio::test]
async fn the_feed_sends_a_snapshot_then_only_changes() {
    let sw = common::fake_swarm::connected_swarm_with(fast()).await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "swarm-tasks", "prod", None, json!({})))
        .await;
    let first = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(first["data"]["type"], "snapshot");
    assert_eq!(first["data"]["tasks"].as_array().unwrap().len(), 2);
    {
        let mut tasks = sw.daemon.tasks.lock().unwrap();
        tasks[0]["Version"]["Index"] = json!(30);
        tasks[0]["Status"]["State"] = json!("shutdown");
        tasks.as_array_mut().unwrap().remove(1);
    }
    let next = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(next["data"]["type"], "changes");
    assert_eq!(next["data"]["upsert"][0]["id"], "t1");
    assert_eq!(next["data"]["upsert"][0]["state"], "shutdown");
    assert_eq!(next["data"]["remove"], json!(["t2"]));
    // Nothing changes: nothing is sent.
    let quiet = tokio::time::timeout(
        std::time::Duration::from_millis(400),
        ws.until(|v| is(v, 1, "data")),
    )
    .await;
    assert!(quiet.is_err(), "{quiet:?}");
}

#[tokio::test]
async fn one_feed_per_swarm_and_it_stops_without_subscribers() {
    let sw = common::fake_swarm::connected_swarm_with(fast()).await;
    let addr = sw.t.serve().await;
    let mut a = Ws::open(addr, &sw.admin).await;
    let mut b = Ws::open(addr, &sw.admin).await;
    a.send(sub(1, "swarm-tasks", "prod", None, json!({}))).await;
    b.send(sub(1, "swarm-tasks", "prod", None, json!({}))).await;
    a.until(|v| is(v, 1, "data")).await;
    b.until(|v| is(v, 1, "data")).await;
    let before = task_polls(&sw);
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    let polls = task_polls(&sw) - before;
    assert!((5..=13).contains(&polls), "one poller at 100 ms: {polls}");
    drop(a);
    drop(b);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let stopped = task_polls(&sw);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(task_polls(&sw), stopped, "no subscribers, no polling");
}

#[tokio::test]
async fn a_poll_error_pauses_and_recovers() {
    let sw = common::fake_swarm::connected_swarm_with(fast()).await;
    let mut ws = Ws::open(sw.t.serve().await, &sw.admin).await;
    ws.send(sub(1, "swarm-tasks", "prod", None, json!({})))
        .await;
    ws.until(|v| is(v, 1, "data")).await;
    sw.daemon
        .fail_tasks
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let paused = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(paused["data"]["type"], "paused");
    assert!(paused["data"]["reason"]
        .as_str()
        .unwrap()
        .contains("manager unavailable"));
    sw.daemon
        .fail_tasks
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let back = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(back["data"]["type"], "snapshot");
}

/// A trusted swarm "prod" with no agent connected.
async fn swarm_without_agents() -> (TestApp, Session) {
    let t = app_with(fast()).await;
    let admin = t.setup_admin().await;
    t.store
        .create_swarm_environment("prod", "h", fleetops_core::now_unix())
        .await
        .unwrap();
    t.state.envs.load(&t.store).await.unwrap();
    (t, admin)
}

#[tokio::test]
async fn the_task_feed_starts_paused_without_a_manager() {
    let (t, admin) = swarm_without_agents().await;
    let addr = t.serve().await;
    let mut a = Ws::open(addr, &admin).await;
    a.send(sub(1, "swarm-tasks", "prod", None, json!({}))).await;
    let first = a.until(|v| v["sub"] == 1).await;
    assert_eq!(first["type"], "data", "{first}");
    assert_eq!(first["data"]["type"], "paused");
    // A second subscriber to the paused feed is told so too.
    let mut b = Ws::open(addr, &admin).await;
    b.send(sub(1, "swarm-tasks", "prod", None, json!({}))).await;
    let first = b.until(|v| v["sub"] == 1).await;
    assert_eq!(first["data"]["type"], "paused", "{first}");
}

#[tokio::test]
async fn swarm_events_subscribe_without_a_manager() {
    let (t, admin) = swarm_without_agents().await;
    let mut ws = Ws::open(t.serve().await, &admin).await;
    ws.send(sub(1, "events", "prod", None, json!({}))).await;
    let err = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        ws.until(|v| v["type"] == "error"),
    )
    .await;
    assert!(err.is_err(), "refused: {err:?}");
}

#[tokio::test]
async fn swarm_events_follow_view_cells() {
    use fleetops_core::perm::{Row, Verb};
    let (sw, bob) = common::fake_swarm::with_custom(&[(Row::SwarmServices, Verb::View)]).await;
    let mut ws = Ws::open(sw.t.serve().await, &bob).await;
    ws.send(sub(1, "events", "prod", None, json!({}))).await;
    let ev = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(ev["data"]["kind"], "service");
    // The swarm network's creation needs networks.view.
    let more = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        ws.until(|v| is(v, 1, "data")),
    )
    .await;
    assert!(more.is_err(), "{more:?}");
}
