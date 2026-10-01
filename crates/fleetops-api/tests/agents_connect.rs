mod common;

use common::{agent::TestAgent, *};
use fleetops_tunnel::control::{ByeReason, Msg, PROTO_MAJOR};
use serde_json::json;
use std::time::Duration;

/// `req` sets no Origin; the agent command is built from it.
fn with_origin(
    mut r: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    r.headers_mut()
        .insert("origin", "http://127.0.0.1".parse().unwrap());
    r
}

async fn agent_env(t: &TestApp, admin: &Session, name: &str) -> String {
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": name, "kind": "docker-agent"})),
            Some(admin),
        )))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    r.json["enrollment"]["join_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The only agent environment in these tests.
async fn env_id(t: &TestApp) -> i64 {
    t.store.list_agents().await.unwrap()[0].environment_id
}

async fn base(t: &TestApp) -> String {
    format!("http://{}", t.serve().await)
}

async fn enroll(base: &str, token: &str) -> (TestAgent, String) {
    let mut a = TestAgent::connect(base, token).await.unwrap();
    a.hello(PROTO_MAJOR).await;
    match a.welcome().await {
        Msg::Welcome {
            agent_key: Some(k),
            env_name,
            ..
        } => {
            assert_eq!(env_name, "web-01");
            (a, k)
        }
        m => panic!("{m:?}"),
    }
}

#[tokio::test]
async fn a_join_token_enrolls_and_the_key_reconnects() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;

    let (a, key) = enroll(&base, &token).await;
    assert!(key.starts_with("fk_"));
    eventually("registered", || t.state.agents.connected() == 1).await;
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert_eq!(list.json[0]["agent"]["connected"], true);
    assert_eq!(list.json[0]["agent"]["enrolled"], true);
    assert_eq!(list.json[0]["agent"]["agent_version"], "0.2.0-test");
    drop(a);
    eventually("unregistered", || t.state.agents.connected() == 0).await;

    // The join token is spent; the key works.
    assert_eq!(TestAgent::connect(&base, &token).await.err(), Some(401));
    let mut again = TestAgent::connect(&base, &key).await.unwrap();
    again.hello(PROTO_MAJOR).await;
    assert!(matches!(
        again.welcome().await,
        Msg::Welcome {
            agent_key: None,
            ..
        }
    ));
}

#[tokio::test]
async fn a_join_token_enrolls_exactly_one_agent() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (a, b) = tokio::join!(
        TestAgent::connect(&base, &token),
        TestAgent::connect(&base, &token)
    );
    let mut winners = 0;
    for agent in [a, b] {
        let Ok(mut agent) = agent else { continue };
        agent.hello(PROTO_MAJOR).await;
        match agent.next().await {
            Some(Msg::Welcome {
                agent_key: Some(_), ..
            }) => winners += 1,
            Some(Msg::Welcome {
                agent_key: None, ..
            }) => panic!("a second agent got in without a key"),
            _ => {} // refused after the upgrade: the other one won the race
        }
    }
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn bad_and_missing_credentials_are_401() {
    let t = app().await;
    let admin = t.setup_admin().await;
    agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    assert_eq!(TestAgent::connect(&base, "fj_nope").await.err(), Some(401));
    assert_eq!(TestAgent::connect(&base, "fk_nope").await.err(), Some(401));
    assert_eq!(
        TestAgent::connect(&base, "session-cookie-value")
            .await
            .err(),
        Some(401)
    );
}

#[tokio::test]
async fn repeated_bad_credentials_are_rate_limited() {
    let t = app().await;
    let base = base(&t).await;
    let mut last = None;
    for _ in 0..8 {
        last = TestAgent::connect(&base, "fk_wrong").await.err();
    }
    assert_eq!(last, Some(429));
}

#[tokio::test]
async fn an_incompatible_agent_is_told_why_and_the_reason_is_shown() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let mut a = TestAgent::connect(&base, &token).await.unwrap();
    a.hello(PROTO_MAJOR + 1).await;
    assert_eq!(
        a.next().await,
        Some(Msg::Bye {
            reason: ByeReason::Incompatible
        })
    );
    a.closed().await;
    // API tests build environments with FakeFactory, so the env's own
    // status is the fake's; the stored reason is what the API serves here.
    // (AgentDockerEnv's down-with-reason is covered in fleetops-docker.)
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let err = list.json[0]["agent"]["last_error"].as_str().unwrap();
    assert!(err.contains("protocol 2"), "{err}");
    assert_eq!(
        t.state.agents.offline_reason(env_id(&t).await).as_deref(),
        Some(err)
    );
    // The join token is not spent by a refused agent.
    let (_ok, _key) = enroll(&base, &token).await;
}

