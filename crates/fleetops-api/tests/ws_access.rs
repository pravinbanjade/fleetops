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
