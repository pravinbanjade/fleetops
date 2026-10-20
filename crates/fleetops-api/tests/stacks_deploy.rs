mod common;

use common::fake_docker::FakeDocker;
use fleetops_api::stacks::{execute, restore, Timing};
use fleetops_core::compose::docker::{LABEL_REVISION, LABEL_SERVICE};
use fleetops_core::compose::plan::{plan, Live};
use fleetops_core::compose::{parse, Project};
use fleetops_core::docker::{ContainerOps, NetworkOps, VolumeOps};
use fleetops_core::Result;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn timing() -> Timing {
    Timing {
        exit_check: Duration::from_millis(5),
        poll: Duration::from_millis(1),
        health_cap: Duration::from_millis(50),
    }
}

fn project(yaml: &str) -> Project {
    parse(yaml, &[], "shop").expect("valid").project
}

async fn live(f: &FakeDocker) -> Live {
    Live {
        containers: f.list_containers().await.unwrap(),
        networks: f.list_networks().await.unwrap(),
        volumes: f.list_volumes().await.unwrap(),
        images: vec![],
    }
}

async fn deploy(f: &Arc<FakeDocker>, p: &Project, prev: Option<&Project>, rev: i64) -> Result<()> {
    let plan = plan(p, prev.map(|x| (x, rev - 1)), &live(f).await);
    execute(f.as_ref(), p, &plan, rev, &timing(), &|_| {}).await
}

/// Containers as (name, state, service, revision), sorted.
fn snapshot(f: &FakeDocker) -> Vec<(String, String, String, String)> {
    let mut v: Vec<_> = f
        .state
        .lock()
        .unwrap()
        .containers
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.state.clone(),
                c.labels.get(LABEL_SERVICE).cloned().unwrap_or_default(),
                c.labels.get(LABEL_REVISION).cloned().unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

fn network_names(f: &FakeDocker) -> Vec<String> {
    let mut v: Vec<String> = f
        .state
        .lock()
        .unwrap()
        .networks
        .iter()
        .map(|n| n.name.clone())
        .collect();
    v.sort();
    v
}

fn pos(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .position(|c| c.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix} in {calls:?}"))
}

const THREE: &str = r#"
services:
  web: { image: x, depends_on: [api] }
  api: { image: x, depends_on: [db] }
  db: { image: x }
"#;

#[tokio::test]
async fn fresh_deploy_creates_in_order() {
    let f = FakeDocker::new();
    deploy(&f, &project(THREE), None, 1).await.unwrap();
    let calls = f.calls();
    let pull = pos(&calls, "pull x:latest");
    let net = pos(&calls, "create_network shop_default");
    let db = pos(&calls, "create_raw:shop-db-1");
    let api = pos(&calls, "create_raw:shop-api-1");
    let web = pos(&calls, "create_raw:shop-web-1");
    assert!(pull < net && net < db && db < api && api < web, "{calls:?}");
    let snap = snapshot(&f);
    assert_eq!(snap.len(), 3);
    assert!(
        snap.iter().all(|c| c.1 == "running" && c.3 == "1"),
        "{snap:?}"
    );
}

#[tokio::test]
async fn recreate_renames_then_commits() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: 'x:1'}\n");
    let v2 = project("services:\n  web: {image: 'x:2'}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    deploy(&f, &v2, Some(&v1), 2).await.unwrap();
    let calls = f.calls();
    let rename = pos(&calls, "rename new1 shop-web-1-fleetops-old-2-run");
    let removed = calls
        .iter()
        .rposition(|c| c == "remove new1")
        .expect("old removed at commit");
    let created2 = calls
        .iter()
        .rposition(|c| c == "create_raw:shop-web-1")
        .unwrap();
    assert!(rename < created2 && created2 < removed, "{calls:?}");
    assert_eq!(
        snapshot(&f),
        vec![(
            "shop-web-1".into(),
            "running".into(),
            "web".into(),
            "2".into()
        )]
    );
}

