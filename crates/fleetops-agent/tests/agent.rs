use fleetops_agent::Config;
use fleetops_api::{ApiConfig, AppState};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Answers every request with a fixed /version body.
fn fake_docker(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("docker.sock");
    let l = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = c.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let body =
                        r#"{"Version":"27.0.0","ApiVersion":"1.47","Os":"linux","Arch":"x86_64"}"#;
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                    if c.write_all(resp.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    path
}

struct Server {
    base: String,
    state: AppState,
    task: tokio::task::JoinHandle<()>,
}

async fn server(store: Store, port: u16) -> Server {
    server_with(store, port, ApiConfig::default()).await
}

async fn server_with(store: Store, port: u16, config: ApiConfig) -> Server {
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    let state = AppState::new(
        store.clone(),
        config,
        Arc::new(DockerEnvFactory::new(agents.clone())),
    )
    .with_agents(agents);
    state.envs.load(&store).await.unwrap();
    let app = fleetops_api::app(state.clone(), axum::Router::new());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server { base, state, task }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Creates the environment straight in the store; returns (env id, join token).
async fn agent_env(store: &Store) -> (i64, String) {
    let token = fleetops_api::agents::new_join_token();
    let hash = fleetops_api::auth_token_hash(&token);
    let env = store
        .create_agent_environment(
            "web-01",
            fleetops_core::env::EnvKind::DockerAgent,
            &hash,
            fleetops_core::now_unix() + 900,
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    (env.id, token)
}

fn config(
    base: &str,
    data: &std::path::Path,
    docker: &std::path::Path,
    join: Option<String>,
) -> Config {
    Config {
        server: base.into(),
        join_token: join,
        data: data.into(),
        docker: format!("unix://{}", docker.display()),
        ca: None,
        log: "warn".into(),
        server_silence: Duration::from_secs(45),
    }
}

#[tokio::test]
async fn the_agent_enrolls_and_its_environment_comes_up() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("data");
    tokio::spawn(fleetops_agent::run(config(
        &srv.base,
        &data,
        &docker,
        Some(token),
    )));

    eventually("connected", || srv.state.agents.connected() == 1).await;
    assert!(data.join("agent.key").exists());
    let env = srv.state.envs.get("web-01").unwrap();
    match env.status().await {
        fleetops_core::env::EnvStatus::Up {
            version: fleetops_core::env::EnvVersion::Docker(version),
        } => assert_eq!(version.version, "27.0.0"),
        other => panic!("{other:?}"),
    }
    assert!(store.get_agent(env_id).await.unwrap().unwrap().enrolled);
}

#[tokio::test]
async fn agent_reconnects_after_the_server_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let port: u16 = srv.base.rsplit(':').next().unwrap().parse().unwrap();
    let data = dir.path().join("data");
    tokio::spawn(fleetops_agent::run(config(
        &srv.base,
        &data,
        &docker,
        Some(token),
    )));
    eventually("connected", || srv.state.agents.connected() == 1).await;

    // Stop the old process: its listener and its live tunnel. (axum's
    // per-connection tasks outlive the aborted serve future.)
    srv.task.abort();
    let _ = srv.task.await;
    srv.state.agents.get(env_id).unwrap().session.close().await;
    // A new process: same store and port, empty registry.
    let srv = server(store.clone(), port).await;
    for _ in 0..100 {
        if srv.state.agents.connected() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        srv.state.agents.connected(),
        1,
        "the agent came back with its key"
    );
}

#[tokio::test]
async fn a_revoked_key_stops_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("data");
    let agent = tokio::spawn(fleetops_agent::run(config(
        &srv.base,
        &data,
        &docker,
        Some(token),
    )));
    eventually("connected", || srv.state.agents.connected() == 1).await;

    store
        .rotate_agent(env_id, "unused", fleetops_core::now_unix() + 900)
        .await
        .unwrap();
    srv.state.agents.revoke(env_id);
    let res = tokio::time::timeout(Duration::from_secs(5), agent)
        .await
        .expect("agent kept running")
        .unwrap();
    let err = res.unwrap_err().to_string();
    assert!(err.contains("revoked"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn unwritable_data_dir_fails_before_connecting() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("ro");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(data.join("x"), b"x").is_ok() {
        return; // running as root
    }
    let err = fleetops_agent::run(config(&srv.base, &data, &docker, Some(token)))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("must be writable"), "{err:#}");
    assert!(
        !store.get_agent(env_id).await.unwrap().unwrap().enrolled,
        "the join token was not spent"
    );
}

#[tokio::test]
async fn a_silent_server_is_dropped_and_redialed() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (_env_id, token) = agent_env(&store).await;
    // A server that never pings: what a half-open connection looks like
    // from the agent (server crashed, NAT dropped the mapping).
    let srv = server_with(
        store.clone(),
        0,
        ApiConfig {
            agent_ping_interval: Duration::from_secs(3600),
            ..ApiConfig::default()
        },
    )
    .await;
    let data = dir.path().join("data");
    let mut cfg = config(&srv.base, &data, &docker, Some(token));
    cfg.server_silence = Duration::from_millis(500);
    tokio::spawn(fleetops_agent::run(cfg));

    let connects = || async {
        store
            .list_audit(100, None)
            .await
            .unwrap()
            .iter()
            .filter(|r| r.action == "agent.connect")
            .count()
    };
    for _ in 0..60 {
        if connects().await >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the agent never gave up on a server that stopped pinging");
}

#[tokio::test]
async fn a_new_join_command_re_enrolls_over_a_rotated_key() {
    let dir = tempfile::tempdir().unwrap();
    let docker = fake_docker(dir.path());
    let store = Store::in_memory().await.unwrap();
    let (env_id, token) = agent_env(&store).await;
    let srv = server(store.clone(), 0).await;
    let data = dir.path().join("data");
    let first = tokio::spawn(fleetops_agent::run(config(
        &srv.base,
        &data,
        &docker,
        Some(token),
    )));
    eventually("connected", || srv.state.agents.connected() == 1).await;
    let old_key = std::fs::read_to_string(data.join("agent.key")).unwrap();

    // The admin rotates; the old agent is told and stops.
    let token2 = fleetops_api::agents::new_join_token();
    store
        .rotate_agent(
            env_id,
            &fleetops_api::auth_token_hash(&token2),
            fleetops_core::now_unix() + 900,
        )
        .await
        .unwrap();
    srv.state.agents.revoke(env_id);
    let _ = tokio::time::timeout(Duration::from_secs(5), first).await;
    eventually("old session gone", || srv.state.agents.connected() == 0).await;

    // The new command runs on the same host, with the same data volume.
    let second = tokio::spawn(fleetops_agent::run(config(
        &srv.base,
        &data,
        &docker,
        Some(token2),
    )));
    for _ in 0..100 {
        if srv.state.agents.connected() == 1 {
            break;
        }
        assert!(!second.is_finished(), "{:?}", second.await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        srv.state.agents.connected(),
        1,
        "the new join command did not reconnect"
    );
    let new_key = std::fs::read_to_string(data.join("agent.key")).unwrap();
    assert_ne!(new_key, old_key, "a fresh key replaced the rotated one");
}
