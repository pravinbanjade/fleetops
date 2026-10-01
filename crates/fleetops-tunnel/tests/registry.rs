use fleetops_tunnel::{
    control::{ByeReason, HostInfo},
    registry::{AgentInfo, Registry},
    testing::loopback,
    Target,
};
use std::time::Duration;
use tokio::sync::mpsc;

fn info(v: &str) -> AgentInfo {
    AgentInfo {
        agent_version: v.into(),
        host: HostInfo::default(),
        connected_at: 0,
    }
}

fn session() -> fleetops_tunnel::Session {
    loopback(Target::Unix("/nonexistent.sock".into()))
}

#[tokio::test]
async fn a_newer_session_replaces_the_older_one() {
    let r = Registry::default();
    let (bye1, mut rx1) = mpsc::channel(1);
    let first = r.insert(7, session(), info("0.2.0"), bye1);
    let (bye2, _rx2) = mpsc::channel(1);
    let second = r.insert(7, session(), info("0.2.1"), bye2);
    assert_ne!(first, second);
    assert_eq!(rx1.recv().await, Some(ByeReason::Replaced));
    assert_eq!(r.get(7).unwrap().info.agent_version, "0.2.1");

    // The replaced session's cleanup must not remove its successor.
    assert!(!r.remove(7, first, "dropped"));
    assert!(r.get(7).is_some());
    assert!(r.remove(7, second, "connection lost"));
    assert!(r.get(7).is_none());
    assert_eq!(r.offline_reason(7).as_deref(), Some("connection lost"));
    assert_eq!(r.connected(), 0);
}

#[tokio::test]
async fn revoke_says_bye_to_the_live_session() {
    let r = Registry::default();
    let (bye, mut rx) = mpsc::channel(1);
    r.insert(3, session(), info("0.2.0"), bye);
    r.revoke(3);
    assert_eq!(rx.recv().await, Some(ByeReason::Revoked));
    r.revoke(99); // no session: nothing happens
}

#[tokio::test]
async fn waiters_wake_on_every_change() {
    let r = std::sync::Arc::new(Registry::default());
    let waiter = {
        let r = r.clone();
        tokio::spawn(async move { r.wait_change().await })
    };
    tokio::task::yield_now().await;
    let (bye, _rx) = mpsc::channel(1);
    r.insert(1, session(), info("0.2.0"), bye);
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("no wake")
        .unwrap();
}
