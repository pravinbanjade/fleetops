use fleetops_core::docker::EngineVersion;
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion};
use fleetops_core::kube::KubeVersion;
use serde_json::json;

#[test]
fn kube_kinds_parse_and_print() {
    for (k, s) in [
        (EnvKind::DockerLocal, "docker-local"),
        (EnvKind::DockerAgent, "docker-agent"),
        (EnvKind::KubeConfig, "kube-config"),
        (EnvKind::KubeAgent, "kube-agent"),
    ] {
        assert_eq!(k.as_str(), s);
        assert_eq!(s.parse::<EnvKind>().unwrap(), k);
        assert_eq!(k.is_kube(), s.starts_with("kube-"));
    }
}

#[test]
fn an_up_status_tags_its_version() {
    let docker = EnvStatus::Up {
        version: EnvVersion::Docker(EngineVersion {
            version: "27.0".into(),
            api_version: "1.47".into(),
            os: "linux".into(),
            arch: "amd64".into(),
        }),
    };
    assert_eq!(
        serde_json::to_value(&docker).unwrap(),
        json!({"state": "up", "version": {"kind": "docker", "version": "27.0", "api_version": "1.47", "os": "linux", "arch": "amd64"}})
    );
    let kube = EnvStatus::Up {
        version: EnvVersion::Kube(KubeVersion {
            git_version: "v1.32.0".into(),
            platform: "linux/amd64".into(),
            nodes: Some(3),
            nodes_ready: Some(2),
        }),
    };
    assert_eq!(
        serde_json::to_value(&kube).unwrap(),
        json!({"state": "up", "version": {"kind": "kube", "git_version": "v1.32.0", "platform": "linux/amd64", "nodes": 3, "nodes_ready": 2}})
    );
}
