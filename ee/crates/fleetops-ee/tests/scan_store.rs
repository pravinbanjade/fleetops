use fleetops_ee::scan::{
    config::{EnvOverride, GlobalConfig, Mirror},
    db::{self, PasswordChange, Status},
    model::{Finding, ImageKey, ImageUse, ScanTarget, Severity, Trigger},
};
use fleetops_store::Store;

async fn store() -> Store {
    let store = Store::in_memory().await.unwrap();
    fleetops_ee::db::migrate(store.pool()).await.unwrap();
    store
}

async fn env(store: &Store, name: &str) -> i64 {
    store
        .create_environment(name, "docker-local", "unix:///var/run/docker.sock", 0)
        .await
        .unwrap()
        .id
}

fn mirror(user: &str) -> Mirror {
    Mirror {
        trivy_image: Some("mirror.local/trivy:1".into()),
        db_repository: Some("mirror.local/trivy-db".into()),
        java_db_repository: None,
        username: Some(user.into()),
    }
}

fn target(env_id: i64, key: &str) -> ScanTarget {
    ScanTarget {
        env_id,
        node: String::new(),
        key: ImageKey(key.into()),
        image_ref: "nginx:1.25".into(),
        namespace: String::new(),
        pull_secrets: vec![],
        platform: None,
    }
}

fn finding(id: &str, sev: Severity) -> Finding {
    Finding {
        vuln_id: id.into(),
        pkg_name: "libssl3".into(),
        installed_version: "3.0.11".into(),
        fixed_version: Some("3.0.14".into()),
        severity: sev,
        title: None,
        url: None,
    }
}

fn use_of(env_id: i64, key: &str, container: &str) -> ImageUse {
    ImageUse {
        env_id,
        node: String::new(),
        key: ImageKey(key.into()),
        image_ref: "nginx:1.25".into(),
        image_id: Some("sha256:cfg".into()),
        namespace: String::new(),
        workload: String::new(),
        container: container.into(),
        running: !container.is_empty(),
        pull_secrets: vec![],
        platform: None,
    }
}

#[tokio::test]
async fn settings_round_trip_and_password_is_sealed() {
    let store = store().await;
    let (g, pw) = db::get_global(&store).await.unwrap();
    assert_eq!(g, GlobalConfig::default(), "off with no mirror by default");
    assert_eq!(pw, None);
    let g = GlobalConfig {
        enabled: true,
        mirror: Some(mirror("bot")),
    };
    db::put_global(&store, &g, PasswordChange::Set("hunter2".into()), 1)
        .await
        .unwrap();
    let raw: Option<String> =
        sqlx::query_scalar("SELECT mirror_secret FROM ee_scan_global WHERE id = 1")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert!(raw.is_some_and(|r| !r.contains("hunter2")), "sealed");
    let (back, pw) = db::get_global(&store).await.unwrap();
    assert_eq!(back, g);
    assert_eq!(pw.as_deref(), Some("hunter2"));
    db::put_global(&store, &g, PasswordChange::Keep, 2)
        .await
        .unwrap();
    assert_eq!(
        db::get_global(&store).await.unwrap().1.as_deref(),
        Some("hunter2")
    );
    db::put_global(&store, &g, PasswordChange::Clear, 3)
        .await
        .unwrap();
    assert_eq!(db::get_global(&store).await.unwrap().1, None);

    let id = env(&store, "local").await;
    assert_eq!(db::get_env(&store, id).await.unwrap(), None);
    let o = EnvOverride {
        enabled: Some(false),
        mirror: Some(mirror("env-bot")),
        socket_path: Some("/run/user/1000/docker.sock".into()),
    };
    db::put_env(&store, id, &o, PasswordChange::Set("pw".into()), 4)
        .await
        .unwrap();
    assert_eq!(
        db::get_env(&store, id).await.unwrap(),
        Some((o.clone(), Some("pw".into())))
    );
    assert_eq!(
        db::list_envs(&store).await.unwrap(),
        vec![(id, o, Some("pw".into()))]
    );
}

#[tokio::test]
async fn env_delete_cascades() {
    let store = store().await;
    let id = env(&store, "gone").await;
    db::put_env(&store, id, &EnvOverride::default(), PasswordChange::Keep, 0)
        .await
        .unwrap();
    db::replace_uses(&store, id, &[use_of(id, "sha256:a", "web")])
        .await
        .unwrap();
    store.delete_environment(id).await.unwrap();
    assert_eq!(db::get_env(&store, id).await.unwrap(), None);
    assert!(db::uses_for_envs(&store, &[id]).await.unwrap().is_empty());
}

