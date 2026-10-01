use fleetops_tunnel::{testing::loopback, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn targets_parse_from_docker_urls() {
    assert_eq!(
        Target::parse("unix:///var/run/docker.sock").unwrap(),
        Target::Unix("/var/run/docker.sock".into())
    );
    assert_eq!(
        Target::parse("tcp://10.0.0.5:2375").unwrap(),
        Target::Tcp("10.0.0.5:2375".into())
    );
    assert_eq!(
        Target::parse("http://localhost:2375").unwrap(),
        Target::Tcp("localhost:2375".into())
    );
    assert!(Target::parse("ssh://host").is_err());
    assert!(Target::parse("unix://").is_err());
}

#[tokio::test]
async fn streams_are_piped_to_a_unix_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("echo.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = c.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });

    let server = loopback(Target::Unix(path));
    let mut s = server.open_stream().await.unwrap();
    s.write_all(b"through the tunnel").await.unwrap();
    let mut buf = [0u8; 18];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"through the tunnel");
}

#[tokio::test]
async fn an_unreachable_target_resets_the_stream() {
    let server = loopback(Target::Unix("/nonexistent/fleetops.sock".into()));
    let mut s = server.open_stream().await.unwrap();
    let _ = s.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(std::time::Duration::from_secs(1), s.read(&mut buf))
        .await
        .expect("hung");
    assert!(matches!(n, Ok(0) | Err(_)));
}
