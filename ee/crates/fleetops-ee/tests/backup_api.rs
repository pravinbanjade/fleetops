mod common;

use common::*;
use fleetops_api::ApiConfig;
use fleetops_core::now_unix;
use fleetops_ee::backup::{db, db::Trigger, run::run_once};
use fleetops_store::backup::backup_name;
use serde_json::{json, Value};
use std::time::Duration;

const DAY: i64 = 86_400;
const BPW: &str = "correct horse battery";

struct Fx {
    ee: Ee,
    admin: Session,
    data: tempfile::TempDir,
    /// A directory outside the data directory, for the local target.
    out: tempfile::TempDir,
}

async fn fx(licensed: bool) -> Fx {
    let data = tempfile::tempdir().unwrap();
    let k = key();
    let ee = ee_app_with(
        &k,
        None,
        ApiConfig {
            data_dir: Some(data.path().to_path_buf()),
            ..Default::default()
        },
    )
    .await;
    let admin = ee.admin().await;
    if licensed {
        let r = ee
            .install(&admin, &license(&k, now_unix() + DAY, None, &[]))
            .await;
        assert!(r.status.is_success(), "{}", r.text);
    }
    Fx {
        ee,
        admin,
        data,
        out: tempfile::tempdir().unwrap(),
    }
}

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

impl Fx {
    async fn call(&self, method: &str, uri: &str, v: Option<Value>) -> Resp {
        self.ee
            .send(method, uri, v.and_then(body), Some(&self.admin))
            .await
    }

    async fn configure(&self) {
        let r = self
            .call(
                "PUT",
                "/api/v1/ee/backup/schedule/passphrase",
                Some(json!({"passphrase": BPW})),
            )
            .await;
        assert_eq!(r.status, 204, "{}", r.text);
        let r = self
            .call(
                "PUT",
                "/api/v1/ee/backup/targets/local",
                Some(json!({"settings": {"kind": "local", "path": self.out.path()}})),
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.text);
    }

    async fn runs(&self) -> Vec<Value> {
        self.call("GET", "/api/v1/ee/backup/runs", None)
            .await
            .json()
            .as_array()
            .unwrap()
            .clone()
    }

