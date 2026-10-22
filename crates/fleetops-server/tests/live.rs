//! End to end against a real Docker daemon: REST plus every websocket topic
//! through the full app. Enable with `--features docker-it` (see
//! `make test-docker`).
#![cfg(feature = "docker-it")]

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    Router,
};
use fleetops_api::{envs::bootstrap_local, ApiConfig, AppState};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tower::ServiceExt;

struct Live {
    router: Router,
    cookie: String,
    csrf: String,
}

impl Live {
    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call_from(None, method, uri, body).await
    }

    /// `origin` is what a browser would send; agent enrollment needs it.
    async fn call_from(
        &self,
        origin: Option<&str>,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .header("x-csrf-token", &self.csrf);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        if let Some(o) = origin {
            b = b.header("origin", o);
        }
        let mut r = b
            .body(body.map_or(Body::empty(), |v| Body::from(v.to_string())))
            .unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        let res = self.router.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

async fn start() -> (Live, SocketAddr) {
    start_with(ApiConfig::default()).await
}

async fn start_with(config: ApiConfig) -> (Live, SocketAddr) {
    let host = std::env::var("DOCKER_HOST").expect("DOCKER_HOST (make test-docker sets it)");
    let store = Store::in_memory().await.unwrap();
    // The server's own wiring: one agent registry for factory and state.
    let state = fleetops_server::build_state(store.clone(), config);
    bootstrap_local(&store, &state.envs, &host)
        .await
        .unwrap()
        .expect("local environment registered");
    let router = fleetops_api::app(state, Router::new());
    let mut r = Request::post("/api/v1/auth/setup")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"username": "admin", "password": "correct-horse"}).to_string(),
        ))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let res = router.clone().oneshot(r).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let live = Live {
        router: router.clone(),
        cookie,
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (live, addr)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(ws: &mut Socket) -> Message {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("frame within 30s")
            .expect("socket open")
            .unwrap();
        if !matches!(m, Message::Ping(_) | Message::Pong(_)) {
            return m;
        }
    }
}

/// JSON frames until `pred` matches.
async fn until(ws: &mut Socket, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        if let Message::Text(t) = next(ws).await {
            let v: Value = serde_json::from_str(&t).unwrap();
            if pred(&v) {
                return v;
            }
        }
    }
}

