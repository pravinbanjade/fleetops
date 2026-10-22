use fleetops_git::testing::{SshOpts, SshServer, TestRepo};
use fleetops_git::{fetch_commit, fetch_file, generate_key, has_commit, KeyPair, Limits, Source};
use std::sync::Arc;
use std::time::{Duration, Instant};

const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

fn ssh_src(server: &SshServer, key: Option<&KeyPair>) -> Source {
    Source {
        url: server.url("repo.git"),
        branch: "main".into(),
        path: "compose.yaml".into(),
        token: None,
        ssh_key: key.map(|k| k.private.clone()),
        host_key: None,
        seen_host_key: Default::default(),
    }
}

async fn served(opts: impl FnOnce(&KeyPair) -> SshOpts) -> (TestRepo, SshServer, KeyPair) {
    let key = generate_key("test");
    let repo = TestRepo::new("main");
    let server = SshServer::start(&repo.root(), opts(&key)).await;
    (repo, server, key)
}

fn authorised(key: &KeyPair) -> SshOpts {
    SshOpts {
        authorized: vec![key.public.clone()],
        accept_env: true,
        hold: None,
    }
}

#[tokio::test]
async fn fetches_a_file_over_ssh() {
    let (repo, server, key) = served(authorised).await;
    let one = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = ssh_src(&server, Some(&key));
    let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(
        (f.commit.as_str(), f.content.as_str()),
        (one.as_str(), COMPOSE)
    );
    assert_eq!(src.seen_host_key.get(), Some(server.host_key.clone()));
    let two = repo.commit(&[("compose.yaml", "services: {}\n")], "two");
    let f = fetch_file(&dir, &src, Limits::default()).await.unwrap();
    assert_eq!(
        (f.commit.as_str(), f.content.as_str()),
        (two.as_str(), "services: {}\n")
    );
}

#[tokio::test]
async fn fetches_a_commit_by_id_over_ssh() {
    let (repo, server, key) = served(authorised).await;
    let one = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    repo.commit(&[("compose.yaml", "services: {}\n")], "two");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    fetch_commit(&dir, &ssh_src(&server, Some(&key)), &one, Limits::default())
        .await
        .unwrap();
    assert!(has_commit(&dir, &one));
}

#[tokio::test]
async fn v1_server_refuses_unadvertised_commits() {
    let (repo, server, key) = served(|k| SshOpts {
        accept_env: false,
        ..authorised(k)
    })
    .await;
    let one = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let two = repo.commit(&[("compose.yaml", "services: {}\n")], "two");
    let cache = tempfile::tempdir().unwrap();
    let src = ssh_src(&server, Some(&key));
    let f = fetch_file(&cache.path().join("1"), &src, Limits::default())
        .await
        .unwrap();
    assert_eq!(f.commit, two);
    let e = fetch_commit(&cache.path().join("2"), &src, &one, Limits::default())
        .await
        .unwrap_err();
    assert_eq!(
        e.detail(),
        format!("commit {} cannot be fetched", &one[..7])
    );
}

#[tokio::test]
async fn missing_key() {
    let (repo, server, _) = served(authorised).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let e = fetch_file(
        &cache.path().join("1"),
        &ssh_src(&server, None),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.detail(), "the repository needs an SSH key");
}

#[tokio::test]
async fn refused_key_still_reports_the_host_key() {
    let (repo, server, _) = served(authorised).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let other = generate_key("other");
    let cache = tempfile::tempdir().unwrap();
    let src = ssh_src(&server, Some(&other));
    let e = fetch_file(&cache.path().join("1"), &src, Limits::default())
        .await
        .unwrap_err();
    assert_eq!(e.detail(), "the repository refused the SSH key");
    assert_eq!(src.seen_host_key.get(), Some(server.host_key.clone()));
    let e = fetch_commit(
        &cache.path().join("2"),
        &src,
        &"a".repeat(40),
        Limits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.detail(), "the repository refused the SSH key");
    assert!(!e.detail().contains(&other.private[40..80]));
}

