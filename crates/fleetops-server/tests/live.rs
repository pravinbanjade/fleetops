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
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .header("x-csrf-token", &self.csrf);
        if body.is_some() {
            b = b.header("content-type", "application/json");
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
    let host = std::env::var("DOCKER_HOST").expect("DOCKER_HOST (make test-docker sets it)");
    let store = Store::in_memory().await.unwrap();
    let state = AppState::new(
        store.clone(),
        ApiConfig::default(),
        Arc::new(DockerEnvFactory::default()),
    );
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