#[tokio::test]
async fn uses_are_replaced_per_environment() {
    let store = store().await;
    let (a, b) = (env(&store, "a").await, env(&store, "b").await);
    db::replace_uses(
        &store,
        a,
        &[use_of(a, "sha256:1", "web"), use_of(a, "sha256:2", "")],
    )
    .await
    .unwrap();
    db::replace_uses(&store, b, &[use_of(b, "sha256:1", "api")])
        .await
        .unwrap();
    db::replace_uses(&store, a, &[use_of(a, "sha256:3", "web")])
        .await
        .unwrap();
    let mut got = db::uses_for_envs(&store, &[a, b]).await.unwrap();
    got.sort_by(|x, y| x.key.cmp(&y.key));
    assert_eq!(
        got,
        vec![use_of(b, "sha256:1", "api"), use_of(a, "sha256:3", "web")]
    );
}

#[tokio::test]
async fn finish_ok_reports_only_new_findings() {
    let store = store().await;
    let e = env(&store, "local").await;
    let one = db::insert_scan(&store, &target(e, "sha256:k"), Trigger::Arrival, 10)
        .await
        .unwrap();
    assert_eq!(
        db::active_scan(&store, &ImageKey("sha256:k".into()))
            .await
            .unwrap(),
        Some(one)
    );
    db::mark_running(&store, one, 11).await.unwrap();
    let new = db::finish_ok(
        &store,
        one,
        "trivy:1",
        &[finding("CVE-1", Severity::Critical)],
        12,
    )
    .await
    .unwrap();
    assert_eq!(new.len(), 1);
    assert_eq!(
        db::active_scan(&store, &ImageKey("sha256:k".into()))
            .await
            .unwrap(),
        None
    );

    let two = db::insert_scan(&store, &target(e, "sha256:k"), Trigger::Periodic, 100)
        .await
        .unwrap();
    db::mark_running(&store, two, 101).await.unwrap();
    let new = db::finish_ok(
        &store,
        two,
        "trivy:1",
        &[
            finding("CVE-1", Severity::Critical),
            finding("CVE-2", Severity::High),
        ],
        102,
    )
    .await
    .unwrap();
    assert_eq!(
        new.iter().map(|f| f.vuln_id.as_str()).collect::<Vec<_>>(),
        ["CVE-2"]
    );
    let rows = db::findings(&store, two).await.unwrap();
    let first = |id: &str| {
        rows.iter()
            .find(|r| r.finding.vuln_id == id)
            .unwrap()
            .first_seen_at
    };
    assert_eq!(
        first("CVE-1"),
        12,
        "first seen is kept from the earlier scan"
    );
    assert_eq!(first("CVE-2"), 102);
    let state = db::latest(&store, &[ImageKey("sha256:k".into())])
        .await
        .unwrap();
    let s = &state[&ImageKey("sha256:k".into())];
    assert_eq!(s.last.id, two);
    assert_eq!(
        s.last_done
            .as_ref()
            .unwrap()
            .counts
            .unwrap()
            .critical_fixable,
        1
    );
    assert_eq!(s.last_done.as_ref().unwrap().counts.unwrap().high, 1);
}

#[tokio::test]
async fn failed_scan_keeps_the_last_good_findings() {
    let store = store().await;
    let e = env(&store, "local").await;
    let ok = db::insert_scan(&store, &target(e, "sha256:k"), Trigger::Arrival, 1)
        .await
        .unwrap();
    db::finish_ok(
        &store,
        ok,
        "trivy:1",
        &[finding("CVE-1", Severity::Critical)],
        2,
    )
    .await
    .unwrap();
    let bad = db::insert_scan(&store, &target(e, "sha256:k"), Trigger::Periodic, 3)
        .await
        .unwrap();
    db::finish_failed(&store, bad, "trivy exited 1: boom", 4)
        .await
        .unwrap();
    let state = db::latest(&store, &[ImageKey("sha256:k".into())])
        .await
        .unwrap();
    let s = &state[&ImageKey("sha256:k".into())];
    assert_eq!(s.last.status, Status::Failed);
    assert_eq!(s.last.error.as_deref(), Some("trivy exited 1: boom"));
    assert_eq!(s.last_done.as_ref().unwrap().id, ok);
    assert_eq!(db::findings(&store, ok).await.unwrap().len(), 1);
}

