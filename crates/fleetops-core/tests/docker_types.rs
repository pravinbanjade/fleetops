use fleetops_core::docker::{ContainerAction, ContainerSpec, EngineVersion, PortBinding};
use fleetops_core::env::{validate_env_name, validate_env_url, EnvKind, EnvStatus, EnvVersion};
use fleetops_core::Error;
use serde_json::json;

#[test]
fn spec_deserializes_with_defaults() {
    let spec: ContainerSpec = serde_json::from_value(json!({
        "image": "nginx:1.27",
        "ports": [{"container_port": 80, "host_port": 8080}]
    }))
    .unwrap();
    assert_eq!(spec.image, "nginx:1.27");
    assert_eq!(spec.name, None);
    assert!(spec.env.is_empty());
    assert_eq!(
        spec.ports,
        vec![PortBinding {
            container_port: 80,
            protocol: "tcp".into(),
            host_ip: None,
            host_port: Some(8080)
        }]
    );
    assert!(!spec.privileged);
}

#[test]
fn spec_validation() {
    let ok = ContainerSpec {
        image: "alpine".into(),
        ..Default::default()
    };
    assert!(ok.validate().is_ok());
    let cases = [
        ContainerSpec {
            image: " ".into(),
            ..Default::default()
        },
        ContainerSpec {
            name: Some("-bad".into()),
            ..ok.clone()
        },
        ContainerSpec {
            name: Some("has space".into()),
            ..ok.clone()
        },
        ContainerSpec {
            restart_policy: Some("sometimes".into()),
            ..ok.clone()
        },
        ContainerSpec {
            advanced: Some(json!([1])),
            ..ok.clone()
        },
        ContainerSpec {
            ports: vec![PortBinding {
                container_port: 80,
                protocol: "icmp".into(),
                host_ip: None,
                host_port: None,
            }],
            ..ok.clone()
        },
        ContainerSpec {
            ports: vec![PortBinding {
                container_port: 0,
                protocol: "tcp".into(),
                host_ip: None,
                host_port: None,
            }],
            ..ok.clone()
        },
    ];
    for c in cases {
        assert!(matches!(c.validate(), Err(Error::Invalid(_))), "{c:?}");
    }
    let named = ContainerSpec {
        name: Some("web_1.a-b".into()),
        restart_policy: Some("unless-stopped".into()),
        ..ok
    };
    assert!(named.validate().is_ok());
}

#[test]
fn container_action_parse() {
    for a in ["start", "stop", "restart", "kill", "pause", "unpause"] {
        assert_eq!(a.parse::<ContainerAction>().unwrap().as_str(), a);
    }
    assert!(matches!(
        "explode".parse::<ContainerAction>(),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn env_kind_and_status_serialization() {
    assert_eq!(
        "docker-local".parse::<EnvKind>().unwrap(),
        EnvKind::DockerLocal
    );
    assert!("k8s".parse::<EnvKind>().is_err());
    let up = EnvStatus::Up {
        version: EnvVersion::Docker(EngineVersion {
            version: "29.0".into(),
            api_version: "1.54".into(),
            os: "linux".into(),
            arch: "arm64".into(),
        }),
    };
    assert_eq!(serde_json::to_value(&up).unwrap()["state"], "up");
    assert_eq!(
        serde_json::to_value(&up).unwrap()["version"]["kind"],
        "docker"
    );
    assert_eq!(
        serde_json::to_value(&up).unwrap()["version"]["api_version"],
        "1.54"
    );
    let down = EnvStatus::Down {
        reason: "socket missing".into(),
    };
    assert_eq!(
        serde_json::to_value(&down).unwrap(),
        json!({"state": "down", "reason": "socket missing"})
    );
}

#[test]
fn env_name_and_url_rules() {
    assert!(validate_env_name("local").is_ok());
    assert!(validate_env_name("prod-eu.1").is_ok());
    assert!(validate_env_name("").is_err());
    assert!(validate_env_name("a/b").is_err());
    assert!(validate_env_name(&"x".repeat(65)).is_err());
    for ok in [
        "unix:///var/run/docker.sock",
        "tcp://10.0.0.5:2375",
        "http://h:2375",
        "https://h:2376",
    ] {
        assert!(validate_env_url(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "unix://",
        "/var/run/docker.sock",
        "ssh://host",
        "ftp://x",
    ] {
        assert!(validate_env_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn image_references_split_into_repo_and_tag() {
    use fleetops_core::docker::split_image_reference as split;
    let ok = |r: &str, repo: &str, tag: &str| {
        assert_eq!(
            split(r).unwrap(),
            (repo.to_string(), tag.to_string()),
            "{r}"
        )
    };
    ok("nginx", "nginx", "latest");
    ok("nginx:1.27", "nginx", "1.27");
    ok(" alpine:3.20 ", "alpine", "3.20");
    ok("ghcr.io/org/app:2", "ghcr.io/org/app", "2");
    ok("registry:5000/team/app", "registry:5000/team/app", "latest");
    ok("registry:5000/team/app:v1", "registry:5000/team/app", "v1");
    ok("alpine@sha256:abc", "alpine@sha256:abc", "");
    for bad in ["", "  ", "nginx:", ":1", "has space:1", "a\nb"] {
        assert!(matches!(split(bad), Err(Error::Invalid(_))), "{bad:?}");
    }
}