#[tokio::test]
async fn orphan_stopped_then_removed_at_commit() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: x}\n  worker: {image: x}\n");
    let v2 = project("services:\n  web: {image: x}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    let worker = f.container("shop-worker-1").unwrap().id;
    deploy(&f, &v2, Some(&v1), 2).await.unwrap();
    let calls = f.calls();
    let stop = pos(&calls, &format!("stop {worker}"));
    let remove = pos(&calls, &format!("remove {worker}"));
    assert!(stop < remove);
    assert!(f.container("shop-worker-1").is_none());
    assert_eq!(snapshot(&f).len(), 1);
}

#[tokio::test]
async fn scale_down_deferred_to_commit() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: x, deploy: {replicas: 3}}\n");
    let v2 = project("services:\n  web: {image: x}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    assert_eq!(snapshot(&f).len(), 3);
    let third = f.container("shop-web-3").unwrap().id;
    deploy(&f, &v2, Some(&v1), 2).await.unwrap();
    let calls = f.calls();
    assert!(
        pos(
            &calls,
            &format!("rename {third} shop-web-3-fleetops-old-2-run")
        ) < pos(&calls, &format!("remove {third}"))
    );
    let snap = snapshot(&f);
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].0, "shop-web-1");
    assert_eq!(snap[0].3, "1", "the kept replica is not recreated");
}

#[tokio::test]
async fn failure_at_each_step_restores() {
    let v1 = project("services:\n  web: {image: 'x:1'}\n");
    let v2 = project(
        "services:\n  web: {image: 'x:2', networks: [default, back]}\nnetworks:\n  back:\n",
    );
    for fail in [
        "pull x:2",
        "create_network shop_back",
        "create_raw:shop-web-1",
        "start new2",
    ] {
        let f = FakeDocker::new();
        deploy(&f, &v1, None, 1).await.unwrap();
        let before = (snapshot(&f), network_names(&f));
        *f.fail_on.lock().unwrap() = Some(fail.into());
        let err = deploy(&f, &v2, Some(&v1), 2).await;
        assert!(err.is_err(), "{fail}: deploy should fail");
        *f.fail_on.lock().unwrap() = None;
        restore(f.as_ref(), "shop", 2).await.unwrap();
        assert_eq!((snapshot(&f), network_names(&f)), before, "{fail}");
    }
}

#[tokio::test]
async fn port_conflict_restores_running_old() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: x, ports: ['8080:80']}\n");
    let v2 = project("services:\n  web: {image: x, ports: ['8080:80'], environment: {A: '1'}}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    // The new container cannot bind 8080, so its start fails.
    *f.fail_on.lock().unwrap() = Some("start new2".into());
    assert!(deploy(&f, &v2, Some(&v1), 2).await.is_err());
    *f.fail_on.lock().unwrap() = None;
    restore(f.as_ref(), "shop", 2).await.unwrap();
    let web = f.container("shop-web-1").unwrap();
    assert_eq!((web.id.as_str(), web.state.as_str()), ("new1", "running"));
    assert_eq!(snapshot(&f).len(), 1);
}

#[tokio::test]
async fn health_timeout_fails() {
    let f = FakeDocker::new();
    f.state.lock().unwrap().inspect_state.insert(
        "shop-db-1".into(),
        json!({"Health": {"Status": "starting"}}),
    );
    let p = project(
        r#"
services:
  db:
    image: x
    restart: always
    healthcheck: { test: "true", interval: 1s, retries: 1 }
  web:
    image: x
    depends_on: { db: { condition: service_healthy } }
"#,
    );
    let e = deploy(&f, &p, None, 1).await.unwrap_err();
    assert!(e.to_string().contains("did not become ready"), "{e}");
    assert!(f.container("shop-web-1").is_none());
}

#[tokio::test]
async fn quick_nonzero_exit_fails() {
    let f = FakeDocker::new();
    f.state.lock().unwrap().inspect_state.insert(
        "shop-web-1".into(),
        json!({"Running": false, "ExitCode": 1}),
    );
    let e = deploy(&f, &project("services:\n  web: {image: x}\n"), None, 1)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("exited with code 1"), "{e}");
}

