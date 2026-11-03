//! Trivy as a throwaway container on a Docker host or Swarm node.

use super::config::{resolve_socket, Effective, SocketChoice};
use super::inventory::{SCAN_LABEL, SCAN_NAME_PREFIX};
use super::model::ScanTarget;
use super::trivy::{self, ImageSource, REPORT_PATH};
use fleetops_core::docker::{split_image_reference, ContainerAction, DockerOps, VolumeSpec};
use fleetops_core::streams::{BoxStream, LogOptions};
use fleetops_core::{Error, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// The named volume that keeps Trivy's databases between scans.
pub const CACHE_VOLUME: &str = "fleetops-trivy-cache";
pub const NANO_CPUS: i64 = 1_000_000_000;
pub const MEMORY: i64 = 1 << 30;
/// A scan that runs longer than this is killed.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// The biggest report read back.
pub const REPORT_MAX: u64 = 64 << 20;
const POLL: Duration = Duration::from_secs(2);

pub fn container_name(t: &ScanTarget, scan_id: i64) -> String {
    let short: String = t
        .key
        .as_str()
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    format!("{SCAN_NAME_PREFIX}{short}-{scan_id}")
}

/// The `ContainerCreate` body of one scan.
pub fn container_body(cfg: &Effective, t: &ScanTarget, choice: &SocketChoice) -> Value {
    let src = match choice {
        SocketChoice::Mount(_) => ImageSource::Socket,
        SocketChoice::Export => ImageSource::Tar,
    };
    let mut env = trivy::auth_env(cfg);
    let mut mounts = vec![json!({"Type": "volume", "Source": CACHE_VOLUME, "Target": "/cache"})];
    if let SocketChoice::Mount(path) = choice {
        env.push("DOCKER_HOST=unix:///var/run/docker.sock".into());
        mounts.push(json!({"Type": "bind", "Source": path, "Target": "/var/run/docker.sock"}));
    }
    json!({
        "Image": cfg.trivy_image,
        "Cmd": trivy::image_args(&t.image_ref, cfg, src),
        "Env": env,
        "Labels": {SCAN_LABEL: "1", "fleetops.scan.key": t.key.as_str()},
        "HostConfig": {
            "Mounts": mounts,
            "NanoCpus": NANO_CPUS,
            "Memory": MEMORY,
            "AutoRemove": false,
        },
    })
}

async fn drain(mut s: BoxStream<Value>, what: &str) -> Result<()> {
    while let Some(m) = s.next().await {
        let m = m.map_err(|e| Error::Invalid(format!("{what}: {e}")))?;
        if let Some(e) = m["error"].as_str().or(m["errorDetail"]["message"].as_str()) {
            return Err(Error::Invalid(format!("{what}: {e}")));
        }
    }
    Ok(())
}

pub async fn prepare(d: &dyn DockerOps, cfg: &Effective) -> Result<()> {
    prepare_with_cache(d, cfg, CACHE_VOLUME).await
}

/// [`prepare`] with the database cache in `volume`.
pub async fn prepare_with_cache(d: &dyn DockerOps, cfg: &Effective, volume: &str) -> Result<()> {
    let (repo, tag) = split_image_reference(&cfg.trivy_image)?;
    let auth = cfg.auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
    drain(
        d.pull_image_auth(&repo, &tag, auth),
        &format!("pulling {}", cfg.trivy_image),
    )
    .await?;
    if d.inspect_volume(volume).await.is_err() {
        d.create_volume(&VolumeSpec {
            name: volume.into(),
            driver: None,
            driver_opts: BTreeMap::new(),
            labels: BTreeMap::from([(SCAN_LABEL.to_string(), "1".to_string())]),
        })
        .await?;
    }
    // Download the database now, so a mirror or network problem shows as
    // "scanner unavailable" rather than as one failed scan per image.
    let body = json!({
        "Image": cfg.trivy_image,
        "Cmd": trivy::db_args(cfg),
        "Env": trivy::auth_env(cfg),
        "Labels": {SCAN_LABEL: "1"},
        "HostConfig": {
            "Mounts": [{"Type": "volume", "Source": volume, "Target": "/cache"}],
            "NanoCpus": NANO_CPUS,
            "Memory": MEMORY,
        },
    });
    let name = format!("{SCAN_NAME_PREFIX}db-{:08x}", rand::random::<u32>());
    let id = d.create_container_raw(&name, &body).await?;
    let out = match start_and_wait(d, &id).await {
        Ok(0) => Ok(()),
        Ok(code) => Err(Error::Invalid(format!(
            "downloading the vulnerability database failed (trivy exited {code}): {}",
            log_tail(d, &id).await
        ))),
        Err(e) => Err(Error::Invalid(format!(
            "downloading the vulnerability database: {e}"
        ))),
    };
    let _ = d.remove_container(&id, true, false).await;
    out
}

fn missing_socket(e: &Error) -> bool {
    let m = e.to_string();
    m.contains("bind source path does not exist") || m.contains("no such file or directory")
}

pub async fn scan(
    d: &dyn DockerOps,
    t: &ScanTarget,
    cfg: &Effective,
    scan_id: i64,
    tmp_dir: &Path,
) -> Result<Vec<u8>> {
    let info = d.info().await?;
    let version = serde_json::to_value(d.version().await?).unwrap_or(Value::Null);
    let mut choice = resolve_socket(&info, &version, cfg.socket_path.as_deref());
    let name = container_name(t, scan_id);
    let id = match d
        .create_container_raw(&name, &container_body(cfg, t, &choice))
        .await
    {
        Ok(id) => id,
        Err(e) if matches!(choice, SocketChoice::Mount(_)) && missing_socket(&e) => {
            choice = SocketChoice::Export;
            d.create_container_raw(&name, &container_body(cfg, t, &choice))
                .await?
        }
        Err(e) => return Err(e),
    };
    let out = run(d, &id, t, &choice, tmp_dir).await;
    if let Err(e) = d.remove_container(&id, true, false).await {
        tracing::warn!(container = name, error = %e, "could not remove a scan container");
    }
    out
}

async fn run(
    d: &dyn DockerOps,
    id: &str,
    t: &ScanTarget,
    choice: &SocketChoice,
    tmp_dir: &Path,
) -> Result<Vec<u8>> {
    if *choice == SocketChoice::Export {
        upload_export(d, id, &t.image_ref, tmp_dir).await?;
    }
    let code = start_and_wait(d, id).await?;
    if code != 0 {
        return Err(Error::Internal(format!(
            "trivy exited {code}: {}",
            log_tail(d, id).await
        )));
    }
    d.read_file(id, REPORT_PATH, REPORT_MAX).await
}

/// Starts `id` and waits for it to exit; its exit code.
async fn start_and_wait(d: &dyn DockerOps, id: &str) -> Result<i64> {
    d.container_action(id, ContainerAction::Start).await?;
    let started = std::time::Instant::now();
    loop {
        let v = d.inspect_container(id).await?;
        if v["State"]["Running"] != true && v["State"]["Status"] != "created" {
            return Ok(v["State"]["ExitCode"].as_i64().unwrap_or(-1));
        }
        if started.elapsed() > SCAN_TIMEOUT {
            let _ = d.container_action(id, ContainerAction::Kill).await;
            return Err(Error::Internal(format!(
                "trivy did not finish within {} minutes",
                SCAN_TIMEOUT.as_secs() / 60
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn log_tail(d: &dyn DockerOps, id: &str) -> String {
    let opts = LogOptions {
        follow: false,
        tail: Some(400),
        stdout: true,
        stderr: true,
        ..Default::default()
    };
    let lines: Vec<String> = d
        .logs(id, &opts)
        .filter_map(|l| async move { l.ok().map(|l| l.text) })
        .collect()
        .await;
    error_lines(&lines)
}

/// What explains a failed run: Trivy's FATAL/ERROR lines or a Go panic's
/// first lines when there are any (a goroutine dump's tail says nothing),
/// else the last 20 lines.
pub fn error_lines(lines: &[String]) -> String {
    let key: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| {
            l.contains("FATAL")
                || l.contains("ERROR")
                || l.starts_with("panic:")
                || l.starts_with("fatal error:")
        })
        .take(5)
        .collect();
    if !key.is_empty() {
        return key.join("\n");
    }
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// Exports the image to a file under `tmp_dir`, then streams it into the
/// container as `/in/image.tar`; the file is removed either way.
async fn upload_export(d: &dyn DockerOps, id: &str, image: &str, tmp_dir: &Path) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    tokio::fs::create_dir_all(tmp_dir)
        .await
        .map_err(|e| Error::Internal(format!("scan spool: {e}")))?;
    let path = tmp_dir.join(format!(
        "scan-{}.tar",
        id.chars().take(12).collect::<String>()
    ));
    let result = async {
        let mut f = tokio::fs::File::create(&path)
            .await
            .map_err(|e| Error::Internal(format!("scan spool: {e}")))?;
        let mut size = 0u64;
        let mut s = d.export_image(image);
        while let Some(chunk) = s.next().await {
            let chunk = chunk?;
            size += chunk.len() as u64;
            f.write_all(&chunk)
                .await
                .map_err(|e| Error::Internal(format!("scan spool: {e}")))?;
        }
        f.flush()
            .await
            .map_err(|e| Error::Internal(format!("scan spool: {e}")))?;
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| Error::Internal(format!("scan spool: {e}")))?;
        d.put_archive_stream(id, "/", tar_stream(file, size)).await
    }
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

/// A one-file tar (`in/image.tar`) streamed from `file` of `size` bytes.
fn tar_stream(file: tokio::fs::File, size: u64) -> BoxStream<Vec<u8>> {
    let mut h = tar::Header::new_gnu();
    // Infallible for this short relative path.
    let _ = h.set_path("in/image.tar");
    h.set_size(size);
    h.set_mode(0o644);
    h.set_cksum();
    let header = h.as_bytes().to_vec();
    let pad = ((512 - size % 512) % 512) as usize + 1024;
    let body = tokio_util::io::ReaderStream::new(file).map(|r| {
        r.map(|b| b.to_vec())
            .map_err(|e| Error::Internal(format!("scan spool: {e}")))
    });
    Box::pin(
        futures_util::stream::once(async move { Ok(header) })
            .chain(body)
            .chain(futures_util::stream::once(
                async move { Ok(vec![0u8; pad]) },
            )),
    )
}

pub async fn cleanup_orphans(d: &dyn DockerOps) -> Result<()> {
    for c in d.list_containers().await? {
        if c.labels.contains_key(SCAN_LABEL) {
            d.remove_container(&c.id, true, false).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::config::{effective, GlobalConfig};
    use crate::scan::model::ImageKey;

    fn target() -> ScanTarget {
        ScanTarget {
            env_id: 1,
            node: String::new(),
            key: ImageKey("sha256:0123456789abcdef0123".into()),
            image_ref: "nginx:1.25".into(),
            namespace: String::new(),
            pull_secrets: vec![],
        }
    }

    fn cfg() -> Effective {
        effective(&GlobalConfig::default(), None, None, None)
    }

    #[test]
    fn container_body_mounts_socket_and_cache() {
        let b = container_body(
            &cfg(),
            &target(),
            &SocketChoice::Mount("/var/run/docker.sock".into()),
        );
        let mounts = b["HostConfig"]["Mounts"].as_array().unwrap();
        assert_eq!(
            mounts[0],
            json!({"Type": "volume", "Source": CACHE_VOLUME, "Target": "/cache"})
        );
        assert_eq!(
            mounts[1],
            json!({"Type": "bind", "Source": "/var/run/docker.sock", "Target": "/var/run/docker.sock"})
        );
        assert!(b["Env"]
            .as_array()
            .unwrap()
            .contains(&json!("DOCKER_HOST=unix:///var/run/docker.sock")));
        let cmd: Vec<&str> = b["Cmd"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(cmd.last(), Some(&"nginx:1.25"));
        assert!(cmd.windows(2).any(|w| w == ["--image-src", "docker"]));
    }

    #[test]
    fn container_body_export_has_no_socket() {
        let b = container_body(&cfg(), &target(), &SocketChoice::Export);
        let mounts = b["HostConfig"]["Mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 1, "only the cache");
        assert!(!b.to_string().contains("docker.sock"));
        let cmd: Vec<&str> = b["Cmd"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(cmd.windows(2).any(|w| w == ["--input", trivy::INPUT_PATH]));
    }

    #[test]
    fn container_body_sets_limits_and_labels() {
        let b = container_body(&cfg(), &target(), &SocketChoice::Export);
        assert_eq!(b["Image"], crate::scan::config::DEFAULT_TRIVY_IMAGE);
        assert_eq!(b["HostConfig"]["NanoCpus"], 1_000_000_000i64);
        assert_eq!(b["HostConfig"]["Memory"], 1_073_741_824i64);
        assert_eq!(b["Labels"][SCAN_LABEL], "1");
        assert_eq!(
            b["Labels"]["fleetops.scan.key"],
            "sha256:0123456789abcdef0123"
        );
        assert_eq!(container_name(&target(), 7), "fleetops-scan-0123456789ab-7");
    }

    #[test]
    fn error_lines_prefer_fatal_over_a_goroutine_dump() {
        let mut lines: Vec<String> = vec![
            "2026 INFO scanning".into(),
            "2026 FATAL Fatal error: image scan failed: unable to open tar".into(),
        ];
        lines.extend((0..50).map(|i| format!("\tgoroutine frame {i}")));
        assert_eq!(
            error_lines(&lines),
            "2026 FATAL Fatal error: image scan failed: unable to open tar"
        );
        let plain: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        assert!(error_lines(&plain).starts_with("line 10"));
    }

    #[test]
    fn container_body_passes_mirror_credentials() {
        let mut c = cfg();
        c.auth = Some(("bot".into(), "pw".into()));
        let b = container_body(&c, &target(), &SocketChoice::Export);
        let env = b["Env"].as_array().unwrap();
        assert!(
            env.contains(&json!("TRIVY_USERNAME=bot")) && env.contains(&json!("TRIVY_PASSWORD=pw"))
        );
    }
}
