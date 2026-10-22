mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use fleetops_store::{GitSettings, TokenChange};
use serde_json::{json, Value};

const BASE: &str = "/api/v1/envs/local/stacks";
const COMPOSE: &str = r#"
services:
  db:
    image: postgres
    secrets: [db_password]
    configs: [app, var_conf, inline]
  web:
    image: nginx
    secrets: [db_password]
secrets:
  db_password: { file: ./pw.txt }
configs:
  app: { file: ./app.conf }
  var_conf: { environment: CONF }
  inline: { content: "x=1" }
"#;

async fn setup(app: &TestApp) -> Session {
    let s = app.with_local().await;
    let r = app
        .send(req(
            "POST",
            BASE,
            Some(json!({"name": "shop", "compose": COMPOSE, "variables": ["CONF=c"]})),
            Some(&s),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    s
}

async fn list(app: &TestApp, s: &Session) -> Vec<Value> {
    let r = app
        .send(req("GET", &format!("{BASE}/shop/files"), None, Some(s)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    r.json.as_array().unwrap().clone()
}

fn item<'a>(items: &'a [Value], kind: &str, name: &str) -> &'a Value {
    items
        .iter()
        .find(|i| i["kind"] == kind && i["name"] == name)
        .unwrap_or_else(|| panic!("{kind} {name} missing: {items:?}"))
}

async fn put(app: &TestApp, s: &Session, kind: &str, name: &str, value: &str) -> Resp {
    app.send(req(
        "PUT",
        &format!("{BASE}/shop/files/{kind}/{name}"),
        Some(json!({ "value": value })),
        Some(s),
    ))
    .await
}

async fn stack_id(app: &TestApp) -> i64 {
    let env = app.store.list_environments().await.unwrap()[0].id;
    app.store.stack_by_name(env, "shop").await.unwrap().id
}

#[tokio::test]
async fn lists_declared_and_unused_items_with_sources() {
    let app = app().await;
    let s = setup(&app).await;
    let id = stack_id(&app).await;
    app.store
        .set_stack_file(id, "secret", "old", "x", None, 1)
        .await
        .unwrap();
    let items = list(&app, &s).await;
    assert_eq!(items.len(), 5, "{items:?}");
    let pw = item(&items, "secret", "db_password");
    assert_eq!(pw["source"], "fleetops");
    assert_eq!(pw["path"], "./pw.txt");
    assert_eq!(pw["used_by"], json!(["db", "web"]));
    assert_eq!(
        (pw["set"].clone(), pw["used"].clone()),
        (json!(false), json!(true))
    );
    let v = item(&items, "config", "var_conf");
    assert_eq!(
        (v["source"].clone(), v["variable"].clone()),
        (json!("variable"), json!("CONF"))
    );
    assert_eq!(item(&items, "config", "inline")["source"], "inline");
    let old = item(&items, "secret", "old");
    assert_eq!(
        (old["used"].clone(), old["set"].clone()),
        (json!(false), json!(true))
    );
    assert!(old["content"].is_null());
}

#[tokio::test]
async fn set_secret_is_write_only() {
    let app = app().await;
    let s = setup(&app).await;
    let r = put(&app, &s, "secret", "db_password", "hunter2").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(r.json.is_null(), "{:?}", r.json);
    let items = list(&app, &s).await;
    let pw = item(&items, "secret", "db_password");
    assert_eq!(pw["set"], true);
    assert!(pw["content"].is_null());
    assert_eq!(pw["updated_by"], "admin");
    assert!(!json!(items).to_string().contains("hunter2"));
    let d = app
        .send(req(
            "DELETE",
            &format!("{BASE}/shop/files/secret/db_password"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(d.status, StatusCode::NO_CONTENT);
    let d = app
        .send(req(
            "DELETE",
            &format!("{BASE}/shop/files/secret/db_password"),
            None,
            Some(&s),
        ))
        .await;
    assert_eq!(d.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn config_content_shown_only_for_fleetops_and_repo_sources() {
    let app = app().await;
    let s = setup(&app).await;
    let r = put(&app, &s, "config", "app", "listen 80;").await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    let items = list(&app, &s).await;
    assert_eq!(item(&items, "config", "app")["content"], "listen 80;");
    assert!(item(&items, "config", "var_conf")["content"].is_null());
    assert!(item(&items, "config", "inline")["content"].is_null());
}

#[tokio::test]
async fn git_stack_refuses_config_values() {
    let app = app().await;
    let s = setup(&app).await;
    let id = stack_id(&app).await;
    let settings = GitSettings {
        url: "https://git.example/shop.git".into(),
        branch: "main".into(),
        path: "deploy/compose.yaml".into(),
        sync: "manual".into(),
        poll_seconds: 300,
        auto_deploy: false,
    };
    app.store
        .set_stack_git(id, &settings, TokenChange::Keep, Some("sec"), None, 1)
        .await
        .unwrap();
    app.store
        .record_git_fetch(id, "c1", COMPOSE, &[("app".into(), "from repo".into())], 2)
        .await
        .unwrap();
    let r = put(&app, &s, "config", "app", "x").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.json["message"]
            .to_string()
            .contains("configs of a git stack come from the repository"),
        "{:?}",
        r.json
    );
    let items = list(&app, &s).await;
    let app_conf = item(&items, "config", "app");
    assert_eq!(app_conf["source"], "repo");
    assert_eq!(app_conf["path"], "deploy/app.conf");
    assert_eq!(app_conf["content"], "from repo");
    // Secrets still come from the Secrets tab.
    assert_eq!(
        put(&app, &s, "secret", "db_password", "p").await.status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn non_utf8_and_oversized_values_refused() {
    let app = app().await;
    let s = setup(&app).await;
    let mut body = b"{\"value\": \"".to_vec();
    body.extend_from_slice(&[0xff, 0xfe]);
    body.extend_from_slice(b"\"}");
    let mut r = Request::builder()
        .method("PUT")
        .uri(format!("{BASE}/shop/files/secret/db_password"))
        .header("cookie", &s.cookie)
        .header("x-csrf-token", &s.csrf)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    r.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    let r = app.send(r).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.json.to_string().contains("must be UTF-8 text"),
        "{:?}",
        r.json
    );
    let big = "x".repeat(500 * 1024 + 1);
    let r = put(&app, &s, "secret", "db_password", &big).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json.to_string().contains("500 KiB"), "{:?}", r.json);
}

#[tokio::test]
async fn bad_name_refused() {
    let app = app().await;
    let s = setup(&app).await;
    for (kind, name) in [
        ("secret", "a%20b"),
        ("secret", &"x".repeat(65)),
        ("token", "pw"),
    ] {
        let r = put(&app, &s, kind, name, "v").await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "{kind} {name}: {:?}",
            r.json
        );
    }
}

#[tokio::test]
async fn audit_records_name_not_value() {
    let app = app().await;
    let s = setup(&app).await;
    put(&app, &s, "secret", "db_password", "hunter2").await;
    put(&app, &s, "config", "app", "conf-text").await;
    app.send(req(
        "DELETE",
        &format!("{BASE}/shop/files/secret/db_password"),
        None,
        Some(&s),
    ))
    .await;
    let audit = app.store.list_audit(1000, None).await.unwrap();
    let find = |action: &str| {
        audit
            .iter()
            .find(|e| e.action == action)
            .unwrap_or_else(|| panic!("no {action}"))
    };
    let set = find("stack.secret.set");
    assert_eq!(set.target_type.as_deref(), Some("stack"));
    assert_eq!(set.target_id.as_deref(), Some("shop"));
    assert_eq!(set.detail.as_ref().unwrap()["name"], "db_password");
    find("stack.config.set");
    find("stack.secret.remove");
    let text = format!("{audit:?}");
    assert!(!text.contains("hunter2") && !text.contains("conf-text"));
}