#[tokio::test]
async fn restore_is_idempotent() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: 'x:1'}\n");
    let v2 = project("services:\n  web: {image: 'x:2'}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    *f.fail_on.lock().unwrap() = Some("start new2".into());
    assert!(deploy(&f, &v2, Some(&v1), 2).await.is_err());
    *f.fail_on.lock().unwrap() = None;
    let first = restore(f.as_ref(), "shop", 2).await.unwrap();
    assert_eq!(first.restored, vec!["shop-web-1".to_string()]);
    let second = restore(f.as_ref(), "shop", 2).await.unwrap();
    assert!(second.removed.is_empty() && second.restored.is_empty());
}

#[tokio::test]
async fn restarting_container_is_stopped_before_set_aside() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  web: {image: 'x:1', ports: ['8080:80']}\n");
    let v2 = project("services:\n  web: {image: 'x:2', ports: ['8080:80']}\n");
    deploy(&f, &v1, None, 1).await.unwrap();
    f.state.lock().unwrap().containers[0].state = "restarting".into();
    *f.fail_on.lock().unwrap() = Some("start new2".into());
    assert!(deploy(&f, &v2, Some(&v1), 2).await.is_err());
    let calls = f.calls();
    assert!(
        pos(&calls, "stop new1") < pos(&calls, "rename new1 shop-web-1-fleetops-old-2-run"),
        "{calls:?}"
    );
    *f.fail_on.lock().unwrap() = None;
    restore(f.as_ref(), "shop", 2).await.unwrap();
    assert_eq!(f.container("shop-web-1").unwrap().state, "running");
}

#[tokio::test]
async fn restore_starts_network_mode_targets_first() {
    let f = FakeDocker::new();
    let yaml = |tag: &str| {
        format!("services:\n  b: {{image: 'x:{tag}'}}\n  a: {{image: 'x:{tag}', network_mode: 'service:b'}}\n")
    };
    let (v1, v2) = (project(&yaml("1")), project(&yaml("2")));
    deploy(&f, &v1, None, 1).await.unwrap();
    let (b, a) = (
        f.container("shop-b-1").unwrap().id,
        f.container("shop-a-1").unwrap().id,
    );
    *f.fail_on.lock().unwrap() = Some("start new4".into());
    assert!(deploy(&f, &v2, Some(&v1), 2).await.is_err());
    *f.fail_on.lock().unwrap() = None;
    let before = f.calls().len();
    restore(f.as_ref(), "shop", 2).await.unwrap();
    let calls = f.calls()[before..].to_vec();
    assert!(
        pos(&calls, &format!("start {b}")) < pos(&calls, &format!("start {a}")),
        "{calls:?}"
    );
    let last_rename = calls.iter().rposition(|c| c.starts_with("rename")).unwrap();
    assert!(
        last_rename < pos(&calls, "start"),
        "every name is back before a start: {calls:?}"
    );
}

#[tokio::test]
async fn anonymous_volumes_carried_over() {
    let f = FakeDocker::new();
    let v1 = project("services:\n  db: {image: postgres, volumes: ['/var/lib/postgresql/data']}\n");
    let v2 = project(
        "services:\n  db: {image: postgres, volumes: ['/var/lib/postgresql/data'], environment: {A: '1'}}\n",
    );
    deploy(&f, &v1, None, 1).await.unwrap();
    f.state.lock().unwrap().inspect_extra.insert(
        "new1".into(),
        json!({"Mounts": [
            {"Type": "volume", "Name": "anon1", "Destination": "/var/lib/postgresql/data"},
            {"Type": "volume", "Name": "anon2", "Destination": "/image/declared"}
        ]}),
    );
    deploy(&f, &v2, Some(&v1), 2).await.unwrap();
    let st = f.state.lock().unwrap();
    let (_, body) = st.created_bodies.last().unwrap();
    let mounts = body["HostConfig"]["Mounts"].as_array().unwrap();
    assert!(mounts.contains(&json!({"Type": "volume", "Source": "anon1", "Target": "/var/lib/postgresql/data", "ReadOnly": false})), "{mounts:?}");
    assert!(
        mounts.contains(&json!({"Type": "volume", "Source": "anon2", "Target": "/image/declared"})),
        "{mounts:?}"
    );
}