#[tokio::test]
async fn a_second_connection_replaces_the_first() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut first, key) = enroll(&base, &token).await;
    let mut second = TestAgent::connect(&base, &key).await.unwrap();
    second.hello(PROTO_MAJOR).await;
    second.welcome().await;
    assert_eq!(
        first.next().await,
        Some(Msg::Bye {
            reason: ByeReason::Replaced
        })
    );
    first.closed().await;
    assert_eq!(t.state.agents.connected(), 1);
}

#[tokio::test]
async fn rotating_revokes_the_live_session_and_the_key() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, key) = enroll(&base, &token).await;
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/web-01/agent/rotate",
            None,
            Some(&admin),
        )))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(
        a.next().await,
        Some(Msg::Bye {
            reason: ByeReason::Revoked
        })
    );
    a.closed().await;
    assert_eq!(TestAgent::connect(&base, &key).await.err(), Some(401));
}

#[tokio::test]
async fn deleting_an_agent_environment_revokes_its_session() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, _key) = enroll(&base, &token).await;
    let r = t
        .send(req("DELETE", "/api/v1/envs/web-01", None, Some(&admin)))
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(
        a.next().await,
        Some(Msg::Bye {
            reason: ByeReason::Revoked
        })
    );
    a.closed().await;
    eventually("registry emptied", || t.state.agents.connected() == 0).await;
}

#[tokio::test]
async fn an_agent_that_stops_answering_pings_is_dropped() {
    let t = app_with(fleetops_api::ApiConfig {
        agent_ping_interval: Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (mut a, _key) = enroll(&base, &token).await;
    // Answer one ping, then go silent.
    if let Some(Msg::Ping { n }) = a.next().await {
        a.pong(n).await;
    }
    a.closed().await;
    eventually("unregistered", || t.state.agents.connected() == 0).await;
    let rows = t.store.list_audit(50, None).await.unwrap();
    assert!(rows.iter().any(|r| r.action == "agent.enroll"));
    assert!(rows.iter().any(|r| r.action == "agent.disconnect"));
}

#[tokio::test]
async fn the_server_opens_data_streams_to_the_agent() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (_a, _key) = enroll(&base, &token).await;
    eventually("registered", || t.state.agents.connected() == 1).await;
    let live = t.state.agents.get(env_id(&t).await).unwrap();
    let mut s = live.session.open_stream().await.unwrap();
    s.write_all(b"echo").await.unwrap();
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"echo");
}

#[tokio::test]
async fn flapping_agents_audit_one_disconnect_per_five_minutes() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (a, key) = enroll(&base, &token).await;
    drop(a);
    for _ in 0..3 {
        eventually("unregistered", || t.state.agents.connected() == 0).await;
        let mut b = TestAgent::connect(&base, &key).await.unwrap();
        b.hello(PROTO_MAJOR).await;
        b.welcome().await;
        drop(b);
    }
    eventually("unregistered", || t.state.agents.connected() == 0).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let n = t
        .store
        .list_audit(100, None)
        .await
        .unwrap()
        .iter()
        .filter(|r| r.action == "agent.disconnect")
        .count();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn rotating_during_the_handshake_still_revokes_the_session() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = agent_env(&t, &admin, "web-01").await;
    let base = base(&t).await;
    let (a, key) = enroll(&base, &token).await;
    drop(a);
    eventually("unregistered", || t.state.agents.connected() == 0).await;

    // The key passes the check at upgrade; the rotate lands before hello.
    let mut b = TestAgent::connect(&base, &key).await.unwrap();
    let r = t
        .send(with_origin(req(
            "POST",
            "/api/v1/envs/web-01/agent/rotate",
            None,
            Some(&admin),
        )))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    b.hello(PROTO_MAJOR).await;
    loop {
        match b.next().await {
            Some(Msg::Welcome { .. }) => continue,
            Some(Msg::Bye { reason }) => {
                assert_eq!(reason, ByeReason::Revoked);
                break;
            }
            other => panic!("expected bye(revoked), got {other:?}"),
        }
    }
    b.closed().await;
    eventually("not registered", || t.state.agents.connected() == 0).await;
}
