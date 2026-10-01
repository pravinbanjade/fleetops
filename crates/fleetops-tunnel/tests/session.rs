use fleetops_tunnel::{Role, Session, StreamOpener};
use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

/// A server session whose agent side echoes every stream back.
fn echo_pair() -> (Session, Session) {
    let (a, b) = duplex(1 << 20);
    let server = Session::spawn(a, Role::Server, |_s| {});
    let agent = Session::spawn(b, Role::Client, |s| {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(s);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    });
    (server, agent)
}

#[tokio::test]
async fn server_opens_streams_the_agent_serves() {
    let (server, _agent) = echo_pair();
    let mut tasks = Vec::new();
    for i in 0..3 {
        let server = server.clone();
        tasks.push(tokio::spawn(async move {
            let mut s = server.open().await.unwrap();
            let msg = format!("hello-{i}");
            s.write_all(msg.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
            let mut back = String::new();
            s.read_to_string(&mut back).await.unwrap();
            assert_eq!(back, msg);
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
}

#[tokio::test]
async fn a_slow_reader_stalls_only_its_own_stream() {
    let (server, _agent) = echo_pair();
    // Stream A: write far more than the 256 KiB window and never read the echo.
    let mut a = server.open_stream().await.unwrap();
    let stalled = tokio::time::timeout(Duration::from_millis(500), async {
        let chunk = vec![7u8; 64 * 1024];
        for _ in 0..128 {
            a.write_all(&chunk).await.unwrap();
        }
    })
    .await;
    assert!(stalled.is_err(), "8 MiB went through with nobody reading");

    // Stream B still round-trips.
    let mut b = server.open_stream().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        b.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    })
    .await
    .expect("stream B was blocked by stream A");
}

#[tokio::test]
async fn closing_the_session_ends_open_streams() {
    let (server, agent) = echo_pair();
    let mut s = server.open_stream().await.unwrap();
    s.write_all(b"x").await.unwrap();
    let mut one = [0u8; 1];
    s.read_exact(&mut one).await.unwrap();

    server.close().await;

    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(1), s.read(&mut buf))
        .await
        .expect("an open stream hung after its session closed");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    tokio::time::timeout(Duration::from_secs(1), agent.closed())
        .await
        .expect("the agent side never noticed");
    assert!(server.is_closed());
    assert!(server.open_stream().await.is_err());
}

/// A session whose agent side answers each stream once (as Docker would,
/// which also acknowledges the stream in yamux) and then holds it open.
fn silent_pair() -> (Session, Session) {
    let (a, b) = duplex(1 << 20);
    let server = Session::spawn(a, Role::Server, |_s| {});
    let agent = Session::spawn(b, Role::Client, |mut s| {
        tokio::spawn(async move {
            let _ = s.write_all(b"k").await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
            drop(s);
        });
    });
    (server, agent)
}

#[tokio::test]
async fn closing_wakes_a_reader_already_waiting() {
    let (server, _agent) = echo_pair();
    let mut s = server.open_stream().await.unwrap();
    s.write_all(b"x").await.unwrap();
    let mut one = [0u8; 1];
    s.read_exact(&mut one).await.unwrap();
    // A follow-logs or terminal reader, parked waiting for more.
    let reader = tokio::spawn(async move {
        let mut b = [0u8; 16];
        s.read(&mut b).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_secs(2), server.close())
        .await
        .expect("close hung");
    let r = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .expect("a waiting reader hung after the session closed")
        .unwrap();
    assert!(matches!(r, Ok(0) | Err(_)), "{r:?}");
}

#[tokio::test]
async fn too_many_streams_fails_that_open_only() {
    let (server, _agent) = silent_pair();
    let mut held = Vec::new();
    for _ in 0..fleetops_tunnel::MAX_STREAMS {
        let mut s = server.open_stream().await.unwrap();
        s.write_all(b"x").await.unwrap();
        let mut k = [0u8; 1];
        s.read_exact(&mut k).await.unwrap();
        held.push(s);
    }
    let busy = server.open_stream().await;
    assert!(busy.is_err(), "the stream over the limit was opened");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !server.is_closed(),
        "the whole session died at the stream limit"
    );
    // A freed stream makes room again.
    drop(held.pop());
    tokio::time::timeout(Duration::from_secs(2), server.open_stream())
        .await
        .expect("no room after a stream closed")
        .expect("open after a stream closed");
}
