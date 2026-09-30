//! Runs against a real Docker daemon. Enable with `--features docker-it`;
//! the endpoint comes from DOCKER_HOST (see `make test-docker`).
#![cfg(feature = "docker-it")]

use bollard::query_parameters::CreateImageOptionsBuilder;
use fleetops_core::docker::{
    ContainerAction, ContainerOps, ContainerSpec, EntryKind, ImageOps, NetworkOps, NetworkSpec,
    PortBinding, SystemOps, VolumeOps, VolumeSpec,
};
use fleetops_core::env::{EnvFactory, EnvKind, EnvStatus};
use fleetops_core::Error;
use fleetops_docker::{DockerEnvFactory, LocalDocker};
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
        env: vec!["FLEETOPS_IT=1".into()],
        ports: vec![PortBinding {
            container_port: 8080,
            protocol: "tcp".into(),
            host_ip: None,
            host_port: None,
        }],
        labels: BTreeMap::from([("fleetops.it".into(), "1".into())]),
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
        listed.labels.get("fleetops.it").map(String::as_str),
        Some("1")
    );
    assert!(listed
        .ports
        .iter()
        .any(|p| p.private_port == 8080 && p.public_port.is_some()));

    let spec = d.container_spec(&id).await.unwrap();
    assert_eq!(spec.image, IMAGE);
    assert_eq!(spec.cmd, Some(vec!["sleep".into(), "300".into()]));
    assert!(spec.env.contains(&"FLEETOPS_IT=1".to_string()));
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

#[tokio::test]
async fn recreate_by_name_keeps_mounted_volume() {
    let d = docker().await;
    let name = unique("recreate");
    let vol = unique("recreate-vol");
    let mut spec = sleeper(&name);
    spec.advanced = Some(serde_json::json!({
        "HostConfig": {"Mounts": [{"Type": "volume", "Source": vol, "Target": "/data"}]}
    }));
    let old_id = d.create_container(&spec).await.unwrap();
    d.container_action(&old_id, ContainerAction::Start)
        .await
        .unwrap();

    // The form round-trip: prefill from the live container, change one thing,
    // recreate addressing the container by name.
    let mut edited = d.container_spec(&name).await.unwrap();
    edited.env.push("CHANGED=1".into());
    let out = fleetops_core::recreate::recreate(&d, &name, &edited, 1)
        .await
        .unwrap();
    assert!(out.old_removed);

    let new = d.inspect_container(&name).await.unwrap();
    assert_eq!(
        new["Id"],
        out.new_id.as_str(),
        "name points at the replacement"
    );
    assert_eq!(new["State"]["Running"], true);
    assert!(new["Config"]["Env"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("CHANGED=1")));
    let mounts = new["Mounts"].as_array().unwrap();
    assert!(
        mounts
            .iter()
            .any(|m| m["Name"] == vol.as_str() && m["Destination"] == "/data"),
        "volume survived recreate: {mounts:?}"
    );
    assert!(d.inspect_container(&old_id).await.is_err(), "old removed");

    d.remove_container(&out.new_id, true, false).await.unwrap();
    d.remove_volume(&vol, true).await.unwrap();
}

#[tokio::test]
async fn archive_downloads_wait_for_a_free_permit() {
    let d = docker().await;
    let name = unique("permits");
    let id = d.create_container(&sleeper(&name)).await.unwrap();
    let all = fleetops_docker::files::archive_permits()
        .acquire_many(fleetops_docker::files::ARCHIVE_CONCURRENCY as u32)
        .await
        .unwrap();
    let blocked = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        d.list_dir(&id, "/etc"),
    )
    .await;
    assert!(
        blocked.is_err(),
        "listing must wait while every permit is held"
    );
    drop(all);
    assert!(!d.list_dir(&id, "/etc").await.unwrap().entries.is_empty());
    d.remove_container(&id, true, false).await.unwrap();
}

/// A unix-socket proxy to the real daemon that can be killed, taking every
/// open connection with it, to simulate the daemon disappearing.
fn socket_proxy(path: std::path::PathBuf) -> tokio::task::JoinHandle<()> {
    let upstream = host()
        .strip_prefix("unix://")
        .expect("docker-it proxy test needs a unix DOCKER_HOST")
        .to_string();
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            let (mut down, _) = listener.accept().await.unwrap();
            let upstream = upstream.clone();
            conns.spawn(async move {
                if let Ok(mut up) = tokio::net::UnixStream::connect(&upstream).await {
                    let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
                }
            });
        }
    })
}

