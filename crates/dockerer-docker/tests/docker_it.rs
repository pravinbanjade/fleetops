//! Runs against a real Docker daemon. Enable with `--features docker-it`;
//! the endpoint comes from DOCKER_HOST (see `make test-docker`).
#![cfg(feature = "docker-it")]

use bollard::query_parameters::CreateImageOptionsBuilder;
use dockerer_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, EntryKind, ImageOps, NetworkOps, NetworkSpec,
    PortBinding, SystemOps, VolumeOps, VolumeSpec,
};
use dockerer_core::env::{EnvFactory, EnvKind, EnvStatus};
use dockerer_core::Error;
use dockerer_docker::{DockerEnvFactory, LocalDocker};
use futures_util::StreamExt;
use std::collections::BTreeMap;

const IMAGE: &str = "alpine:3.20";

fn host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into())
}

fn unique(name: &str) -> String {
    format!("dkr-it-{}-{name}", std::process::id())
}

async fn docker() -> LocalDocker {
    let raw = bollard::Docker::connect_with_host(&host()).unwrap();
    let mut pull = raw.create_image(
        Some(
            CreateImageOptionsBuilder::new()
                .from_image("alpine")
                .tag("3.20")
                .build(),
        ),
        None,
        None,
    );
    while let Some(step) = pull.next().await {
        step.expect("pull alpine:3.20");
    }
    LocalDocker::connect(&host()).unwrap()
}

