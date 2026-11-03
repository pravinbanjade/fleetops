//! Trivy as a Job in the `fleetops-scan` namespace, one per scan.

use super::config::Effective;
use super::inventory::{SCAN_LABEL, SCAN_NAMESPACE as NAMESPACE};
use super::model::ScanTarget;
use super::trivy::{self, ImageSource};
use base64::Engine as _;
use fleetops_core::kube::{ApplyMode, KubeKind, KubeOps};
use fleetops_core::streams::LogOptions;
use fleetops_core::{now_unix, Error, Result};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use std::time::Duration;

pub const CACHE_PVC: &str = "fleetops-trivy-cache";
pub const MIRROR_SECRET: &str = "fleetops-scan-mirror";
/// The Job's own deadline; FleetOps waits a minute longer.
pub const DEADLINE_SECS: i64 = 1200;
const POLL: Duration = Duration::from_secs(3);

pub fn job_name(scan_id: i64) -> String {
    format!("fleetops-scan-{scan_id}")
}

fn pull_secret_name(scan_id: i64) -> String {
    format!("fleetops-scan-{scan_id}-pull")
}

/// `repo@digest` when the key is a digest, so the scan reads exactly the
/// image the pods run; else the reference as written.
pub fn pinned_ref(t: &ScanTarget) -> String {
    let Some(d) = t.key.digest() else {
        return t.image_ref.clone();
    };
    let r = t.image_ref.split('@').next().unwrap_or(&t.image_ref);
    let slash = r.rfind('/').map_or(0, |i| i + 1);
    let repo = match r[slash..].rfind(':') {
        Some(i) => &r[..slash + i],
        None => r,
    };
    format!("{repo}@{d}")
}

fn labels() -> Value {
    json!({ SCAN_LABEL: "1" })
}

/// The Job, as JSON (which `apply` reads as YAML).
pub fn job_manifest(
    cfg: &Effective,
    t: &ScanTarget,
    scan_id: i64,
    pull_secret: Option<&str>,
    mirror_secret: Option<&str>,
) -> String {
    let mut env = vec![json!({"name": "DOCKER_CONFIG", "value": "/docker"})];
    for kv in trivy::auth_env(cfg) {
        if let Some((k, v)) = kv.split_once('=') {
            env.push(json!({"name": k, "value": v}));
        }
    }
    let mut mounts = vec![json!({"name": "cache", "mountPath": "/cache"})];
    let mut volumes =
        vec![json!({"name": "cache", "persistentVolumeClaim": {"claimName": CACHE_PVC}})];
    if let Some(s) = pull_secret {
        mounts.push(json!({"name": "pull", "mountPath": "/docker", "readOnly": true}));
        volumes.push(json!({"name": "pull", "secret": {"secretName": s,
            "items": [{"key": ".dockerconfigjson", "path": "config.json"}]}}));
    }
    let mut pod = json!({
        "restartPolicy": "Never",
        "containers": [{
            "name": "trivy",
            "image": cfg.trivy_image,
            "args": trivy::image_args(&pinned_ref(t), cfg, ImageSource::Remote),
            "env": env,
            "resources": {
                "limits": {"cpu": "1", "memory": "1Gi"},
                "requests": {"cpu": "100m", "memory": "256Mi"},
            },
            "volumeMounts": mounts,
        }],
        "volumes": volumes,
    });
    if let Some(m) = mirror_secret {
        pod["imagePullSecrets"] = json!([{ "name": m }]);
    }
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": job_name(scan_id), "namespace": NAMESPACE, "labels": labels()},
        "spec": {
            "backoffLimit": 0,
            "ttlSecondsAfterFinished": 300,
            "activeDeadlineSeconds": DEADLINE_SECS,
            "template": {"metadata": {"labels": labels()}, "spec": pod},
        },
    })
    .to_string()
}

/// The registry host of an image reference, as `config.json` keys it.
pub fn registry_of(image: &str) -> String {
    match image.split_once('/') {
        Some((host, _)) if host.contains('.') || host.contains(':') || host == "localhost" => {
            host.to_string()
        }
        _ => "https://index.docker.io/v1/".into(),
    }
}

fn dockerconfig_secret(name: &str, auths: &Map<String, Value>) -> String {
    let cfg = json!({ "auths": auths }).to_string();
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": "kubernetes.io/dockerconfigjson",
        "metadata": {"name": name, "namespace": NAMESPACE, "labels": labels()},
        "data": {".dockerconfigjson": base64::engine::general_purpose::STANDARD.encode(cfg)},
    })
    .to_string()
}

/// The mirror credentials as a pull secret for the Trivy image.
pub fn mirror_secret(cfg: &Effective) -> Option<String> {
    let (u, p) = cfg.auth.as_ref()?;
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
    let mut auths = Map::new();
    auths.insert(registry_of(&cfg.trivy_image), json!({ "auth": token }));
    Some(dockerconfig_secret(MIRROR_SECRET, &auths))
}

