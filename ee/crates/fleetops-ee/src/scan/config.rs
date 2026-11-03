//! Scanning settings: the global switch and mirror, per-environment
//! overrides, and the settings one scan actually runs with.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Trivy, pinned by digest; bumped deliberately, never `latest`.
pub const DEFAULT_TRIVY_IMAGE: &str =
    "aquasec/trivy:0.76.1@sha256:790fd66d1697395d8693d1acaa1ea702406a20f109b6fc7bac8db623eca6c06c";

/// Where Trivy and its databases come from, for air-gapped environments.
/// `None` fields use Trivy's public locations. The password is stored
/// sealed, apart from this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Mirror {
    pub trivy_image: Option<String>,
    pub db_repository: Option<String>,
    pub java_db_repository: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub enabled: bool,
    pub mirror: Option<Mirror>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvOverride {
    /// `None` inherits the global switch.
    pub enabled: Option<bool>,
    /// Replaces the global mirror as a whole when set.
    pub mirror: Option<Mirror>,
    /// The engine socket's path on the host; `None` detects it.
    pub socket_path: Option<String>,
}

/// What one environment's scans run with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub enabled: bool,
    pub trivy_image: String,
    pub db_repository: Option<String>,
    pub java_db_repository: Option<String>,
    /// Mirror username and password, when both are set.
    pub auth: Option<(String, String)>,
    pub socket_path: Option<String>,
}

pub fn effective(
    g: &GlobalConfig,
    g_pw: Option<&str>,
    o: Option<&EnvOverride>,
    o_pw: Option<&str>,
) -> Effective {
    let (mirror, pw) = match o.and_then(|o| o.mirror.as_ref()) {
        Some(m) => (Some(m), o_pw),
        None => (g.mirror.as_ref(), g_pw),
    };
    let m = mirror.cloned().unwrap_or_default();
    Effective {
        enabled: o.and_then(|o| o.enabled).unwrap_or(g.enabled),
        trivy_image: m
            .trivy_image
            .unwrap_or_else(|| DEFAULT_TRIVY_IMAGE.to_string()),
        db_repository: m.db_repository,
        java_db_repository: m.java_db_repository,
        auth: m.username.zip(pw.map(str::to_string)),
        socket_path: o.and_then(|o| o.socket_path.clone()),
    }
}

/// How a Docker-host scan reaches the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketChoice {
    /// Bind-mount the engine socket at this host path.
    Mount(String),
    /// Export the image and give Trivy the tar.
    Export,
}

pub const DOCKER_SOCKET: &str = "/var/run/docker.sock";
pub const PODMAN_SOCKET: &str = "/run/podman/podman.sock";

/// Picks the socket from the engine's `/info` and `/version`: an override
/// wins; rootless engines keep their socket somewhere we can't know, so they
/// export.
pub fn resolve_socket(info: &Value, version: &Value, override_path: Option<&str>) -> SocketChoice {
    if let Some(p) = override_path {
        return SocketChoice::Mount(p.to_string());
    }
    let rootless = info["SecurityOptions"].as_array().is_some_and(|o| {
        o.iter()
            .any(|s| s.as_str().is_some_and(|s| s.contains("name=rootless")))
    }) || info["DockerRootDir"]
        .as_str()
        .is_some_and(|d| d.starts_with("/home/"));
    // Podman's compat `/info` keeps its own storage path; `/version` may name it.
    let podman = version["Components"].as_array().is_some_and(|c| {
        c.iter()
            .any(|c| c["Name"].as_str().is_some_and(|n| n.contains("Podman")))
    }) || info["DockerRootDir"]
        .as_str()
        .is_some_and(|d| d.contains("/containers/storage"));
    match (rootless, podman) {
        (true, _) => SocketChoice::Export,
        (false, true) => SocketChoice::Mount(PODMAN_SOCKET.into()),
        (false, false) => SocketChoice::Mount(DOCKER_SOCKET.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mirror(image: &str, user: Option<&str>) -> Mirror {
        Mirror {
            trivy_image: Some(image.into()),
            db_repository: Some(format!("{image}-db")),
            java_db_repository: None,
            username: user.map(Into::into),
        }
    }

    #[test]
    fn effective_defaults_to_pinned_image() {
        let e = effective(&GlobalConfig::default(), None, None, None);
        assert!(!e.enabled, "off by default");
        assert_eq!(e.trivy_image, DEFAULT_TRIVY_IMAGE);
        assert!(e.trivy_image.contains("@sha256:"));
        assert_eq!(e.auth, None);
    }

    #[test]
    fn inherit_takes_global_enabled() {
        let g = GlobalConfig {
            enabled: true,
            mirror: None,
        };
        assert!(effective(&g, None, Some(&EnvOverride::default()), None).enabled);
        let off = EnvOverride {
            enabled: Some(false),
            ..Default::default()
        };
        assert!(!effective(&g, None, Some(&off), None).enabled);
    }

    #[test]
    fn effective_env_mirror_replaces_global() {
        let g = GlobalConfig {
            enabled: true,
            mirror: Some(mirror("global/trivy:1", Some("g"))),
        };
        let o = EnvOverride {
            mirror: Some(mirror("local/trivy:1", None)),
            ..Default::default()
        };
        let e = effective(&g, Some("gpw"), Some(&o), None);
        assert_eq!(e.trivy_image, "local/trivy:1");
        assert_eq!(e.db_repository.as_deref(), Some("local/trivy:1-db"));
        assert_eq!(
            e.auth, None,
            "the global credentials do not leak into the env mirror"
        );
        let g_only = effective(&g, Some("gpw"), None, None);
        assert_eq!(g_only.auth, Some(("g".into(), "gpw".into())));
    }

    #[test]
    fn socket_override_wins() {
        let info = json!({"SecurityOptions": ["name=rootless"]});
        assert_eq!(
            resolve_socket(&info, &json!({}), Some("/run/user/1000/docker.sock")),
            SocketChoice::Mount("/run/user/1000/docker.sock".into())
        );
    }

    #[test]
    fn rootless_docker_exports() {
        let info = json!({"SecurityOptions": ["name=seccomp,profile=builtin", "name=rootless"]});
        assert_eq!(
            resolve_socket(&info, &json!({}), None),
            SocketChoice::Export
        );
    }

    #[test]
    fn rootful_podman_mounts_podman_socket() {
        let version = json!({"Components": [{"Name": "Podman Engine", "Version": "5.2"}]});
        assert_eq!(
            resolve_socket(
                &json!({"DockerRootDir": "/var/lib/containers/storage"}),
                &version,
                None
            ),
            SocketChoice::Mount(PODMAN_SOCKET.into())
        );
        let rootless = json!({"DockerRootDir": "/home/me/.local/share/containers/storage"});
        assert_eq!(
            resolve_socket(&rootless, &version, None),
            SocketChoice::Export
        );
    }

    #[test]
    fn plain_docker_mounts_default() {
        let info = json!({"SecurityOptions": ["name=seccomp,profile=builtin"], "DockerRootDir": "/var/lib/docker"});
        let version = json!({"Components": [{"Name": "Engine", "Version": "27.3"}]});
        assert_eq!(
            resolve_socket(&info, &version, None),
            SocketChoice::Mount(DOCKER_SOCKET.into())
        );
    }
}
