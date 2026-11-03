//! Real Trivy scans on the current docker context's daemon:
//! `cargo test -p fleetops-ee --features docker-it --test scan_docker_it`.
//! Pulls the pinned Trivy image and alpine:3.20; downloads Trivy's database.
#![cfg(feature = "docker-it")]

use fleetops_core::docker::{ContainerOps, DockerOps, VolumeOps};
use fleetops_core::streams::StreamOps;
use fleetops_docker::LocalDocker;
use fleetops_ee::scan::{
    config::{effective, Effective, GlobalConfig},
    docker,
    inventory::SCAN_LABEL,
    model::{ImageKey, ScanTarget},
    trivy::parse_report,
};
use futures_util::StreamExt;

fn host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into())
}

async fn docker() -> LocalDocker {
    let d = LocalDocker::connect(&host()).unwrap();
    let mut pull = d.pull_image("alpine", "3.20");
    while let Some(m) = pull.next().await {
        m.expect("pull alpine:3.20");
    }
    d
}

fn target(scan: &str) -> ScanTarget {
    ScanTarget {
        env_id: 1,
        node: String::new(),
        key: ImageKey(format!("local:1::{scan}")),
        image_ref: "alpine:3.20".into(),
        namespace: String::new(),
        pull_secrets: vec![],
    }
}

fn cfg() -> Effective {
    effective(&GlobalConfig::default(), None, None, None)
}

async fn no_scan_containers_left(d: &dyn DockerOps, key: &str) {
    let left: Vec<_> = d
        .list_containers()
        .await
        .unwrap()
        .into_iter()
        .filter(|c| c.labels.get("fleetops.scan.key").map(String::as_str) == Some(key))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn scans_alpine_via_socket() {
    let d = docker().await;
    let c = cfg();
    docker::prepare(&d, &c).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let report = docker::scan(&d, &target("socket"), &c, 1, dir.path())
        .await
        .unwrap();
    parse_report(&report).unwrap();
    no_scan_containers_left(&d, "local:1::socket").await;
}

#[tokio::test]
async fn scans_via_export_fallback() {
    let d = docker().await;
    let mut c = cfg();
    docker::prepare(&d, &c).await.unwrap();
    c.socket_path = Some("/nonexistent/fleetops-it.sock".into());
    let dir = tempfile::tempdir().unwrap();
    let report = docker::scan(&d, &target("export"), &c, 2, dir.path())
        .await
        .unwrap();
    parse_report(&report).unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "the spooled image is removed"
    );
    no_scan_containers_left(&d, "local:1::export").await;
}

#[tokio::test]
async fn bad_trivy_image_fails_prepare() {
    let d = docker().await;
    let mut c = cfg();
    c.trivy_image = "fleetops-nope/trivy:0".into();
    let e = docker::prepare(&d, &c).await.unwrap_err();
    assert!(e.to_string().contains("fleetops-nope/trivy:0"), "{e}");
}

#[tokio::test]
async fn cleanup_removes_labelled_containers() {
    let d = docker().await;
    let body =
        serde_json::json!({"Image": "alpine:3.20", "Cmd": ["true"], "Labels": {SCAN_LABEL: "1"}});
    let id = d
        .create_container_raw("fleetops-scan-it-orphan", &body)
        .await
        .unwrap();
    docker::cleanup_orphans(&d).await.unwrap();
    assert!(d.inspect_container(&id).await.is_err());
}

#[tokio::test]
async fn unreachable_db_repository_fails_prepare() {
    let d = docker().await;
    let mut c = cfg();
    c.db_repository = Some("127.0.0.1:9/fleetops-nope/trivy-db".into());
    // An empty cache, as on first turn-on: a fresh cached database is not downloaded again.
    let volume = format!("fleetops-trivy-cache-it-{}", std::process::id());
    let e = docker::prepare_with_cache(&d, &c, &volume)
        .await
        .unwrap_err();
    let _ = d.remove_volume(&volume, true).await;
    assert!(e.to_string().contains("database"), "{e}");
}