#[tokio::test]
async fn changed_host_key_fetches_nothing() {
    let (repo, server, key) = served(authorised).await;
    let head = repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = Source {
        host_key: Some("ssh-ed25519 SHA256:other".into()),
        ..ssh_src(&server, Some(&key))
    };
    let e = fetch_file(&dir, &src, Limits::default()).await.unwrap_err();
    let fp = server.host_key.split_once(' ').unwrap().1;
    assert_eq!(
        e.detail(),
        format!("the host key of 127.0.0.1 changed (now {fp}); if that is expected, accept the new key in the Git tab")
    );
    assert!(!has_commit(&dir, &head));
    assert_eq!(src.seen_host_key.get(), Some(server.host_key.clone()));
    // The pinned key itself passes.
    let src = Source {
        host_key: Some(server.host_key.clone()),
        ..ssh_src(&server, Some(&key))
    };
    fetch_file(&dir, &src, Limits::default()).await.unwrap();
}

#[tokio::test]
async fn ssh_timeout_releases_the_cache() {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    let (repo, server, key) = served(|k| SshOpts {
        hold: Some(hold.clone()),
        ..authorised(k)
    })
    .await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("1");
    let src = ssh_src(&server, Some(&key));
    let start = Instant::now();
    let e = fetch_file(
        &dir,
        &src,
        Limits {
            timeout: Duration::from_secs(1),
            ..Limits::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        e.detail(),
        "fetching the repository took longer than 2 minutes"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    // The cut ended the session (the server saw the client go), so its
    // held fetch gives up the one permit to the next fetch.
    let gone = Instant::now();
    while server.sessions.load(std::sync::atomic::Ordering::SeqCst) > 0 {
        assert!(
            gone.elapsed() < Duration::from_secs(5),
            "the SSH session was left open"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    hold.add_permits(1);
    let f = tokio::time::timeout(
        Duration::from_secs(10),
        fetch_file(&dir, &src, Limits::default()),
    )
    .await
    .expect("the cache lock was released")
    .unwrap();
    assert_eq!(f.content, COMPOSE);
}

#[tokio::test]
async fn path_with_a_space_and_a_quote() {
    let (repo, server, key) = served(authorised).await;
    repo.commit(&[("compose.yaml", COMPOSE)], "one");
    std::fs::rename(
        repo.root().join("repo.git"),
        repo.root().join("it's here.git"),
    )
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let src = Source {
        url: server.url("it's%20here.git"),
        ..ssh_src(&server, Some(&key))
    };
    let f = fetch_file(&cache.path().join("1"), &src, Limits::default())
        .await
        .unwrap();
    assert_eq!(f.content, COMPOSE);
}

#[tokio::test]
async fn connect_timeout_during_kex_closes_the_socket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Says hello, then never answers the key exchange.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"SSH-2.0-stall\r\n").await.unwrap();
        let mut buf = [0u8; 4096];
        while sock.read(&mut buf).await.is_ok_and(|n| n > 0) {}
        let _ = closed_tx.send(());
    });
    let key = generate_key("k");
    let src = Source {
        url: format!("ssh://git@127.0.0.1:{port}/repo.git"),
        branch: "main".into(),
        path: "compose.yaml".into(),
        token: None,
        ssh_key: Some(key.private.clone()),
        host_key: None,
        seen_host_key: Default::default(),
    };
    let cache = tempfile::tempdir().unwrap();
    let limits = Limits {
        timeout: Duration::from_secs(1),
        ..Limits::default()
    };
    let e = fetch_file(&cache.path().join("1"), &src, limits)
        .await
        .unwrap_err();
    assert!(e.detail().contains("took longer than"), "{}", e.detail());
    tokio::time::timeout(Duration::from_secs(5), closed_rx)
        .await
        .expect("the stalled SSH connection was left open")
        .unwrap();
}
