mod common;

use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_role_change_ends_every_subscription_on_every_tab() {
    let t = app_with(ws_config()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "apps", "p");
    let r = t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let id = r.json["id"].as_i64().unwrap();
    t.grant("bob", "k", "viewer", None).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let addr = t.serve().await;
    let mut tab1 = Ws::open(addr, &bob).await;
    let mut tab2 = Ws::open(addr, &bob).await;
    for ws in [&mut tab1, &mut tab2] {
        ws.send(json!({"op": "sub", "sub": 1, "topic": "kube-watch", "env": "k", "id": "pods/-"}))
            .await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    t.send(req(
        "PATCH",
        &format!("/api/v1/users/{id}"),
        Some(json!({"role": "admin"})),
        Some(&admin),
    ))
    .await;
    // Connection-level frames drain first, so the order of the two is not fixed.
    for ws in [&mut tab1, &mut tab2] {
        let (mut end, mut told) = (None, false);
        while end.is_none() || !told {
            let m = ws.json().await;
            match m["type"].as_str() {
                Some("end") if m["sub"] == 1 => end = Some(m),
                Some("access_changed") => told = true,
                _ => {}
            }
        }
        let end = end.unwrap();
        assert_eq!(end["reason"], "access");
        assert_eq!(end["message"], "your access changed");
    }
}

#[tokio::test]
async fn another_users_change_leaves_a_connection_alone() {
    let t = app_with(ws_config()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let addr = t.serve().await;
    let mut ws = Ws::open(addr, &admin).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "kube-watch", "env": "k", "id": "pods/-"}))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    t.state.hubs.access_changed(9999);
    ws.quiet(200).await;
}

async fn started_exec(t: &TestApp) -> common::fake_kube::ExecHandle {
    for _ in 0..100 {
        if let Some(h) = t.fake.kube.take_exec() {
            return h;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("exec did not start");
}

#[tokio::test]
async fn a_revoked_terminal_takes_no_more_input_even_when_the_client_is_behind() {
    let t = app_with(ws_config()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "apps", "p");
    let r = t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let id = r.json["id"].as_i64().unwrap();
    t.grant("bob", "k", "operator", None).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let mut ws = Ws::open(t.serve().await, &bob).await;
    ws.send(
        json!({"op": "sub", "sub": 7, "topic": "exec", "env": "k", "id": "apps/p",
        "opts": {"cmd": ["sh"], "cols": 80, "rows": 24}}),
    )
    .await;
    let mut h = started_exec(&t).await;
    // The client stops reading while the shell prints a lot: the server's
    // socket backs up and the subscription's queue fills.
    let out = h.output.clone();
    tokio::spawn(async move {
        for _ in 0..4096 {
            if out.send(Ok(vec![b'x'; 64 * 1024])).await.is_err() {
                return;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    t.state.hubs.access_changed(id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut frame = 7u32.to_be_bytes().to_vec();
    frame.extend(b"id\n");
    ws.send_binary(frame).await;
    let got = tokio::time::timeout(Duration::from_secs(1), h.input.recv()).await;
    assert!(
        !matches!(got, Ok(Some(_))),
        "input reached the shell after access was revoked"
    );
}

#[tokio::test]
async fn a_running_task_keeps_reporting_through_an_access_change() {
    use std::sync::atomic::Ordering;
    let t = app_with(ws_config()).await;
    let admin = t.with_local().await;
    let r = t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let id = r.json["id"].as_i64().unwrap();
    t.grant("bob", "local", "operator", None).await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    t.fake.hold_pulls.store(true, Ordering::SeqCst);
    let r = t
        .send(req(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine"})),
            Some(&bob),
        ))
        .await;
    let task = r.json["task_id"].as_str().unwrap().to_string();
    let mut ws = Ws::open(t.serve().await, &bob).await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "task", "id": task}))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    t.state.hubs.access_changed(id);
    ws.until(|m| m["type"] == "access_changed").await;
    t.fake.hold_pulls.store(false, Ordering::SeqCst);
    t.fake.release_pulls.notify_one();
    let end = ws.until(|m| m["type"] == "end" && m["sub"] == 1).await;
    assert_eq!(end["reason"], "succeeded", "{end}");
}