    async fn wait_runs(&self, n: usize) -> Vec<Value> {
        for _ in 0..100 {
            let runs = self.runs().await;
            if runs.len() >= n {
                return runs;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no run recorded");
    }
}

#[tokio::test]
async fn unlicensed_run_is_skipped_and_audited() {
    let f = fx(false).await;
    db::put_schedule(
        f.ee.store.pool(),
        &db::Schedule {
            enabled: true,
            ..Default::default()
        },
        0,
    )
    .await
    .unwrap();
    run_once(&f.ee.state, Trigger::Schedule, None).await;
    let runs = f.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["result"], "skipped");
    let audit = f.ee.store.list_audit(10, None).await.unwrap();
    let a = audit
        .iter()
        .find(|a| a.action == "backup.scheduled")
        .unwrap();
    assert_eq!(a.result, "skipped");
    assert_eq!(a.detail.as_ref().unwrap()["reason"], "license_required");
    let st = f.call("GET", "/api/v1/ee/backup/status", None).await.json();
    assert_eq!(st["paused"], true);
    assert_eq!(st["licensed"], false);
}

#[tokio::test]
async fn unlicensed_can_disable_and_delete_target() {
    let f = fx(false).await;
    let pool = f.ee.store.pool();
    db::put_schedule(
        pool,
        &db::Schedule {
            enabled: true,
            ..Default::default()
        },
        0,
    )
    .await
    .unwrap();
    db::upsert_target(
        &f.ee.store,
        &db::TargetSettings::Local {
            path: f.out.path().to_path_buf(),
        },
        None,
        0,
    )
    .await
    .unwrap();
    let r = f
        .call(
            "PUT",
            "/api/v1/ee/backup/schedule",
            Some(json!({"enabled": false, "hour_utc": 2, "retention": {"kind": "count", "value": 7}})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let r = f
        .call("DELETE", "/api/v1/ee/backup/targets/local", None)
        .await;
    assert_eq!(r.status, 204, "{}", r.text);
}

#[tokio::test]
async fn unlicensed_configure_refused() {
    let f = fx(false).await;
    for (m, uri, b) in [
        (
            "PUT",
            "/api/v1/ee/backup/schedule/passphrase",
            Some(json!({"passphrase": BPW})),
        ),
        (
            "PUT",
            "/api/v1/ee/backup/schedule",
            Some(
                json!({"enabled": false, "hour_utc": 5, "retention": {"kind": "count", "value": 7}}),
            ),
        ),
        ("POST", "/api/v1/ee/backup/run", None),
        ("GET", "/api/v1/ee/backup/recent", None),
    ] {
        let r = f.call(m, uri, b).await;
        assert_eq!(r.status, 403, "{uri}: {}", r.text);
        assert_eq!(r.json()["code"], "license_required", "{uri}");
    }
}

#[tokio::test]
async fn enable_needs_passphrase_and_target() {
    let f = fx(true).await;
    let enable =
        json!({"enabled": true, "hour_utc": 2, "retention": {"kind": "count", "value": 7}});
    let r = f
        .call("PUT", "/api/v1/ee/backup/schedule", Some(enable.clone()))
        .await;
    assert!(
        r.json()["message"].as_str().unwrap().contains("passphrase"),
        "{}",
        r.text
    );
    f.call(
        "PUT",
        "/api/v1/ee/backup/schedule/passphrase",
        Some(json!({"passphrase": BPW})),
    )
    .await;
    let r = f
        .call("PUT", "/api/v1/ee/backup/schedule", Some(enable.clone()))
        .await;
    assert!(
        r.json()["message"].as_str().unwrap().contains("target"),
        "{}",
        r.text
    );
    f.configure().await;
    let r = f
        .call("PUT", "/api/v1/ee/backup/schedule", Some(enable))
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.json()["enabled"], true);
}

#[tokio::test]
async fn run_now_local_target_then_recent_lists_it() {
    let f = fx(true).await;
    f.configure().await;
    let r = f.call("POST", "/api/v1/ee/backup/run", None).await;
    assert_eq!(r.status, 202, "{}", r.text);
    let runs = f.wait_runs(1).await;
    assert_eq!(runs[0]["result"], "success", "{runs:?}");
    assert_eq!(runs[0]["target_kind"], "local");
    let recent = f.call("GET", "/api/v1/ee/backup/recent", None).await.json();
    assert_eq!(recent["backups"].as_array().unwrap().len(), 1);
    assert_eq!(recent["backups"][0]["target_kind"], "local");
    assert!(recent["backups"][0]["name"]
        .as_str()
        .unwrap()
        .starts_with("fleetops-backup-"));
    // Nothing is left behind in the data directory's scratch space.
    let tmp = f.data.path().join("backup-tmp");
    assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
}

async fn one_backup(f: &Fx) -> String {
    f.configure().await;
    run_once(&f.ee.state, Trigger::Manual, None).await;
    let recent = f.call("GET", "/api/v1/ee/backup/recent", None).await.json();
    recent["backups"][0]["name"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn restore_from_recent_with_stored_passphrase() {
    let f = fx(true).await;
    let name = one_backup(&f).await;
    let r = f
        .call(
            "POST",
            "/api/v1/ee/backup/recent/restore",
            Some(json!({"target_kind": "local", "name": name})),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(r.json()["summary"]["edition"], "enterprise");
    let r = f
        .call(
            "POST",
            &format!("/api/v1/backup/stage/{id}/restore"),
            Some(json!({"confirm": "RESTORE"})),
        )
        .await;
    assert_eq!(r.status, 202, "{}", r.text);
    assert!(f.data.path().join("restore-pending/fleetops.db").exists());
}

#[tokio::test]
async fn restore_from_recent_after_passphrase_change_asks() {
    let f = fx(true).await;
    let name = one_backup(&f).await;
    f.call(
        "PUT",
        "/api/v1/ee/backup/schedule/passphrase",
        Some(json!({"passphrase": "a different passphrase"})),
    )
    .await;
    let req = |p: Option<&str>| json!({"target_kind": "local", "name": name, "passphrase": p});
    let r = f
        .call("POST", "/api/v1/ee/backup/recent/restore", Some(req(None)))
        .await;
    assert_eq!(r.status, 400);
    assert!(
        r.json()["message"]
            .as_str()
            .unwrap()
            .contains("this backup was made with a different passphrase; enter it to restore"),
        "{}",
        r.text
    );
    let r = f
        .call(
            "POST",
            "/api/v1/ee/backup/recent/restore",
            Some(req(Some(BPW))),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
}

#[tokio::test]
async fn retention_applied_after_run() {
    let f = fx(true).await;
    f.configure().await;
    db::put_schedule(
        f.ee.store.pool(),
        &db::Schedule {
            retention: db::Retention::Count(2),
            ..Default::default()
        },
        0,
    )
    .await
    .unwrap();
    let now = now_unix();
    for d in 1..=3 {
        std::fs::write(f.out.path().join(backup_name(now - d * DAY)), "old").unwrap();
    }
    std::fs::write(f.out.path().join("keep-me.txt"), "x").unwrap();
    run_once(&f.ee.state, Trigger::Manual, None).await;
    let mut names: Vec<_> = std::fs::read_dir(f.out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            backup_name(now - DAY),
            backup_name(now),
            "keep-me.txt".to_string()
        ]
    );
}

#[tokio::test]
async fn secrets_never_returned() {
    let f = fx(true).await;
    let r = f
        .call(
            "PUT",
            "/api/v1/ee/backup/targets/s3",
            Some(json!({
                "settings": {"kind": "s3", "region": "us-east-1", "bucket": "b", "access_key_id": "AKIA1"},
                "secret_access_key": "super-secret-value"
            })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert!(!r.text.contains("super-secret-value"));
    let r = f.call("GET", "/api/v1/ee/backup/targets", None).await;
    assert!(!r.text.contains("super-secret-value"));
    assert_eq!(r.json()[0]["secret_set"], true);
    let audit = serde_json::to_string(&f.ee.store.list_audit(50, None).await.unwrap()).unwrap();
    assert!(!audit.contains("super-secret-value"));
}

#[tokio::test]
async fn local_target_in_data_dir_refused() {
    let f = fx(true).await;
    let inside = f.data.path().join("backups");
    std::fs::create_dir_all(&inside).unwrap();
    let r = f
        .call(
            "PUT",
            "/api/v1/ee/backup/targets/local",
            Some(json!({"settings": {"kind": "local", "path": inside}})),
        )
        .await;
    assert_eq!(r.status, 400);
    assert!(
        r.text.contains("can't be inside the data directory"),
        "{}",
        r.text
    );
}