fn sleeper(name: &str) -> ContainerSpec {
    ContainerSpec {
        name: Some(name.to_string()),
        image: IMAGE.into(),
        cmd: Some(vec!["sleep".into(), "300".into()]),
        env: vec!["DOCKERER_IT=1".into()],
        ports: vec![PortBinding {
            container_port: 8080,
            protocol: "tcp".into(),
            host_ip: None,
            host_port: None,
        }],
        labels: BTreeMap::from([("dockerer.it".into(), "1".into())]),
        restart_policy: Some("no".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn environment_status_is_up() {
    let env = DockerEnvFactory
        .build(1, "local", EnvKind::DockerLocal, &host())
        .unwrap();
    match env.status().await {
        EnvStatus::Up { version } => assert!(!version.api_version.is_empty()),
        other => panic!("expected up, got {other:?}"),
    }
    assert!(env.docker().is_some());
}

#[tokio::test]
async fn system_info_and_disk_usage() {
    let d = docker().await;
    assert!(d.info().await.unwrap()["NCPU"].as_i64().unwrap() >= 1);
    assert!(d.disk_usage().await.unwrap().is_object());
}

#[tokio::test]
async fn container_lifecycle() {
    let d = docker().await;
    let name = unique("life");
    let id = d.create_container(&sleeper(&name)).await.unwrap();
    d.container_action(&id, ContainerAction::Start)
        .await
        .unwrap();

    let listed = d
        .list_containers()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    assert_eq!(listed.name, name);
    assert_eq!(listed.state, "running");
    assert_eq!(
        listed.labels.get("dockerer.it").map(String::as_str),
        Some("1")
    );
    assert!(listed
        .ports
        .iter()
        .any(|p| p.private_port == 8080 && p.public_port.is_some()));

    let spec = d.container_spec(&id).await.unwrap();
    assert_eq!(spec.image, IMAGE);
    assert_eq!(spec.cmd, Some(vec!["sleep".into(), "300".into()]));
    assert!(spec.env.contains(&"DOCKERER_IT=1".to_string()));
    assert_eq!(spec.ports[0].container_port, 8080);

    d.container_action(&id, ContainerAction::Pause)
        .await
        .unwrap();
    d.container_action(&id, ContainerAction::Unpause)
        .await
        .unwrap();
    d.container_action(&id, ContainerAction::Kill)
        .await
        .unwrap();
    let renamed = unique("life2");
    d.rename_container(&id, &renamed).await.unwrap();
    assert_eq!(
        d.inspect_container(&id).await.unwrap()["Name"],
        format!("/{renamed}")
    );
    d.remove_container(&id, true, false).await.unwrap();

    let err = d.inspect_container(&id).await.unwrap_err();
    assert!(
        matches!(err, Error::Upstream { status: 404, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn advanced_override_reaches_docker() {
    let d = docker().await;
    let spec = ContainerSpec {
        advanced: Some(serde_json::json!({"WorkingDir": "/tmp"})),
        ..sleeper(&unique("adv"))
    };
    let id = d.create_container(&spec).await.unwrap();
    assert_eq!(
        d.inspect_container(&id).await.unwrap()["Config"]["WorkingDir"],
        "/tmp"
    );
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn duplicate_name_is_upstream_conflict() {
    let d = docker().await;
    let name = unique("dup");
    let id = d.create_container(&sleeper(&name)).await.unwrap();
    let err = d.create_container(&sleeper(&name)).await.unwrap_err();
    assert!(
        matches!(err, Error::Upstream { status: 409, .. }),
        "{err:?}"
    );
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn browse_and_read_files() {
    let d = docker().await;
    let id = d
        .create_container(&sleeper(&unique("files")))
        .await
        .unwrap();
    let etc = d.list_dir(&id, "/etc").await.unwrap();
    assert!(etc
        .entries
        .iter()
        .any(|e| e.name == "passwd" && e.kind == EntryKind::File));
    assert!(etc
        .entries
        .iter()
        .any(|e| e.name == "apk" && e.kind == EntryKind::Dir));
    let root = d.list_dir(&id, "/").await.unwrap();
    assert!(root.entries.iter().any(|e| e.name == "bin"));
    let release = d.read_file(&id, "/etc/alpine-release", 1024).await.unwrap();
    assert!(String::from_utf8_lossy(&release).starts_with("3.20"));
    assert!(matches!(
        d.read_file(&id, "/etc/passwd", 4).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.read_file(&id, "/etc", 1 << 20).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.list_dir(&id, "/nope").await,
        Err(Error::Upstream { status: 404, .. })
    ));
    d.remove_container(&id, true, false).await.unwrap();
}

#[tokio::test]
async fn images_list_tag_and_remove() {
    let d = docker().await;
    let images = d.list_images().await.unwrap();
    let alpine = images
        .iter()
        .find(|i| i.repo_tags.iter().any(|t| t == IMAGE))
        .unwrap();
    assert!(alpine.size > 0);
    assert!(d.inspect_image(IMAGE).await.unwrap()["Id"].is_string());
    assert!(d
        .image_history(IMAGE)
        .await
        .unwrap()
        .as_array()
        .is_some_and(|h| !h.is_empty()));
    let repo = unique("tag");
    d.tag_image(IMAGE, &repo, "it").await.unwrap();
    let removed = d.remove_image(&format!("{repo}:it"), false).await.unwrap();
    assert_eq!(removed, vec![format!("{repo}:it")]);
}

#[tokio::test]
async fn volumes_create_list_remove() {
    let d = docker().await;
    let name = unique("vol");
    let v = d
        .create_volume(&VolumeSpec {
            name: name.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(v.name, name);
    let listed = d
        .list_volumes()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == name)
        .unwrap();
    assert!(!listed.in_use);

    let spec = ContainerSpec {
        binds: vec![format!("{name}:/data")],
        ..sleeper(&unique("volc"))
    };
    let cid = d.create_container(&spec).await.unwrap();
    assert!(
        d.list_volumes()
            .await
            .unwrap()
            .into_iter()
            .find(|v| v.name == name)
            .unwrap()
            .in_use
    );
    assert!(matches!(
        d.remove_volume(&name, false).await,
        Err(Error::Upstream { status: 409, .. })
    ));
    d.remove_container(&cid, true, false).await.unwrap();
    d.remove_volume(&name, false).await.unwrap();
}

#[tokio::test]
async fn networks_create_connect_remove() {
    let d = docker().await;
    let name = unique("net");
    let id = d
        .create_network(&NetworkSpec {
            name: name.clone(),
            subnet: Some("10.231.0.0/24".into()),
            attachable: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let listed = d
        .list_networks()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.id == id)
        .unwrap();
    assert_eq!(listed.subnet.as_deref(), Some("10.231.0.0/24"));
    assert!(listed.attachable);

    let cid = d.create_container(&sleeper(&unique("netc"))).await.unwrap();
    d.container_action(&cid, ContainerAction::Start)
        .await
        .unwrap();
    d.connect_network(&id, &cid).await.unwrap();
    assert!(d.inspect_network(&id).await.unwrap()["Containers"]
        .as_object()
        .is_some_and(|m| m.contains_key(&cid)));
    d.disconnect_network(&id, &cid, true).await.unwrap();
    d.remove_container(&cid, true, false).await.unwrap();
    d.remove_network(&id).await.unwrap();
}
