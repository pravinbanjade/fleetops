mod common;

use common::fake_kube::ExecHandle;
use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::{json, Value};
use std::time::Duration;

async fn kube_ws() -> (TestApp, Session, Ws) {
    let t = app_with(ws_config()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "apps", "web-1");
    let ws = Ws::open(t.serve().await, &admin).await;
    (t, admin, ws)
}

async fn started_exec(t: &TestApp) -> ExecHandle {
    for _ in 0..100 {
        if let Some(h) = t.fake.kube.take_exec() {
            return h;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("exec did not start");
}

#[tokio::test]
async fn kube_logs_stream_over_the_shared_socket() {
    let (t, _admin, mut ws) = kube_ws().await;
    t.fake.kube.set_logs(&["one", "two"]);
    ws.send(
        json!({"op": "sub", "sub": 1, "topic": "logs", "env": "k", "id": "apps/web-1",
        "opts": {"follow": false, "container": "main"}}),
    )
    .await;
    let data = ws.until(|m| m["type"] == "data").await;
    let texts: Vec<Value> = data["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].clone())
        .collect();
    assert_eq!(texts, [json!("one"), json!("two")]);
    ws.until(|m| m["type"] == "end").await;
    assert_eq!(
        t.fake.kube.log_opts.lock().unwrap()[0].container.as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn a_kube_pod_id_must_be_namespace_slash_pod() {
    let (_t, _admin, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "logs", "env": "k", "id": "web-1"}))
        .await;
    let e = ws.until(|m| m["type"] == "error").await;
    assert_eq!(e["code"], "invalid");
}

#[tokio::test]
async fn kube_exec_carries_input_resize_output_and_the_exit_code() {
    let (t, admin, mut ws) = kube_ws().await;
    ws.send(
        json!({"op": "sub", "sub": 7, "topic": "exec", "env": "k", "id": "apps/web-1",
        "opts": {"cmd": ["sh"], "cols": 80, "rows": 24}}),
    )
    .await;
    let mut h = started_exec(&t).await;
    assert_eq!(h.target, "apps/web-1");
    let mut frame = 7u32.to_be_bytes().to_vec();
    frame.extend(b"ls\n");
    ws.send_binary(frame).await;
    assert_eq!(h.input.recv().await.unwrap(), b"ls\n");
    ws.send(json!({"op": "resize", "sub": 7, "cols": 100, "rows": 40}))
        .await;
    assert_eq!(h.resize.recv().await.unwrap(), (100, 40));
    h.output.send(Ok(b"bin etc".to_vec())).await.unwrap();
    match ws.frame().await.unwrap() {
        WsMessage::Binary(b) => {
            assert_eq!(&b[..4], &7u32.to_be_bytes());
            assert_eq!(&b[4..], b"bin etc");
        }
        other => panic!("expected binary, got {other:?}"),
    }
    drop(h.output);
    h.exit.send(Some(0)).unwrap();
    let end = ws.until(|m| m["type"] == "end").await;
    assert_eq!(end["reason"], "exited");
    assert_eq!(end["exit_code"], 0);
    let audit = t
        .send(req("GET", "/api/v1/audit", None, Some(&admin)))
        .await;
    let text = audit.json.to_string();
    assert!(
        text.contains("k8s.exec") && text.contains("pods/apps/web-1"),
        "{text}"
    );
}

#[tokio::test]
async fn a_kube_exec_whose_pod_disappears_ends_promptly() {
    let (t, _admin, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 3, "topic": "exec", "env": "k", "id": "apps/web-1"}))
        .await;
    // Output and exit both dropped: the pod is gone.
    drop(started_exec(&t).await);
    let end = tokio::time::timeout(Duration::from_secs(2), ws.until(|m| m["type"] == "end"))
        .await
        .expect("a prompt end");
    assert_eq!(end["exit_code"], Value::Null);
}

#[tokio::test]
async fn docker_topics_on_a_kube_environment_are_refused() {
    let (_t, _admin, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events", "env": "k"}))
        .await;
    let e = ws.until(|m| m["type"] == "error").await;
    assert_eq!(e["code"], "conflict");
}

#[tokio::test]
async fn a_kube_pod_id_must_be_kubernetes_names() {
    let (_t, _admin, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "logs", "env": "k", "id": "apps/web-1?x=y"}))
        .await;
    let e = ws.until(|m| m["type"] == "error").await;
    assert_eq!(e["code"], "invalid");
}

use fleetops_core::kube::WatchEvent;

async fn watch_sub(ws: &mut Ws, sub: u32, id: &str) {
    ws.send(json!({"op": "sub", "sub": sub, "topic": "kube-watch", "env": "k", "id": id}))
        .await;
}