/// Every `auths` entry of the given `.dockerconfigjson` payloads, merged.
pub fn merge_auths(configs: &[Vec<u8>]) -> Map<String, Value> {
    let mut out = Map::new();
    for c in configs {
        if let Ok(v) = serde_json::from_slice::<Value>(c) {
            if let Some(a) = v["auths"].as_object() {
                for (k, v) in a {
                    out.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }
    out
}

/// A permission refusal, said as what FleetOps' identity is missing.
fn rbac(e: Error, verb: &str, what: &str) -> Error {
    match e {
        Error::Forbidden(m) => Error::Forbidden(format!(
            "FleetOps needs {verb} on {what} in namespace {NAMESPACE}: {m}"
        )),
        e => e,
    }
}

async fn apply(k: &dyn KubeOps, docs: &str, verb: &str, what: &str) -> Result<()> {
    let r = k
        .apply(docs, NAMESPACE, ApplyMode::Commit { force: true }, &|_| {
            Ok(())
        })
        .await
        .map_err(|e| rbac(e, verb, what))?;
    match r.into_iter().find_map(|r| r.error) {
        None => Ok(()),
        Some(e) if e.code == "forbidden" => Err(rbac(Error::Forbidden(e.message), verb, what)),
        Some(e) => Err(Error::Invalid(e.message)),
    }
}

pub async fn prepare(k: &dyn KubeOps, cfg: &Effective) -> Result<()> {
    let mut docs = vec![
        json!({"apiVersion": "v1", "kind": "Namespace",
            "metadata": {"name": NAMESPACE, "labels": labels()}})
        .to_string(),
        json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim",
            "metadata": {"name": CACHE_PVC, "namespace": NAMESPACE, "labels": labels()},
            "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "2Gi"}}}})
        .to_string(),
    ];
    docs.extend(mirror_secret(cfg));
    apply(
        k,
        &docs.join("\n---\n"),
        "create",
        "namespaces, persistentvolumeclaims and secrets",
    )
    .await
}

pub async fn scan(
    k: &dyn KubeOps,
    t: &ScanTarget,
    cfg: &Effective,
    scan_id: i64,
) -> Result<Vec<u8>> {
    let pull = pull_secret_name(scan_id);
    let job = job_name(scan_id);
    let out = run(k, t, cfg, scan_id, &pull, &job).await;
    if let Err(e) = k.delete(KubeKind::Jobs, NAMESPACE, &job).await {
        tracing::info!(job, error = %e, "could not delete a scan job; its TTL will");
    }
    let _ = k.delete(KubeKind::Secrets, NAMESPACE, &pull).await;
    out
}

async fn run(
    k: &dyn KubeOps,
    t: &ScanTarget,
    cfg: &Effective,
    scan_id: i64,
    pull: &str,
    job: &str,
) -> Result<Vec<u8>> {
    let mut configs = Vec::new();
    for name in &t.pull_secrets {
        match k.secret_values(&t.namespace, name).await {
            Ok(vals) => configs.extend(
                vals.into_iter()
                    .filter(|(key, _)| key == ".dockerconfigjson")
                    .map(|(_, v)| v),
            ),
            Err(Error::Forbidden(m)) => {
                return Err(Error::Forbidden(format!(
                    "FleetOps needs get on secrets in namespace {}: {m}",
                    t.namespace
                )))
            }
            Err(e) => tracing::info!(secret = name, error = %e, "pull secret unreadable"),
        }
    }
    let auths = merge_auths(&configs);
    let mut docs = Vec::new();
    if !auths.is_empty() {
        docs.push(dockerconfig_secret(pull, &auths));
    }
    let mirror = cfg.auth.is_some().then_some(MIRROR_SECRET);
    docs.push(job_manifest(
        cfg,
        t,
        scan_id,
        (!auths.is_empty()).then_some(pull),
        mirror,
    ));
    apply(k, &docs.join("\n---\n"), "create", "jobs and secrets").await?;

    let until = now_unix() + DEADLINE_SECS + 60;
    let failed = loop {
        let o = k.get(KubeKind::Jobs, NAMESPACE, job).await?;
        let done = |ty: &str| {
            o.conditions
                .iter()
                .any(|c| c.type_ == ty && c.status == "True")
        };
        if done("Complete") {
            break false;
        }
        if done("Failed") {
            break true;
        }
        if now_unix() > until {
            return Err(Error::Internal(
                "the scan job did not finish in time".into(),
            ));
        }
        tokio::time::sleep(POLL).await;
    };
    let pods = k.workload_pods(KubeKind::Jobs, NAMESPACE, job).await?;
    let pod = pods
        .rows
        .first()
        .map(|r| r.name.clone())
        .ok_or_else(|| Error::Internal("the scan job left no pod".into()))?;
    let opts = LogOptions {
        follow: false,
        stdout: true,
        stderr: true,
        ..Default::default()
    };
    let lines: Vec<String> = k
        .logs(NAMESPACE, &pod, &opts)
        .filter_map(|l| async move { l.ok().map(|l| l.text) })
        .collect()
        .await;
    if failed {
        let tail = lines[lines.len().saturating_sub(20)..].join("\n");
        return Err(Error::Internal(format!("trivy failed: {tail}")));
    }
    Ok(lines.join("\n").into_bytes())
}

/// Deletes scan Jobs and Secrets older than half an hour.
pub async fn cleanup_orphans(k: &dyn KubeOps) -> Result<()> {
    let old = now_unix() - 1800;
    for kind in [KubeKind::Jobs, KubeKind::Secrets] {
        let list = match k.list(kind, Some(NAMESPACE)).await {
            Ok(l) => l,
            Err(Error::NotFound(_)) => return Ok(()),
            Err(e) => return Err(e),
        };
        for r in list.rows {
            let ours = r.name.starts_with("fleetops-scan-") && r.name != MIRROR_SECRET;
            if ours && r.created < old {
                let _ = k.delete(kind, NAMESPACE, &r.name).await;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::config::{effective, GlobalConfig};
    use crate::scan::model::ImageKey;

    fn target(key: &str, image: &str) -> ScanTarget {
        ScanTarget {
            env_id: 1,
            node: String::new(),
            key: ImageKey(key.into()),
            image_ref: image.into(),
            namespace: "web".into(),
            pull_secrets: vec!["reg".into()],
        }
    }

    fn cfg() -> Effective {
        effective(&GlobalConfig::default(), None, None, None)
    }

    fn job(pull: Option<&str>, mirror: Option<&str>) -> Value {
        serde_json::from_str(&job_manifest(
            &cfg(),
            &target("sha256:d", "nginx:1.25"),
            9,
            pull,
            mirror,
        ))
        .unwrap()
    }

    #[test]
    fn job_has_limits_ttl_and_deadline() {
        let j = job(None, None);
        assert_eq!(j["metadata"]["name"], "fleetops-scan-9");
        assert_eq!(j["metadata"]["namespace"], NAMESPACE);
        assert_eq!(j["spec"]["backoffLimit"], 0);
        assert_eq!(j["spec"]["ttlSecondsAfterFinished"], 300);
        assert_eq!(j["spec"]["activeDeadlineSeconds"], 1200);
        let c = &j["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(
            c["resources"]["limits"],
            json!({"cpu": "1", "memory": "1Gi"})
        );
        assert_eq!(j["spec"]["template"]["spec"]["restartPolicy"], "Never");
        let args: Vec<&str> = c["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a.as_str())
            .collect();
        assert_eq!(args.last(), Some(&"nginx@sha256:d"), "pinned to the digest");
        assert!(!args.contains(&"--output"), "the report goes to the log");
    }

    #[test]
    fn job_mounts_pull_secret_only_when_given() {
        let without = job(None, None);
        assert_eq!(
            without["spec"]["template"]["spec"]["volumes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let with = job(Some("fleetops-scan-9-pull"), None);
        let vols = with["spec"]["template"]["spec"]["volumes"]
            .as_array()
            .unwrap();
        assert_eq!(vols[1]["secret"]["secretName"], "fleetops-scan-9-pull");
        assert_eq!(vols[1]["secret"]["items"][0]["path"], "config.json");
    }

    #[test]
    fn job_uses_mirror_pull_secret() {
        let j = job(None, Some(MIRROR_SECRET));
        assert_eq!(
            j["spec"]["template"]["spec"]["imagePullSecrets"],
            json!([{"name": MIRROR_SECRET}])
        );
        assert!(job(None, None)["spec"]["template"]["spec"]["imagePullSecrets"].is_null());
    }

    #[test]
    fn pinned_ref_keeps_registry_port_and_drops_tag() {
        assert_eq!(
            pinned_ref(&target("sha256:d", "reg.local:5000/team/app:1.2")),
            "reg.local:5000/team/app@sha256:d"
        );
        assert_eq!(pinned_ref(&target("ref:1:app:1", "app:1")), "app:1");
    }

    #[test]
    fn mirror_secret_keys_the_trivy_registry() {
        let mut c = cfg();
        assert_eq!(mirror_secret(&c), None);
        c.trivy_image = "mirror.local:5000/aquasec/trivy:0.76.1".into();
        c.auth = Some(("bot".into(), "pw".into()));
        let s: Value = serde_json::from_str(&mirror_secret(&c).unwrap()).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(s["data"][".dockerconfigjson"].as_str().unwrap())
            .unwrap();
        let cfgjson: Value = serde_json::from_slice(&raw).unwrap();
        assert!(cfgjson["auths"]["mirror.local:5000"]["auth"].is_string());
        assert_eq!(
            registry_of("aquasec/trivy:1"),
            "https://index.docker.io/v1/"
        );
    }

    #[test]
    fn merge_auths_combines_secrets_first_wins() {
        let a = br#"{"auths": {"r1": {"auth": "a"}}}"#.to_vec();
        let b = br#"{"auths": {"r1": {"auth": "b"}, "r2": {"auth": "c"}}}"#.to_vec();
        let m = merge_auths(&[a, b, b"not json".to_vec()]);
        assert_eq!(m["r1"]["auth"], "a");
        assert_eq!(m["r2"]["auth"], "c");
    }
}