#[tokio::test]
async fn older_findings_are_dropped_and_history_capped_at_20() {
    let store = store().await;
    let e = env(&store, "local").await;
    let mut ids = vec![];
    for i in 0..25 {
        let id = db::insert_scan(&store, &target(e, "sha256:k"), Trigger::Periodic, i)
            .await
            .unwrap();
        db::finish_ok(&store, id, "trivy:1", &[finding("CVE-1", Severity::Low)], i)
            .await
            .unwrap();
        ids.push(id);
    }
    assert!(
        db::findings(&store, ids[23]).await.unwrap().is_empty(),
        "only the latest keeps findings"
    );
    assert_eq!(db::findings(&store, ids[24]).await.unwrap().len(), 1);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ee_scans WHERE image_key = 'sha256:k'")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(n, 20);
}

#[tokio::test]
async fn recover_marks_running_failed_and_returns_queued() {
    let store = store().await;
    let e = env(&store, "local").await;
    let running = db::insert_scan(&store, &target(e, "sha256:a"), Trigger::Arrival, 1)
        .await
        .unwrap();
    db::mark_running(&store, running, 2).await.unwrap();
    let queued = db::insert_scan(&store, &target(e, "sha256:b"), Trigger::Arrival, 3)
        .await
        .unwrap();
    let again = db::recover_after_restart(&store, 10).await.unwrap();
    let mut keys: Vec<_> = again.iter().map(|t| t.key.as_str().to_string()).collect();
    keys.sort();
    assert_eq!(keys, ["sha256:a", "sha256:b"]);
    let st = db::latest(&store, &[ImageKey("sha256:a".into())])
        .await
        .unwrap();
    let a = &st[&ImageKey("sha256:a".into())].last;
    assert_eq!(a.id, running);
    assert_eq!(a.status, Status::Failed);
    assert_eq!(a.error.as_deref(), Some("interrupted by restart"));
    assert_eq!(
        db::scan(&store, queued).await.unwrap().unwrap().status,
        Status::Failed
    );
}

#[tokio::test]
async fn failing_an_environments_queue_leaves_others() {
    let store = store().await;
    let (a, b) = (env(&store, "a").await, env(&store, "b").await);
    let qa = db::insert_scan(&store, &target(a, "sha256:1"), Trigger::Arrival, 1)
        .await
        .unwrap();
    let qb = db::insert_scan(&store, &target(b, "sha256:2"), Trigger::Arrival, 1)
        .await
        .unwrap();
    db::fail_queued_for_env(&store, a, "scanning turned off", 2)
        .await
        .unwrap();
    assert_eq!(
        db::scan(&store, qa).await.unwrap().unwrap().status,
        Status::Failed
    );
    assert_eq!(
        db::scan(&store, qb).await.unwrap().unwrap().status,
        Status::Queued
    );
}

#[tokio::test]
async fn prune_keeps_keys_in_use() {
    let store = store().await;
    let e = env(&store, "local").await;
    for key in ["sha256:used", "sha256:old"] {
        let id = db::insert_scan(&store, &target(e, key), Trigger::Arrival, 1)
            .await
            .unwrap();
        db::finish_ok(&store, id, "trivy:1", &[finding("CVE-1", Severity::Low)], 1)
            .await
            .unwrap();
    }
    db::replace_uses(&store, e, &[use_of(e, "sha256:used", "web")])
        .await
        .unwrap();
    db::prune_unused(&store, 100).await.unwrap();
    let keys = [
        ImageKey("sha256:used".into()),
        ImageKey("sha256:old".into()),
    ];
    let st = db::latest(&store, &keys).await.unwrap();
    assert!(st.contains_key(&keys[0]));
    assert!(!st.contains_key(&keys[1]));
}

#[tokio::test]
async fn uses_round_trip_platform() {
    let store = store().await;
    let e = env(&store, "k").await;
    let mut a = use_of(e, "sha256:a", "app");
    a.platform = Some("linux/amd64".into());
    let b = use_of(e, "sha256:b", "web");
    db::replace_uses(&store, e, &[a.clone(), b.clone()])
        .await
        .unwrap();
    let got = db::uses_for_envs(&store, &[e]).await.unwrap();
    let p = |k: &str| {
        got.iter()
            .find(|u| u.key.as_str() == k)
            .unwrap()
            .platform
            .clone()
    };
    assert_eq!(p("sha256:a").as_deref(), Some("linux/amd64"));
    assert_eq!(p("sha256:b"), None);

    let mut t = target(e, "sha256:a");
    t.platform = Some("linux/arm64".into());
    let id = db::insert_scan(&store, &t, Trigger::Manual, 1)
        .await
        .unwrap();
    let row = db::scan(&store, id).await.unwrap().unwrap();
    assert_eq!(row.target.platform.as_deref(), Some("linux/arm64"));
}