#[tokio::test]
async fn every_topic_works_against_the_real_daemon() {
    let (live, addr) = start().await;
    let mut req = format!("ws://{addr}/api/v1/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("cookie", live.cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let send = |v: Value| Message::Text(v.to_string().into());

    ws.send(send(
        json!({"op": "sub", "sub": 1, "topic": "events", "env": "local"}),
    ))
    .await
    .unwrap();
    let name = format!("fleetops-live-{}", std::process::id());
    let (status, created) = live
        .call(
            "POST",
            "/api/v1/envs/local/containers",
            Some(json!({
                "name": name, "image": "alpine:3.20",
                "cmd": ["sh", "-c", "echo hello-live; exec sleep 300"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let started = until(&mut ws, |v| {
        v["sub"] == 1 && v["data"]["id"] == id.as_str() && v["data"]["action"] == "start"
    })
    .await;
    assert_eq!(started["data"]["name"], name.as_str());

    ws.send(send(
        json!({"op": "sub", "sub": 2, "topic": "logs", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let logs = until(&mut ws, |v| v["sub"] == 2 && v["type"] == "data").await;
    assert_eq!(logs["data"][0]["text"], "hello-live");

    ws.send(send(
        json!({"op": "sub", "sub": 3, "topic": "stats", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let stats = until(&mut ws, |v| v["sub"] == 3 && v["type"] == "data").await;
    assert!(
        stats["data"]["memory_limit"].as_u64().unwrap() > 0,
        "{stats}"
    );
    ws.send(send(json!({"op": "unsub", "sub": 3})))
        .await
        .unwrap();

    ws.send(send(
        json!({"op": "sub", "sub": 4, "topic": "exec", "env": "local", "id": id}),
    ))
    .await
    .unwrap();
    let mut input = 4u32.to_be_bytes().to_vec();
    input.extend(b"echo exec-$((2+3)); exit\n");
    ws.send(Message::Binary(input.into())).await.unwrap();
    let mut out = Vec::new();
    let end = loop {
        match next(&mut ws).await {
            Message::Binary(b) if b[..4] == 4u32.to_be_bytes() => out.extend(&b[4..]),
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["sub"] == 4 && v["type"] == "end" {
                    break v;
                }
            }
            _ => {}
        }
    };
    assert!(String::from_utf8_lossy(&out).contains("exec-5"));
    assert_eq!(end["exit_code"], 0, "{end}");

    let (status, task) = live
        .call(
            "POST",
            "/api/v1/envs/local/images/pull",
            Some(json!({"image": "alpine:3.20"})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    ws.send(send(
        json!({"op": "sub", "sub": 5, "topic": "task", "id": task["task_id"]}),
    ))
    .await
    .unwrap();
    let done = until(&mut ws, |v| v["sub"] == 5 && v["type"] == "end").await;
    assert_eq!(done["reason"], "succeeded", "{done}");

    let (status, _) = live
        .call(
            "DELETE",
            &format!("/api/v1/envs/local/containers/{id}?force=true"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    until(&mut ws, |v| {
        v["sub"] == 1 && v["data"]["id"] == id.as_str() && v["data"]["action"] == "destroy"
    })
    .await;
}

#[tokio::test]
async fn an_agent_environment_serves_the_same_api_as_local() {
    let (live, addr) = start().await;
    let session = connect_agent(&live, addr).await;

    let (_, status) = live.call("GET", "/api/v1/envs/remote/status", None).await;
    assert_eq!(status["state"], "up", "{status}");
    // Same daemon behind both environments. (Container counts race with
    // the other live test, so compare the daemon's identity instead.)
    let (_, local) = live.call("GET", "/api/v1/envs/local/info", None).await;
    let (_, remote) = live.call("GET", "/api/v1/envs/remote/info", None).await;
    assert!(remote["ID"].is_string(), "{remote}");
    assert_eq!(local["ID"], remote["ID"], "same daemon");
    let (status, list) = live
        .call("GET", "/api/v1/envs/remote/containers", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(list.is_array());

    session.close().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, status) = live.call("GET", "/api/v1/envs/remote/status", None).await;
    assert_eq!(status["state"], "down", "{status}");
}

/// Enrolls environment `remote` and connects a minimal in-process agent to
/// it: the real tunnel, forwarding to DOCKER_HOST.
async fn connect_agent(live: &Live, addr: SocketAddr) -> fleetops_tunnel::Session {
    use fleetops_tunnel::control::*;
    let (status, created) = live
        .call_from(
            Some(&format!("http://{addr}")),
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "remote", "kind": "docker-agent"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["enrollment"]["join_token"]
        .as_str()
        .unwrap()
        .to_string();

    // A minimal agent in-process: the real tunnel, forwarding to DOCKER_HOST.
    let target = fleetops_tunnel::Target::parse(&std::env::var("DOCKER_HOST").unwrap()).unwrap();
    let mut req = format!("ws://{addr}/api/v1/agent/connect")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let session = fleetops_tunnel::Session::spawn(
        fleetops_tunnel::ws::tungstenite_io(ws),
        fleetops_tunnel::Role::Client,
        move |s| {
            let target = target.clone();
            tokio::spawn(async move {
                let _ = fleetops_tunnel::forward(s, &target).await;
            });
        },
    );
    let control = session.open_stream().await.unwrap();
    let (mut rx, mut tx) = tokio::io::split(control);
    write_msg(
        &mut tx,
        &Msg::Hello {
            proto: PROTO_MAJOR,
            agent_version: "it".into(),
            host: HostInfo::default(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_msg(&mut rx).await.unwrap(),
        Some(Msg::Welcome { .. })
    ));
    // Answer pings in the background.
    tokio::spawn(async move {
        while let Ok(Some(m)) = read_msg(&mut rx).await {
            if let Msg::Ping { n } = m {
                let _ = write_msg(&mut tx, &Msg::Pong { n }).await;
            }
        }
    });
    session
}

/// The stack's containers as (service, id, state), from the API.
async fn stack_containers(live: &Live, stack: &str) -> Vec<(String, String, String)> {
    let (_, list) = live
        .call("GET", "/api/v1/envs/local/containers", None)
        .await;
    let mut out: Vec<(String, String, String)> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["compose_project"] == stack)
        .map(|c| {
            (
                c["compose_service"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                c["id"].as_str().unwrap().to_string(),
                c["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

async fn deploy_stack(live: &Live, stack: &str, from: Value) {
    deploy_stack_in(live, "local", stack, from).await
}

async fn deploy_stack_in(live: &Live, env: &str, stack: &str, from: Value) {
    let base = format!("/api/v1/envs/{env}/stacks/{stack}");
    let (status, plan) = live
        .call("POST", &format!("{base}/plan"), Some(from.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let token = plan["plan"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("no plan: {plan}"));
    let (status, started) = live
        .call(
            "POST",
            &format!("{base}/deploy"),
            Some(json!({"from": from, "plan_token": token})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let id = started["task_id"].as_str().unwrap();
    for _ in 0..600 {
        let (_, t) = live.call("GET", &format!("/api/v1/tasks/{id}"), None).await;
        if t["status"] != "running" {
            assert_eq!(t["status"], "succeeded", "{t}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("deploy of {stack} did not finish");
}

#[tokio::test]
async fn stack_lifecycle_against_docker() {
    let (live, _) = start().await;
    let stack = format!("fleetops-it-stack-{}", std::process::id());
    let compose = |level: &str| {
        format!(
            r#"
services:
  db:
    image: alpine:3.20
    command: sleep 300
    restart: unless-stopped
    healthcheck: {{ test: ["CMD", "true"], interval: 1s, retries: 5 }}
    volumes: ["data:/data"]
  web:
    image: alpine:3.20
    command: sleep 300
    restart: unless-stopped
    environment: {{ LEVEL: {level} }}
    ports: ["80"]
    depends_on: {{ db: {{ condition: service_healthy }} }}
volumes:
  data:
"#
        )
    };
    let base = format!("/api/v1/envs/local/stacks/{stack}");
    let (status, body) = live
        .call(
            "POST",
            "/api/v1/envs/local/stacks",
            Some(json!({"name": stack, "compose": compose("info")})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    deploy_stack(&live, &stack, json!({"source": "draft"})).await;
    let first = stack_containers(&live, &stack).await;
    assert_eq!(first.len(), 2, "{first:?}");
    assert!(first.iter().all(|c| c.2 == "running"), "{first:?}");

    let (status, _) = live
        .call(
            "PUT",
            &format!("{base}/draft"),
            Some(json!({"compose": compose("debug")})),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    deploy_stack(&live, &stack, json!({"source": "draft"})).await;
    let second = stack_containers(&live, &stack).await;
    assert_eq!(second[0].1, first[0].1, "db is untouched");
    assert_ne!(second[1].1, first[1].1, "web is recreated");

    deploy_stack(&live, &stack, json!({"source": "revision", "revision": 1})).await;
    let (_, inspect) = live
        .call(
            "GET",
            &format!("/api/v1/envs/local/containers/{stack}-web-1"),
            None,
        )
        .await;
    let env = inspect["Config"]["Env"].as_array().unwrap();
    assert!(env.iter().any(|e| e == "LEVEL=info"), "{env:?}");

    let (status, _) = live
        .call("DELETE", &format!("{base}?volumes=true"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(stack_containers(&live, &stack).await.is_empty());
    let (_, nets) = live.call("GET", "/api/v1/envs/local/networks", None).await;
    assert!(nets
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n["name"] != format!("{stack}_default")));
    let (_, vols) = live.call("GET", "/api/v1/envs/local/volumes", None).await;
    assert!(vols
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["name"] != format!("{stack}_data")));
}

/// Downloads a file from a container through the Files API, as text.
async fn container_file(
    live: &Live,
    env: &str,
    container: &str,
    path: &str,
) -> (StatusCode, String) {
    let mut r = Request::get(format!(
        "/api/v1/envs/{env}/containers/{container}/files/download?path={path}"
    ))
    .header("cookie", &live.cookie)
    .body(Body::empty())
    .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let res = live.router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn stack_secrets_against_docker() {
    let (live, _) = start().await;
    let stack = format!("fleetops-it-secrets-{}", std::process::id());
    // `check` logs in over TCP with the secret, so the deploy (which waits
    // for it to exit 0) succeeds only if postgres took the password from
    // its file.
    let compose = r#"
services:
  db:
    image: postgres:16-alpine
    environment: { POSTGRES_PASSWORD_FILE: /run/secrets/db_password }
    secrets: [db_password]
    configs: [{ source: app_conf, target: /etc/app.conf }]
    healthcheck: { test: ["CMD", "pg_isready", "-U", "postgres", "-h", "127.0.0.1"], interval: 1s, retries: 60 }
  check:
    image: postgres:16-alpine
    command: ["sh", "-c", "PGPASSWORD=$$(cat /run/secrets/db_password) psql -h db -U postgres -tAc 'select 1'"]
    secrets: [db_password]
    restart: "no"
    depends_on: { db: { condition: service_healthy } }
  done:
    image: postgres:16-alpine
    command: ["true"]
    restart: "no"
    depends_on: { check: { condition: service_completed_successfully } }
secrets:
  db_password: { file: ./pw.txt }
configs:
  app_conf: { content: "hello" }
"#;
    let (status, body) = live
        .call(
            "POST",
            "/api/v1/envs/local/stacks",
            Some(json!({"name": stack, "compose": compose})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let base = format!("/api/v1/envs/local/stacks/{stack}");
    let (status, body) = live
        .call(
            "PUT",
            &format!("{base}/files/secret/db_password"),
            Some(json!({"value": "Pg-s3cret-it"})),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    deploy_stack(&live, &stack, json!({"source": "draft"})).await;

    let db = format!("{stack}-db-1");
    let (status, text) = container_file(&live, "local", &db, "/etc/app.conf").await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "hello"));
    let (_, etc) = live
        .call(
            "GET",
            &format!("/api/v1/envs/local/containers/{db}/files?path=/etc"),
            None,
        )
        .await;
    let conf = etc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "app.conf")
        .unwrap_or_else(|| panic!("no app.conf: {etc}"));
    assert_eq!(conf["mode"].as_u64().unwrap() & 0o777, 0o444, "{conf}");
    let (status, text) = container_file(&live, "local", &db, "/run/secrets/db_password").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(!text.contains("Pg-s3cret-it"));
    // `/var/run` is a symlink to `/run` in the image, and Docker follows it.
    let (status, text) = container_file(&live, "local", &db, "/var/run/secrets/db_password").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(!text.contains("Pg-s3cret-it"));

    let (status, _) = live
        .call("DELETE", &format!("{base}?volumes=true"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn stack_configs_through_an_agent() {
    let (live, addr) = start().await;
    let session = connect_agent(&live, addr).await;
    let stack = format!("fleetops-it-agent-conf-{}", std::process::id());
    let compose = r#"
services:
  app:
    image: alpine:3.20
    command: sleep 300
    restart: unless-stopped
    configs: [{ source: app_conf, target: /etc/app.conf }]
configs:
  app_conf: { content: "through the agent" }
"#;
    let (status, body) = live
        .call(
            "POST",
            "/api/v1/envs/remote/stacks",
            Some(json!({"name": stack, "compose": compose})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    deploy_stack_in(&live, "remote", &stack, json!({"source": "draft"})).await;
    let (status, text) =
        container_file(&live, "remote", &format!("{stack}-app-1"), "/etc/app.conf").await;
    assert_eq!(
        (status, text.as_str()),
        (StatusCode::OK, "through the agent")
    );
    let (status, _) = live
        .call(
            "DELETE",
            &format!("/api/v1/envs/remote/stacks/{stack}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    session.close().await;
}

/// Removes an image the test built, through the API.
async fn remove_image(live: &Live, env: &str, tag: &str) {
    let (status, body) = live
        .call(
            "DELETE",
            &format!("/api/v1/envs/{env}/images/{tag}?force=true"),
            None,
        )
        .await;
    assert!(status.is_success(), "{tag}: {status} {body}");
}

/// The image the stack's `service` container runs.
async fn container_image(live: &Live, env: &str, container: &str) -> String {
    let (_, inspect) = live
        .call(
            "GET",
            &format!("/api/v1/envs/{env}/containers/{container}"),
            None,
        )
        .await;
    inspect["Config"]["Image"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn stack_builds_against_docker() {
    use fleetops_git::testing::{GitServer, ServerOpts, TestRepo};
    let data = tempfile::tempdir().unwrap();
    let (live, _) = start_with(ApiConfig {
        data_dir: Some(data.path().to_path_buf()),
        git_allow_http: true,
        ..Default::default()
    })
    .await;
    let repo = TestRepo::new("main");
    let server = GitServer::start(&repo.root(), ServerOpts::default()).await;
    repo.commit(
        &[
            (
                "compose.yaml",
                "services:\n  api:\n    build: ./app\n    command: sleep 300\n    restart: unless-stopped\n",
            ),
            ("app/Dockerfile", "FROM alpine:3.20\nCOPY msg /msg\n"),
            ("app/msg", "one"),
        ],
        "one",
    );
    let stack = format!("fleetops-it-build-{}", std::process::id());
    let base = format!("/api/v1/envs/local/stacks/{stack}");
    let (status, body) = live
        .call(
            "POST",
            "/api/v1/envs/local/stacks",
            Some(json!({"name": stack, "compose": ""})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = live
        .call(
            "PUT",
            &format!("{base}/git"),
            Some(json!({"url": server.url("repo.git"), "branch": "main", "sync": "manual"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    git_sync(&live, &base).await;
    deploy_stack(&live, &stack, json!({"source": "draft"})).await;
    let api = format!("{stack}-api-1");
    let (status, text) = container_file(&live, "local", &api, "/msg").await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "one"));
    let first = container_image(&live, "local", &api).await;

    repo.commit(&[("app/msg", "two")], "two");
    git_sync(&live, &base).await;
    deploy_stack(&live, &stack, json!({"source": "draft"})).await;
    let (_, text) = container_file(&live, "local", &api, "/msg").await;
    assert_eq!(text, "two");
    let second = container_image(&live, "local", &api).await;
    assert_ne!(first, second);

    let (status, _) = live.call("DELETE", &base, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for tag in [first, second] {
        remove_image(&live, "local", &tag).await;
    }
}

#[tokio::test]
async fn inline_dockerfile_builds_through_an_agent() {
    let (live, addr) = start().await;
    let session = connect_agent(&live, addr).await;
    let stack = format!("fleetops-it-inline-build-{}", std::process::id());
    let compose = r#"
services:
  web:
    build:
      dockerfile_inline: "FROM alpine:3.20\nRUN echo hi > /hi"
    command: sleep 300
    restart: unless-stopped
"#;
    let (status, body) = live
        .call(
            "POST",
            "/api/v1/envs/remote/stacks",
            Some(json!({"name": stack, "compose": compose})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    deploy_stack_in(&live, "remote", &stack, json!({"source": "draft"})).await;
    let web = format!("{stack}-web-1");
    let (status, text) = container_file(&live, "remote", &web, "/hi").await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "hi\n"));
    let image = container_image(&live, "remote", &web).await;
    let (status, _) = live
        .call(
            "DELETE",
            &format!("/api/v1/envs/remote/stacks/{stack}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    remove_image(&live, "remote", &image).await;
    session.close().await;
}

/// Syncs the stack's repo, waiting out a sync already running (attaching
/// git starts one).
async fn git_sync(live: &Live, base: &str) {
    for _ in 0..100 {
        let (status, out) = live.call("POST", &format!("{base}/git/sync"), None).await;
        assert_eq!(status, StatusCode::OK, "{out}");
        if out["outcome"] != "queued" {
            assert_ne!(out["outcome"], "failed", "{out}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the sync stayed queued");
}