#[tokio::test]
async fn daemon_loss_mid_session_is_unavailable_then_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("docker.sock");
    let proxy = socket_proxy(sock.clone());
    let env = DockerEnvFactory
        .build(
            1,
            "flaky",
            EnvKind::DockerLocal,
            &format!("unix://{}", sock.display()),
        )
        .unwrap();
    assert!(matches!(env.status().await, EnvStatus::Up { .. }));
    let ops = env.docker().expect("reachable");
    ops.list_containers().await.unwrap();

    // The daemon goes away while a client handle is still held.
    proxy.abort();
    let _ = proxy.await;
    std::fs::remove_file(&sock).unwrap();
    assert!(matches!(env.status().await, EnvStatus::Down { .. }));
    match ops.list_containers().await {
        Err(Error::EnvUnavailable(_)) => {}
        other => panic!("expected EnvUnavailable, got {other:?}"),
    }
    if let Some(fresh) = env.docker() {
        assert!(matches!(
            fresh.list_containers().await,
            Err(Error::EnvUnavailable(_))
        ));
    }

    // It comes back; the same environment recovers without a restart.
    let proxy = socket_proxy(sock.clone());
    assert!(matches!(env.status().await, EnvStatus::Up { .. }));
    env.docker()
        .expect("reachable again")
        .list_containers()
        .await
        .unwrap();
    proxy.abort();
}

mod streams {
    use super::*;
    use fleetops_core::streams::{
        BuildContext, BuildSpec, ExecOptions, LogOptions, LogStream, StreamOps,
    };
    use std::time::Duration;

