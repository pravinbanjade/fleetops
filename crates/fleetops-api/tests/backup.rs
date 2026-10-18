mod common;

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::*;
use fleetops_api::backup::BackupHooks;
use fleetops_store::{backup::SealedColumn, Store};
use http_body_util::BodyExt;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower::ServiceExt;

const PW: &str = "correct horse battery";

/// Downloads a backup; returns status and the raw body.
async fn download(app: &TestApp, s: &Session, passphrase: &str) -> (StatusCode, Vec<u8>) {
    let res = app
        .router
        .clone()
        .oneshot(req(
            "POST",
            "/api/v1/backup",
            Some(json!({ "passphrase": passphrase })),
            Some(s),
        ))
        .await
        .unwrap();
    let status = res.status();
    if status == StatusCode::OK {
        assert_eq!(res.headers()["content-type"], "application/octet-stream");
        let cd = res.headers()["content-disposition"].to_str().unwrap();
        assert!(
            cd.contains("fleetops-backup-") && cd.ends_with(".fob\""),
            "{cd}"
        );
    }
    (
        status,
        res.into_body().collect().await.unwrap().to_bytes().to_vec(),
    )
}

fn upload(uri: &str, file: &[u8], passphrase: &str, s: Option<&Session>) -> Request<Body> {
    let boundary = "fleetopsboundary7";
    let mut body = Vec::new();
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.fob\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend(file);
    body.extend(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"passphrase\"\r\n\r\n{passphrase}\r\n--{boundary}--\r\n").as_bytes());
    let mut b = Request::builder().method("POST").uri(uri).header(
        "content-type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    if let Some(s) = s {
        b = b
            .header("cookie", &s.cookie)
            .header("x-csrf-token", &s.csrf);
    }
    let mut r = b.body(Body::from(body)).unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    r
}

/// A source server with an admin, an environment and a user; its backup.
async fn source_backup() -> (BackupApp, Session, Vec<u8>) {
    let b = app_with_data(None, Default::default()).await;
    let admin = b.app.with_local().await;
    let r = b
        .app
        .create_user(&admin, "bob", "bob-password-1", "user")
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let (status, file) = download(&b.app, &admin, PW).await;
    assert_eq!(status, StatusCode::OK);
    (b, admin, file)
}

