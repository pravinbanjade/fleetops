mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use common::git::*;
use fleetops_api::stacks::git::poll_once;
use fleetops_git::testing::{GitServer, ServerOpts};
use fleetops_store::TokenChange;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SECRET: &str = "hook-secret";

fn hook(id: i64, query: &str, headers: &[(&str, String)], body: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/stacks/hooks/{id}{query}"))
        .header("content-type", "application/json");
    for (k, v) in headers {
        b = b.header(*k, v);
    }
    let mut r = b.body(Body::from(body.to_string())).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 7], 40000))));
    r
}

fn github(body: &str) -> Vec<(&'static str, String)> {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    vec![(
        "x-hub-signature-256",
        format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
    )]
}

const PUSH_MAIN: &str = r#"{"ref":"refs/heads/main"}"#;

async fn webhook_app() -> GitApp {
    git_app(Some(("webhook", false)), ServerOpts::default()).await
}

#[tokio::test]
async fn github_push_accepted_and_synced() {
    let g = webhook_app().await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let r = g
        .send(hook(g.stack.id, "", &github(PUSH_MAIN), PUSH_MAIN))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    assert_eq!(r.json["status"], "accepted");
    for _ in 0..400 {
        if g.git().await.pending_commit.as_deref() == Some(head.as_str()) {
            let row = g
                .store
                .list_audit(100, None)
                .await
                .unwrap()
                .into_iter()
                .find(|e| e.action == "stack.git.webhook")
                .unwrap();
            assert_eq!(row.result, "success");
            assert_eq!(row.target_id.as_deref(), Some("shop"));
            assert_eq!(row.detail.unwrap()["provider"], "github");
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the webhook never synced");
}

#[tokio::test]
async fn gitlab_and_query_secret_accepted() {
    let g = webhook_app().await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let r = g
        .send(hook(
            g.stack.id,
            "",
            &[("x-gitlab-token", SECRET.into())],
            PUSH_MAIN,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    let r = g
        .send(hook(g.stack.id, &format!("?secret={SECRET}"), &[], "{}"))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
}

#[tokio::test]
async fn bad_signature_401_and_audited() {
    let g = webhook_app().await;
    let r = g
        .send(hook(
            g.stack.id,
            "",
            &[("x-hub-signature-256", format!("sha256={}", "00".repeat(32)))],
            PUSH_MAIN,
        ))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json, json!({"error": "unauthorized"}));
    let row = g
        .store
        .list_audit(100, None)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.action == "stack.git.webhook")
        .unwrap();
    assert_eq!(row.result, "failure");
    assert_eq!(row.detail.unwrap()["reason"], "bad signature");
    assert_eq!(g.server.fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_stack_and_non_webhook_stack_same_401() {
    let g = webhook_app().await;
    let bad = g.send(hook(g.stack.id, "?secret=nope", &[], "{}")).await;
    let unknown = g
        .send(hook(9999, &format!("?secret={SECRET}"), &[], "{}"))
        .await;
    g.store
        .set_stack_git(
            g.stack.id,
            &settings(&g.server, "manual", false),
            TokenChange::Keep,
            None,
            None,
            2,
        )
        .await
        .unwrap();
    let off = g
        .send(hook(g.stack.id, &format!("?secret={SECRET}"), &[], "{}"))
        .await;
    for r in [&unknown, &off] {
        assert_eq!(r.status, bad.status);
        assert_eq!(r.json, bad.json);
        assert_eq!(
            r.headers.get("content-type"),
            bad.headers.get("content-type")
        );
    }
    let targets: Vec<_> = g
        .store
        .list_audit(100, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "stack.git.webhook")
        .filter_map(|e| e.target_id)
        .collect();
    assert!(targets.contains(&"9999".to_string()), "{targets:?}");
    assert!(targets.contains(&"shop".to_string()), "{targets:?}");
    let reasons: Vec<_> = g
        .store
        .list_audit(100, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "stack.git.webhook")
        .map(|e| e.detail.unwrap()["reason"].as_str().unwrap().to_string())
        .collect();
    assert!(
        reasons.contains(&"no such stack".to_string()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"webhooks are off for this stack".to_string()),
        "{reasons:?}"
    );
}

#[tokio::test]
async fn other_branch_ignored() {
    let g = webhook_app().await;
    let body = r#"{"ref":"refs/heads/feature"}"#;
    let r = g.send(hook(g.stack.id, "", &github(body), body)).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    assert_eq!(r.json["status"], "ignored");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(g.server.fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn no_session_or_csrf_needed() {
    let g = webhook_app().await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    // Sent with no cookie and no CSRF header at all.
    let r = g
        .send(hook(g.stack.id, "", &github(PUSH_MAIN), PUSH_MAIN))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
}

#[tokio::test]
async fn rate_limited_after_30() {
    let g = webhook_app().await;
    for i in 0..30 {
        let r = g.send(hook(g.stack.id, "?secret=nope", &[], "{}")).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "request {i}");
    }
    let r = g.send(hook(g.stack.id, "?secret=nope", &[], "{}")).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn poll_once_syncs_due_only() {
    let g = git_app(Some(("poll", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    // A second stack, polled and synced just now, is not due.
    let env = g.stack.env_id;
    let other = g
        .store
        .create_stack(env, "other", "", &[], None, 1)
        .await
        .unwrap();
    g.store
        .set_stack_git(
            other.id,
            &settings(&g.server, "poll", false),
            TokenChange::Keep,
            Some("s"),
            None,
            1,
        )
        .await
        .unwrap();
    g.store
        .record_git_fetch(
            other.id,
            "c",
            "",
            &[],
            &Default::default(),
            fleetops_core::now_unix(),
        )
        .await
        .unwrap();
    poll_once(&g.state).await;
    assert_eq!(g.server.fetches.load(Ordering::SeqCst), 1);
    assert!(g.git().await.pending_commit.is_some());
    assert_eq!(
        g.store
            .stack_git(other.id)
            .await
            .unwrap()
            .unwrap()
            .last_fetched_commit
            .as_deref(),
        Some("c")
    );
}

#[tokio::test]
async fn poll_runs_at_most_4_at_once() {
    let g = git_app(None, ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let slow = GitServer::start(
        &g.repo.root(),
        ServerOpts {
            delay: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .await;
    let env = g.stack.env_id;
    for i in 0..6 {
        let st = g
            .store
            .create_stack(env, &format!("s{i}"), "", &[], None, 1)
            .await
            .unwrap();
        g.store
            .set_stack_git(
                st.id,
                &settings(&slow, "poll", false),
                TokenChange::Keep,
                Some("s"),
                None,
                1,
            )
            .await
            .unwrap();
    }
    poll_once(&g.state).await;
    assert_eq!(slow.fetches.load(Ordering::SeqCst), 6);
    let most = slow.in_flight_max.load(Ordering::SeqCst);
    assert!((1..=4).contains(&most), "{most} requests at once");
}

#[tokio::test]
async fn one_address_is_limited_across_stack_ids() {
    let g = webhook_app().await;
    let mut last = StatusCode::OK;
    for id in 0..121 {
        last = g
            .send(hook(10_000 + id, "?secret=nope", &[], "{}"))
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    let rows = g
        .store
        .list_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "stack.git.webhook")
        .count();
    assert_eq!(rows, 120);
}
