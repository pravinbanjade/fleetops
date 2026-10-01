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