    async fn run_to_exit(d: &LocalDocker, name: &str, spec: ContainerSpec) -> String {
        let id = d.create_container(&spec).await.unwrap();
        d.container_action(&id, ContainerAction::Start)
            .await
            .unwrap();
        for _ in 0..50 {
            let v = d.inspect_container(&id).await.unwrap();
            if v["State"]["Running"] == false {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{name} did not exit");
    }

    #[tokio::test]
    async fn events_report_container_lifecycle() {
        let d = docker().await;
        let mut events = d.events();
        let name = unique("events");
        let id = d.create_container(&sleeper(&name)).await.unwrap();
        d.container_action(&id, ContainerAction::Start)
            .await
            .unwrap();
        d.remove_container(&id, true, false).await.unwrap();
        let mut seen = Vec::new();
        while !seen.contains(&"destroy".to_string()) {
            let e = tokio::time::timeout(Duration::from_secs(10), events.next())
                .await
                .expect("event within 10s")
                .unwrap()
                .unwrap();
            if e.id == id {
                assert_eq!(e.kind, "container");
                assert_eq!(e.name.as_deref(), Some(name.as_str()));
                assert!(e.time > 0);
                seen.push(e.action);
            }
        }
        for a in ["create", "start", "destroy"] {
            assert!(seen.iter().any(|s| s == a), "{a} missing from {seen:?}");
        }
    }

    #[tokio::test]
    async fn stats_stream_yields_samples() {
        let d = docker().await;
        let name = unique("stats");
        let id = d.create_container(&sleeper(&name)).await.unwrap();
        d.container_action(&id, ContainerAction::Start)
            .await
            .unwrap();
        let samples: Vec<_> = d.stats(&id).take(2).collect().await;
        assert_eq!(samples.len(), 2);
        let s = samples[1].as_ref().unwrap();
        assert!(
            s.memory_limit > 0 && s.memory_used > 0 && s.pids >= 1,
            "{s:?}"
        );
        assert!(s.time > 1_600_000_000_000);
        d.remove_container(&id, true, false).await.unwrap();
    }

    #[tokio::test]
    async fn logs_come_back_as_lines_per_stream() {
        let d = docker().await;
        let name = unique("logs");
        let mut spec = sleeper(&name);
        spec.cmd = Some(vec![
            "sh".into(),
            "-c".into(),
            "echo one; echo two >&2; printf 'no-newline'".into(),
        ]);
        let id = run_to_exit(&d, &name, spec).await;
        let opts = LogOptions {
            follow: false,
            ..Default::default()
        };
        let lines: Vec<_> = d.logs(&id, &opts).map(|l| l.unwrap()).collect().await;
        let got: Vec<_> = lines.iter().map(|l| (l.stream, l.text.as_str())).collect();
        assert!(got.contains(&(LogStream::Stdout, "one")), "{got:?}");
        assert!(got.contains(&(LogStream::Stderr, "two")), "{got:?}");
        assert!(got.contains(&(LogStream::Stdout, "no-newline")), "{got:?}");
        let tail = LogOptions {
            follow: false,
            tail: Some(1),
            stderr: false,
            ..Default::default()
        };
        let last: Vec<_> = d.logs(&id, &tail).map(|l| l.unwrap().text).collect().await;
        assert_eq!(last, ["no-newline"]);
        d.remove_container(&id, true, false).await.unwrap();
    }

    #[tokio::test]
    async fn tty_container_logs_are_split_into_lines() {
        let d = docker().await;
        let name = unique("ttylogs");
        let mut spec = sleeper(&name);
        spec.cmd = Some(vec!["sh".into(), "-c".into(), "printf 'a\\nb\\n'".into()]);
        spec.advanced = Some(serde_json::json!({"Tty": true}));
        let id = run_to_exit(&d, &name, spec).await;
        let opts = LogOptions {
            follow: false,
            ..Default::default()
        };
        let texts: Vec<_> = d.logs(&id, &opts).map(|l| l.unwrap().text).collect().await;
        assert_eq!(texts, ["a", "b"]);
        d.remove_container(&id, true, false).await.unwrap();
    }

    #[tokio::test]
    async fn exec_runs_a_shell_with_input_resize_and_exit_code() {
        let d = docker().await;
        let name = unique("exec");
        let id = d.create_container(&sleeper(&name)).await.unwrap();
        d.container_action(&id, ContainerAction::Start)
            .await
            .unwrap();
        let mut s = d.exec_start(&id, &ExecOptions::default()).await.unwrap();
        d.exec_resize(&s.exec_id, 120, 40).await.unwrap();
        s.input
            .send(b"stty size; echo hi-$((40+2)); exit 3\n".to_vec())
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Ok(Some(chunk)) =
            tokio::time::timeout(Duration::from_secs(10), s.output.next()).await
        {
            out.extend(chunk.unwrap());
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("hi-42"), "{text}");
        assert!(text.contains("40 120"), "resize reached the tty: {text}");
        let mut code = None;
        for _ in 0..20 {
            code = d.exec_exit_code(&s.exec_id).await.unwrap();
            if code.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(code, Some(3));
        d.remove_container(&id, true, false).await.unwrap();
    }

    #[tokio::test]
    async fn exec_in_missing_container_is_upstream_404() {
        let d = docker().await;
        match d
            .exec_start("fleetops-it-nope", &ExecOptions::default())
            .await
        {
            Err(Error::Upstream { status: 404, .. }) => {}
            Err(e) => panic!("expected 404, got {e:?}"),
            Ok(_) => panic!("expected 404, got a session"),
        }
    }

    #[tokio::test]
    async fn pull_streams_progress_and_missing_image_fails() {
        let d = docker().await;
        let items: Vec<_> = d.pull_image("alpine", "3.20").collect().await;
        assert!(!items.is_empty());
        let last = items.last().unwrap().as_ref().unwrap();
        assert!(
            last["status"].as_str().unwrap().contains("alpine:3.20"),
            "{last}"
        );
        let missing: Vec<_> = d
            .pull_image("fleetops-it/does-not-exist", "latest")
            .collect()
            .await;
        assert!(
            matches!(missing.last(), Some(Err(Error::Upstream { .. }))),
            "{missing:?}"
        );
    }

    #[tokio::test]
    async fn build_from_a_dockerfile_streams_output_and_tags() {
        let d = docker().await;
        let tag = format!("fleetops-it-build:{}", std::process::id());
        let spec = BuildSpec {
            context: BuildContext::Dockerfile(
                b"FROM alpine:3.20\nRUN echo built-ok-$((6*7))\n".to_vec(),
            ),
            tag: Some(tag.clone()),
            dockerfile: None,
        };
        let items: Vec<_> = d.build_image(spec).map(|r| r.unwrap()).collect().await;
        let text: String = items.iter().filter_map(|v| v["stream"].as_str()).collect();
        assert!(text.contains("built-ok-42"), "{text}");
        let img = d.inspect_image(&tag).await.unwrap();
        d.remove_image(img["Id"].as_str().unwrap(), true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn failing_build_step_is_an_error() {
        let d = docker().await;
        let spec = BuildSpec {
            context: BuildContext::Dockerfile(b"FROM alpine:3.20\nRUN exit 7\n".to_vec()),
            tag: None,
            dockerfile: None,
        };
        let items: Vec<_> = d.build_image(spec).collect().await;
        assert!(
            matches!(items.last(), Some(Err(Error::Upstream { .. }))),
            "{items:?}"
        );
    }
}