#[tokio::test]
async fn download_is_admin_only() {
    let b = app_with_data(None, Default::default()).await;
    let admin = b.app.setup_admin().await;
    b.app
        .create_user(&admin, "bob", "bob-password-1", "user")
        .await;
    let bob = session_from(&b.app.login("bob", "bob-password-1").await);
    assert_eq!(download(&b.app, &bob, PW).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn download_needs_passphrase_length() {
    let b = app_with_data(None, Default::default()).await;
    let admin = b.app.setup_admin().await;
    let (status, body) = download(&b.app, &admin, "short").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("at least 12 characters"));
    assert!(b
        .app
        .audit_actions()
        .await
        .iter()
        .all(|a| a != "backup.download"));
}

#[tokio::test]
async fn download_then_stage_shows_summary() {
    let (b, admin, file) = source_backup().await;
    assert!(b
        .app
        .audit_actions()
        .await
        .iter()
        .any(|a| a == "backup.download"));
    let r = b
        .app
        .send(upload("/api/v1/backup/stage", &file, PW, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["summary"]["counts"]["users"], 2);
    assert_eq!(r.json["summary"]["counts"]["environments"], 1);
    assert_eq!(r.json["summary"]["edition"], "community");
    assert!(r.json["summary"].get("secret_key").is_none());
    assert_eq!(r.json["id"].as_str().unwrap().len(), 32);
}

#[tokio::test]
async fn stage_with_wrong_passphrase() {
    let (b, admin, file) = source_backup().await;
    let r = b
        .app
        .send(upload(
            "/api/v1/backup/stage",
            &file,
            "another passphrase",
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        r.json["message"],
        "invalid input: wrong passphrase or damaged file"
    );
}

async fn stage(b: &BackupApp, admin: &Session, file: &[u8]) -> String {
    let r = b
        .app
        .send(upload("/api/v1/backup/stage", file, PW, Some(admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    r.json["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn restore_needs_confirm_word() {
    let (b, admin, file) = source_backup().await;
    let id = stage(&b, &admin, &file).await;
    let r = b
        .app
        .send(req(
            "POST",
            &format!("/api/v1/backup/stage/{id}/restore"),
            Some(json!({"confirm": "restore"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("type RESTORE to confirm"));
}

async fn wait_restart(b: &BackupApp) {
    for _ in 0..40 {
        if b.restarts.0.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no restart");
}

#[tokio::test]
async fn restore_marks_pending_and_restarts() {
    let (src, admin, file) = source_backup().await;
    // Restore over a different server, with its own admin.
    let b = app_with_data(None, Default::default()).await;
    let other = b.app.setup_admin().await;
    let id = stage(&b, &other, &file).await;
    let r = b
        .app
        .send(req(
            "POST",
            &format!("/api/v1/backup/stage/{id}/restore"),
            Some(json!({"confirm": "RESTORE"})),
            Some(&other),
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    wait_restart(&b).await;
    assert_eq!(b.restarts.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(b.data.path().join("restore-pending/fleetops.db").exists());
    assert!(!b.data.path().join("restore-staging").join(&id).exists());
    // What the next start does.
    fleetops_store::backup::apply_pending(b.data.path())
        .await
        .unwrap();
    let s = Store::open(&b.data.path().join("fleetops.db"))
        .await
        .unwrap();
    let names: Vec<_> = s
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(names, vec!["admin", "bob"]);
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    let actions: Vec<_> = s
        .list_audit(100, None)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert_eq!(actions[0], "backup.restore");
    drop((src, admin));
}

#[tokio::test]
async fn setup_restore_only_while_open() {
    let (_src, _admin, file) = source_backup().await;
    // A user exists: setup is over.
    let b = app_with_data(None, Default::default()).await;
    b.app.setup_admin().await;
    let r = b
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // The window closed.
    let c = app_with_data(
        None,
        fleetops_api::ApiConfig {
            setup_window: Duration::ZERO,
            ..Default::default()
        },
    )
    .await;
    let r = c
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Open: stage and restore with no login.
    let d = app_with_data(None, Default::default()).await;
    let r = d
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let id = r.json["id"].as_str().unwrap();
    let r = d
        .app
        .send(req(
            "POST",
            &format!("/api/v1/auth/setup/backup/{id}/restore"),
            None,
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json);
    wait_restart(&d).await;
}

#[tokio::test]
async fn setup_stage_id_not_usable_by_admin_route() {
    let (_src, _admin, file) = source_backup().await;
    let d = app_with_data(None, Default::default()).await;
    let r = d
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    let setup_id = r.json["id"].as_str().unwrap().to_string();
    let admin = d.app.setup_admin().await;
    let r = d
        .app
        .send(req(
            "POST",
            &format!("/api/v1/backup/stage/{setup_id}/restore"),
            Some(json!({"confirm": "RESTORE"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // And an admin-staged id is not usable by the first-run route.
    let e = app_with_data(None, Default::default()).await;
    let admin = e.app.setup_admin().await;
    let id = stage(&e, &admin, &file).await;
    let r = e
        .app
        .send(req(
            "POST",
            &format!("/api/v1/auth/setup/backup/{id}/restore"),
            None,
            None,
        ))
        .await;
    assert_eq!(
        r.status,
        StatusCode::CONFLICT,
        "setup is over once a user exists"
    );
}

#[tokio::test]
async fn second_restore_conflicts() {
    let (_src, _admin, file) = source_backup().await;
    let b = app_with_data(None, Default::default()).await;
    let admin = b.app.setup_admin().await;
    let one = stage(&b, &admin, &file).await;
    let two = stage(&b, &admin, &file).await;
    for (id, want) in [(one, StatusCode::ACCEPTED), (two, StatusCode::CONFLICT)] {
        let r = b
            .app
            .send(req(
                "POST",
                &format!("/api/v1/backup/stage/{id}/restore"),
                Some(json!({"confirm": "RESTORE"})),
                Some(&admin),
            ))
            .await;
        assert_eq!(r.status, want, "{:?}", r.json);
    }
}

/// Stands in for Enterprise: one sealed table.
struct FakeEe;

#[async_trait]
impl BackupHooks for FakeEe {
    fn sealed(&self) -> Vec<SealedColumn> {
        vec![SealedColumn::new("ee_x", "secret", "id", "x:{id}")]
    }
    fn enterprise_migration_max(&self) -> Option<i64> {
        None
    }
    async fn migrate(&self, _: &sqlx::SqlitePool) -> fleetops_core::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn ee_sealed_columns_from_hooks_are_resealed() {
    let src = app_with_data(Some(Arc::new(FakeEe)), Default::default()).await;
    let admin = src.app.setup_admin().await;
    sqlx::query("CREATE TABLE ee_x (id INTEGER PRIMARY KEY, secret TEXT)")
        .execute(src.app.store.pool())
        .await
        .unwrap();
    let sealed = src.app.store.secrets().seal(b"x:5", b"hunter2");
    sqlx::query("INSERT INTO ee_x VALUES (5, ?)")
        .bind(sealed)
        .execute(src.app.store.pool())
        .await
        .unwrap();
    let (_, file) = download(&src.app, &admin, PW).await;
    // A Community target, with no hooks, still re-seals it from the manifest.
    let b = app_with_data(None, Default::default()).await;
    let other = b.app.setup_admin().await;
    let id = stage(&b, &other, &file).await;
    let r = b
        .app
        .send(req(
            "POST",
            &format!("/api/v1/backup/stage/{id}/restore"),
            Some(json!({"confirm": "RESTORE"})),
            Some(&other),
        ))
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    let db = b.data.path().join("restore-pending/fleetops.db");
    let s = Store::open(&db).await.unwrap();
    let v: String = sqlx::query_scalar("SELECT secret FROM ee_x WHERE id = 5")
        .fetch_one(s.pool())
        .await
        .unwrap();
    assert_eq!(b.app.store.secrets().open(b"x:5", &v).unwrap(), b"hunter2");
}

// ---- final-review fixes ----

/// A multipart upload whose body is fed by hand, to cut it off or hold it open.
fn streamed_upload(
    uri: &str,
) -> (
    Request<Body>,
    tokio::sync::mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) });
    let mut r = Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            "content-type",
            "multipart/form-data; boundary=fleetopsboundary7",
        )
        .body(Body::from_stream(stream))
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    (r, tx)
}

const PART_HEAD: &str = "--fleetopsboundary7\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.fob\"\r\nContent-Type: application/octet-stream\r\n\r\n";

#[tokio::test]
async fn cut_off_upload_leaves_nothing() {
    let b = app_with_data(None, Default::default()).await;
    let (r, tx) = streamed_upload("/api/v1/auth/setup/backup");
    let router = b.app.router.clone();
    let res = tokio::spawn(async move { router.oneshot(r).await.unwrap() });
    tx.send(Ok(bytes::Bytes::from(PART_HEAD))).await.unwrap();
    tx.send(Ok(bytes::Bytes::from(vec![7u8; 100_000])))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The connection closes: the handler's future is dropped mid-upload.
    res.abort();
    let _ = res.await;
    drop(tx);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tmp = b.data.path().join("backup-tmp");
    let left = std::fs::read_dir(&tmp).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0, "upload left behind in {tmp:?}");
}

#[tokio::test]
async fn setup_stages_one_upload_at_a_time() {
    let (_src, _admin, file) = source_backup().await;
    let b = app_with_data(None, Default::default()).await;
    let (r, tx) = streamed_upload("/api/v1/auth/setup/backup");
    let router = b.app.router.clone();
    let held = tokio::spawn(async move { router.oneshot(r).await.unwrap() });
    tx.send(Ok(bytes::Bytes::from(PART_HEAD))).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = b
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.json);
    drop(tx);
    let _ = held.await;
}

#[tokio::test]
async fn setup_upload_replaces_the_previous_one() {
    let (_src, _admin, file) = source_backup().await;
    let b = app_with_data(None, Default::default()).await;
    let first = b
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    let first = first.json["id"].as_str().unwrap().to_string();
    let second = b
        .app
        .send(upload("/api/v1/auth/setup/backup", &file, PW, None))
        .await;
    assert_eq!(second.status, StatusCode::OK);
    let r = b
        .app
        .send(req(
            "POST",
            &format!("/api/v1/auth/setup/backup/{first}/restore"),
            None,
            None,
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let staged = std::fs::read_dir(b.data.path().join("restore-staging"))
        .unwrap()
        .count();
    assert_eq!(staged, 1, "the replaced upload's files are removed");
}
