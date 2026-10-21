mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::git::*;
use common::*;
use fleetops_api::stacks::git::SyncTrigger;
use fleetops_git::testing::ServerOpts;
use serde_json::{json, Value};

const TOKEN: &str = "s3cret-token";
const GIT: &str = "/api/v1/envs/local/stacks/shop/git";

fn body(g: &GitApp, extra: Value) -> Value {
    let mut b = json!({"url": g.server.url("repo.git"), "branch": "main", "sync": "manual"});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

async fn put(g: &GitApp, b: Value) -> Resp {
    g.send(req("PUT", GIT, Some(b), Some(&g.admin))).await
}

#[tokio::test]
async fn attach_returns_secret_once() {
    let g = git_app(None, ServerOpts::default()).await;
    let first = put(&g, body(&g, json!({}))).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json);
    let secret = first.json["webhook_secret"].as_str().unwrap().to_string();
    assert_eq!(secret.len(), 64);
    assert_eq!(first.json["status"]["git"]["path"], "compose.yaml");
    assert_eq!(first.json["status"]["git"]["poll_seconds"], 300);
    assert_eq!(
        first.json["status"]["webhook_url"],
        format!("/api/v1/stacks/hooks/{}", g.stack.id)
    );
    let second = put(&g, body(&g, json!({"auto_deploy": true}))).await;
    assert!(second.json["webhook_secret"].is_null());
    let get = g.send(req("GET", GIT, None, Some(&g.admin))).await;
    assert_eq!(get.status, StatusCode::OK);
    assert!(!get.json.to_string().contains(&secret));
    assert_eq!(get.json["git"]["auto_deploy"], true);
    let l = g.send(req("GET", STACKS, None, Some(&g.admin))).await;
    assert_eq!(l.json[0]["source"], "git");
}

