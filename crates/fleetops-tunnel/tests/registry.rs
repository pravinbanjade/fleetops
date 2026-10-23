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

#[tokio::test]
async fn a_node_reconnect_replaces_only_that_node() {
    let r = Registry::default();
    let (bye1, mut rx1) = mpsc::channel(1);
    let first = r.insert_node(7, "n1", "k", session(), info("0.7.0"), bye1);
    let (bye2, mut rx2) = mpsc::channel(1);
    r.insert_node(7, "n2", "k", session(), info("0.7.0"), bye2);
    let (bye3, _rx3) = mpsc::channel(1);
    let again = r.insert_node(7, "n1", "k", session(), info("0.7.1"), bye3);
    assert_eq!(rx1.recv().await, Some(ByeReason::Replaced));
    assert!(rx2.try_recv().is_err(), "n2 untouched");
    let nodes = r.nodes(7);
    assert_eq!(
        nodes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["n1", "n2"]
    );
    assert_eq!(r.get_node(7, "n1").unwrap().id, again);
    // The replaced session's cleanup leaves its successor alone.
    assert!(!r.remove_node(7, "n1", first, "dropped"));
    assert!(r.remove_node(7, "n1", again, "connection lost"));
    assert_eq!(
        r.node_offline_reason(7, "n1").as_deref(),
        Some("connection lost")
    );
    assert_eq!(r.connected(), 1);
}

#[tokio::test]
async fn revoke_says_bye_to_every_node() {
    let r = Registry::default();
    let (b1, mut rx1) = mpsc::channel(1);
    let (b2, mut rx2) = mpsc::channel(1);
    r.insert_node(4, "n1", "k", session(), info("0.7.0"), b1);
    r.insert_node(4, "n2", "k", session(), info("0.7.0"), b2);
    r.revoke(4);
    assert_eq!(rx1.recv().await, Some(ByeReason::Revoked));
    assert_eq!(rx2.recv().await, Some(ByeReason::Revoked));
}

#[tokio::test]
async fn revoke_tag_spares_sessions_on_the_new_key() {
    let r = Registry::default();
    let (b1, mut rx1) = mpsc::channel(1);
    let (b2, mut rx2) = mpsc::channel(1);
    r.insert_node(4, "n1", "old", session(), info("0.7.0"), b1);
    r.insert_node(4, "n2", "new", session(), info("0.7.0"), b2);
    r.revoke_tag(4, "old");
    assert_eq!(rx1.recv().await, Some(ByeReason::Revoked));
    assert!(rx2.try_recv().is_err());
}

#[tokio::test]
async fn get_ignores_swarm_nodes() {
    let r = Registry::default();
    let (b, _rx) = mpsc::channel(1);
    r.insert_node(7, "n1", "k", session(), info("0.7.0"), b);
    assert!(r.get(7).is_none());
    r.set_node_offline_reason(7, "n2", "not a member");
    assert_eq!(r.offline_reason(7), None);
    assert_eq!(
        r.node_offline_reason(7, "n2").as_deref(),
        Some("not a member")
    );
}