async fn opened(t: &TestApp, n: usize) {
    for _ in 0..100 {
        if t.fake.kube.open_watches() == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "expected {n} open watches, have {}",
        t.fake.kube.open_watches()
    );
}

#[tokio::test]
async fn a_kube_watch_streams_a_snapshot_then_changes() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "pods/apps").await;
    opened(&t, 1).await;
    let row = common::fake_kube::row("web-1", Some("apps"));
    t.fake.kube.emit_watch(
        KubeKind::Pods,
        Ok(WatchEvent::Snapshot {
            rows: vec![row.clone()],
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"]["type"], "snapshot");
    assert_eq!(m["data"]["rows"][0]["name"], "web-1");
    t.fake.kube.emit_watch(
        KubeKind::Pods,
        Ok(WatchEvent::Deleted {
            namespace: Some("apps".into()),
            name: "web-1".into(),
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(
        m["data"],
        json!({"type": "deleted", "namespace": "apps", "name": "web-1"})
    );
    assert!(t.fake.kube.calls().contains(&"watch pods apps".to_string()));
}

#[tokio::test]
async fn two_tabs_share_one_upstream_watch() {
    let (t, admin, mut ws) = kube_ws().await;
    let mut ws2 = Ws::open(t.serve().await, &admin).await;
    watch_sub(&mut ws, 1, "pods/-").await;
    watch_sub(&mut ws2, 1, "pods/-").await;
    opened(&t, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        t.fake
            .kube
            .calls()
            .iter()
            .filter(|c| c.starts_with("watch"))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_failed_watch_ends_with_the_reason() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 4, "secrets/-").await;
    opened(&t, 1).await;
    t.fake.kube.emit_watch(
        KubeKind::Secrets,
        Err(fleetops_core::Error::Forbidden(
            "secrets is forbidden".into(),
        )),
    );
    let m = ws.until(|m| m["type"] == "end").await;
    assert_eq!(
        (m["sub"].as_u64(), m["reason"].as_str()),
        (Some(4), Some("error"))
    );
    assert_eq!(m["message"], "secrets is forbidden");
}

#[tokio::test]
async fn a_watch_id_must_be_kind_slash_namespace() {
    let (_t, _admin, mut ws) = kube_ws().await;
    for (sub, bad) in [(1, "pods"), (2, "widgets/apps"), (3, "pods/Not_A_NS")] {
        watch_sub(&mut ws, sub, bad).await;
        let e = ws.until(|m| m["type"] == "error").await;
        assert!(
            e["code"] == "invalid" || e["code"] == "not_found",
            "{bad}: {e}"
        );
    }
}

#[tokio::test]
async fn unsubscribing_stops_the_watch_after_the_grace_period() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "pods/apps").await;
    opened(&t, 1).await;
    ws.send(json!({"op": "unsub", "sub": 1})).await;
    opened(&t, 0).await;
}

#[tokio::test]
async fn deleting_an_environment_ends_its_watches() {
    let (t, admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 2, "pods/apps").await;
    opened(&t, 1).await;
    let r = t
        .send(req("DELETE", "/api/v1/envs/k", None, Some(&admin)))
        .await;
    assert!(r.status.is_success(), "{:?}", r.json);
    let m = tokio::time::timeout(Duration::from_secs(5), ws.until(|m| m["type"] == "end"))
        .await
        .expect("the watch did not end when its environment was deleted");
    assert_eq!(
        (m["sub"].as_u64(), m["reason"].as_str()),
        (Some(2), Some("error"))
    );
    assert_eq!(m["message"], "the environment was removed or changed");
    opened(&t, 0).await;
}

async fn limited_ws() -> (TestApp, Ws) {
    let t = app_with(ws_config()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "team-b", "db-1");
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "operator", Some(&["team-a"])).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let ws = Ws::open(t.serve().await, &bob).await;
    (t, ws)
}

#[tokio::test]
async fn a_limited_watch_sees_only_its_namespaces() {
    let (t, mut ws) = limited_ws().await;
    watch_sub(&mut ws, 1, "pods/-").await;
    opened(&t, 1).await;
    let a = common::fake_kube::row("web-1", Some("team-a"));
    let b = common::fake_kube::row("db-1", Some("team-b"));
    t.fake.kube.emit_watch(
        KubeKind::Pods,
        Ok(WatchEvent::Snapshot {
            rows: vec![a.clone(), b.clone()],
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"]["rows"].as_array().unwrap().len(), 1);
    t.fake
        .kube
        .emit_watch(KubeKind::Pods, Ok(WatchEvent::Added { row: b }));
    t.fake.kube.emit_watch(
        KubeKind::Pods,
        Ok(WatchEvent::Added {
            row: common::fake_kube::row("web-2", Some("team-a")),
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(
        m["data"]["row"]["name"], "web-2",
        "the team-b add never arrives"
    );
}

#[tokio::test]
async fn limited_subscribers_share_the_upstream_watch() {
    let (t, mut ws) = limited_ws().await;
    let admin = session_from(&t.login(ADMIN, ADMIN_PW).await);
    let mut ws2 = Ws::open(t.serve().await, &admin).await;
    watch_sub(&mut ws, 1, "pods/-").await;
    watch_sub(&mut ws2, 1, "pods/-").await;
    opened(&t, 1).await;
}

#[tokio::test]
async fn topics_outside_the_set_are_refused() {
    let (_t, mut ws) = limited_ws().await;
    for (sub, topic, id) in [
        (1, "kube-watch", "pods/team-b"),
        (2, "logs", "team-b/db-1"),
        (3, "exec", "team-b/db-1"),
    ] {
        ws.send(json!({"op": "sub", "sub": sub, "topic": topic, "env": "k", "id": id}))
            .await;
        let e = ws.until(|m| m["type"] == "error" && m["sub"] == sub).await;
        assert_eq!(e["code"], "forbidden", "{topic}");
        assert_eq!(e["message"], "namespace 'team-b' is not in your access");
    }
}

#[tokio::test]
async fn a_filtered_watch_is_its_own_upstream_and_keeps_only_warnings() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "events/-/warnings").await;
    watch_sub(&mut ws, 2, "events/-").await;
    opened(&t, 2).await;
    let mut warn = common::fake_kube::row("a.1", Some("apps"));
    warn.status = "Warning".into();
    let mut normal = common::fake_kube::row("b.1", Some("apps"));
    normal.status = "Normal".into();
    t.fake.kube.emit_watch(
        KubeKind::Events,
        Ok(WatchEvent::Snapshot {
            rows: vec![warn, normal],
        }),
    );
    let m = ws.until(|m| m["type"] == "data" && m["sub"] == 1).await;
    let names: Vec<_> = m["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].clone())
        .collect();
    assert_eq!(names, [json!("a.1")]);
    watch_sub(&mut ws, 3, "pods/-/warnings").await;
    let e = ws.until(|m| m["type"] == "error" && m["sub"] == 3).await;
    assert_eq!(e["code"], "invalid");
}

fn widget_discovery() -> fleetops_core::kube::Discovery {
    fleetops_core::kube::Discovery {
        kinds: vec![common::fake_kube::widgets(true)],
        failed_groups: vec![],
    }
}

#[tokio::test]
async fn a_generic_watch_streams_rows_of_a_discovered_kind() {
    let (t, _admin, mut ws) = kube_ws().await;
    t.fake.kube.set_discovery(widget_discovery());
    watch_sub(&mut ws, 1, "r/example.fleetops.io/v1/widgets/apps").await;
    eventually("the generic watch opens", || {
        t.fake
            .kube
            .calls()
            .contains(&"gwatch example.fleetops.io/v1/widgets apps".to_string())
    })
    .await;
    t.fake.kube.emit_generic_watch(
        &common::fake_kube::widgets(true).api,
        Ok(WatchEvent::Snapshot {
            rows: vec![common::fake_kube::row("w1", Some("apps"))],
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"]["type"], "snapshot");
    assert_eq!(m["data"]["rows"][0]["name"], "w1");
}

#[tokio::test]
async fn a_limited_generic_watch_sees_only_its_namespaces() {
    let (t, mut ws) = limited_ws().await;
    t.fake.kube.set_discovery(widget_discovery());
    watch_sub(&mut ws, 1, "r/example.fleetops.io/v1/widgets/-").await;
    eventually("the generic watch opens", || {
        t.fake
            .kube
            .calls()
            .iter()
            .any(|c| c.starts_with("gwatch example.fleetops.io/v1/widgets"))
    })
    .await;
    t.fake.kube.emit_generic_watch(
        &common::fake_kube::widgets(true).api,
        Ok(WatchEvent::Snapshot {
            rows: vec![
                common::fake_kube::row("a", Some("team-a")),
                common::fake_kube::row("b", Some("team-b")),
            ],
        }),
    );
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(m["data"]["rows"][0]["name"], "a");
}

#[tokio::test]
async fn a_generic_watch_id_must_name_group_version_resource_and_namespace() {
    let (_t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "r/example.fleetops.io/v1").await;
    let e = ws.until(|m| m["type"] == "error" && m["sub"] == 1).await;
    assert_eq!(e["code"], "invalid");
}