#[tokio::test]
async fn rotate_secret_returns_new_one() {
    let g = git_app(None, ServerOpts::default()).await;
    let first = put(&g, body(&g, json!({}))).await;
    let old = first.json["webhook_secret"].as_str().unwrap().to_string();
    let r = g
        .send(req("POST", &format!("{GIT}/secret"), None, Some(&g.admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let new = r.json["webhook_secret"].as_str().unwrap();
    assert_ne!(new, old);
    assert_eq!(
        g.store
            .stack_git_secret(g.stack.id)
            .await
            .unwrap()
            .as_deref(),
        Some(new)
    );
}

#[tokio::test]
async fn validation_messages() {
    let g = git_app(None, ServerOpts::default()).await;
    let cases = [
        (
            json!({"url": "ssh://git.example/r.git"}),
            "only https git URLs are supported",
        ),
        (
            json!({"url": "https://u:p@git.example/r.git"}),
            "put the access token in the token field, not in the URL",
        ),
        (json!({"path": "../x.yaml"}), "is not a valid path"),
        (
            json!({"sync": "poll", "poll_seconds": 30}),
            "at least 60 seconds",
        ),
        (
            json!({"sync": "always"}),
            "sync must be manual, poll or webhook",
        ),
    ];
    for (extra, msg) in cases {
        let r = put(&g, body(&g, extra.clone())).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{extra}: {:?}", r.json);
        assert!(
            r.json["message"].as_str().unwrap().contains(msg),
            "{extra}: {:?}",
            r.json
        );
    }
    assert!(g.store.stack_git(g.stack.id).await.unwrap().is_none());
}

#[tokio::test]
async fn no_data_dir_refused() {
    let app = app_with(git_config()).await;
    let s = app.with_local().await;
    app.send(req(
        "POST",
        STACKS,
        Some(json!({"name": "shop", "compose": "", "variables": []})),
        Some(&s),
    ))
    .await;
    let r = app
        .send(req(
            "PUT",
            GIT,
            Some(json!({"url": "http://127.0.0.1:1/r.git", "branch": "main", "sync": "manual"})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.json);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("need a data directory"));
}

#[tokio::test]
async fn token_never_returned() {
    let g = git_app(
        None,
        ServerOpts {
            token: Some(TOKEN.into()),
            ..Default::default()
        },
    )
    .await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let r = put(&g, body(&g, json!({"token": TOKEN}))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["status"]["git"]["has_token"], true);
    g.sync(SyncTrigger::Manual(1)).await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{TOKEN}"));
    let mut texts = vec![r.json.to_string()];
    for path in [
        GIT.to_string(),
        STACKS.to_string(),
        format!("{STACKS}/shop"),
        "/api/v1/audit".into(),
    ] {
        let resp = g.send(req("GET", &path, None, Some(&g.admin))).await;
        assert_eq!(resp.status, StatusCode::OK, "{path}");
        texts.push(resp.json.to_string());
    }
    for t in texts {
        assert!(!t.contains(TOKEN), "{t}");
        assert!(!t.contains(&b64), "{t}");
    }
}

#[tokio::test]
async fn sync_now_returns_outcome() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let r = g
        .send(req("POST", &format!("{GIT}/sync"), None, Some(&g.admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json, json!({"outcome": "pending", "commit": head}));
}

#[tokio::test]
async fn resume_clears_paused() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.store.set_git_paused(g.stack.id, true).await.unwrap();
    let r = g
        .send(req("POST", &format!("{GIT}/resume"), None, Some(&g.admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["outcome"], "pending");
    assert!(!g.git().await.paused);
}

#[tokio::test]
async fn git_draft_compose_locked_variables_editable() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.sync(SyncTrigger::Manual(1)).await;
    let draft = format!("{STACKS}/shop/draft");
    let ok = g
        .send(req(
            "PUT",
            &draft,
            Some(json!({"compose": WEB_A, "variables": ["A=1"]})),
            Some(&g.admin),
        ))
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT, "{:?}", ok.json);
    let d = g.store.draft(g.stack.id).await.unwrap().unwrap();
    assert_eq!(d.variables, vec!["A=1".to_string()]);
    let bad = g
        .send(req(
            "PUT",
            &draft,
            Some(json!({"compose": WEB_B, "variables": []})),
            Some(&g.admin),
        ))
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json["message"]
        .as_str()
        .unwrap()
        .contains("a git stack's Compose file comes from its repository; change it there"));
}

#[tokio::test]
async fn list_shows_update_available_and_sync_error() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.sync(SyncTrigger::Manual(1)).await;
    let l = g.send(req("GET", STACKS, None, Some(&g.admin))).await;
    assert_eq!(l.json[0]["update_available"], true);
    assert_eq!(l.json[0]["paused"], false);
    assert!(l.json[0]["sync_error"].is_null());
    g.store
        .record_git_error(g.stack.id, "boom", 9)
        .await
        .unwrap();
    let l = g.send(req("GET", STACKS, None, Some(&g.admin))).await;
    assert_eq!(l.json[0]["sync_error"], "boom");
}

#[tokio::test]
async fn detach_returns_to_inline() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.sync(SyncTrigger::Manual(1)).await;
    let dir = g
        .backup
        .data
        .path()
        .join("git")
        .join(g.stack.id.to_string());
    assert!(dir.exists());
    let r = g.send(req("DELETE", GIT, None, Some(&g.admin))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(!dir.exists());
    let get = g.send(req("GET", GIT, None, Some(&g.admin))).await;
    assert_eq!(get.status, StatusCode::NOT_FOUND);
    let st = g.store.stack_by_id(g.stack.id).await.unwrap();
    assert_eq!(st.source, "inline");
    assert_eq!(g.store.draft(st.id).await.unwrap().unwrap().compose, WEB_A);
}

#[tokio::test]
async fn remove_stack_deletes_cache_dir() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.sync(SyncTrigger::Manual(1)).await;
    let dir = g
        .backup
        .data
        .path()
        .join("git")
        .join(g.stack.id.to_string());
    assert!(dir.exists());
    let r = g
        .send(req(
            "DELETE",
            &format!("{STACKS}/shop"),
            None,
            Some(&g.admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(!dir.exists());
}
