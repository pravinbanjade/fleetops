//! Trivy as a Job in the `fleetops-scan` namespace, one per scan.

use super::config::Effective;
use super::inventory::{SCAN_LABEL, SCAN_NAMESPACE as NAMESPACE};
use super::model::ScanTarget;
use super::trivy;
use base64::Engine as _;
use fleetops_core::kube::{ApplyMode, KubeKind, KubeOps};
use fleetops_core::streams::LogOptions;
use fleetops_core::{now_unix, Error, Result};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use std::time::Duration;

pub const CACHE_PVC: &str = "fleetops-trivy-cache";
pub const MIRROR_SECRET: &str = "fleetops-scan-mirror";
/// The mirror's username and password for Trivy itself, read by the Job
/// through `secretKeyRef` so they never appear in its spec.
pub const MIRROR_AUTH_SECRET: &str = "fleetops-scan-mirror-auth";
pub const PREPARE_JOB: &str = "fleetops-scan-prepare";
pub const PREPARE_JAVA_JOB: &str = "fleetops-scan-prepare-java";
/// Checks the Trivy image has the binary scans copy into the scanned image.
pub const PREPARE_COPY_JOB: &str = "fleetops-scan-prepare-copy";
/// Room for the vulnerability and Java databases, each over a gigabyte
/// unpacked, while Trivy replaces them.
pub const CACHE_SIZE: &str = "10Gi";
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

fn job_env(cfg: &Effective) -> Vec<Value> {
    let mut env = vec![];
    if cfg.auth.is_some() {
        for (name, key) in [
            ("TRIVY_USERNAME", "username"),
            ("TRIVY_PASSWORD", "password"),
        ] {
            env.push(json!({"name": name,
                "valueFrom": {"secretKeyRef": {"name": MIRROR_AUTH_SECRET, "key": key}}}));
        }
    }
    env
}

/// The mirror credentials Trivy reads, as an Opaque Secret.
pub fn mirror_auth_secret(cfg: &Effective) -> Option<String> {
    let (u, p) = cfg.auth.as_ref()?;
    let b = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    Some(
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "type": "Opaque",
            "metadata": {"name": MIRROR_AUTH_SECRET, "namespace": NAMESPACE, "labels": labels()},
            "data": {"username": b(u), "password": b(p)},
        })
        .to_string(),
    )
}

/// The download-only Jobs turning scanning on runs, in order: Job name and
/// Trivy arguments.
pub fn prepare_jobs(cfg: &Effective) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (PREPARE_JOB, trivy::db_args(cfg)),
        (PREPARE_JAVA_JOB, trivy::java_db_args(cfg)),
    ]
}

/// The database cache's claim.
pub fn cache_pvc() -> String {
    json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim",
        "metadata": {"name": CACHE_PVC, "namespace": NAMESPACE, "labels": labels()},
        "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": CACHE_SIZE}}}})
    .to_string()
}

/// Kubernetes refusing to grow a claim whose storage class can't expand.
pub fn refuses_resize(message: &str) -> bool {
    message.contains("resize")
}

/// A Job that only downloads a database.
pub fn prepare_manifest(
    cfg: &Effective,
    name: &str,
    args: &[String],
    mirror_secret: Option<&str>,
) -> String {
    let mut pod = json!({
        "restartPolicy": "Never",
        "containers": [{
            "name": "trivy",
            "image": cfg.trivy_image,
            "args": args,
            "env": job_env(cfg),
            "resources": {"limits": {"cpu": "1", "memory": "1Gi"}},
            "volumeMounts": [{"name": "cache", "mountPath": "/cache"}],
        }],
        "volumes": [{"name": "cache", "persistentVolumeClaim": {"claimName": CACHE_PVC}}],
    });
    if let Some(m) = mirror_secret {
        pod["imagePullSecrets"] = json!([{ "name": m }]);
    }
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": name, "namespace": NAMESPACE, "labels": labels()},
        "spec": {
            "backoffLimit": 0,
            "ttlSecondsAfterFinished": 300,
            "activeDeadlineSeconds": DEADLINE_SECS,
            "template": {"metadata": {"labels": labels()}, "spec": pod},
        },
    })
    .to_string()
}

/// The Job that copies the `trivy` binary the way every scan does, so a
/// Trivy image without it fails when scanning is turned on.
pub fn copy_check_manifest(cfg: &Effective, mirror_secret: Option<&str>) -> String {
    let mut pod = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "containers": [{
            "name": "copy",
            "image": cfg.trivy_image,
            "command": ["cp", "/usr/local/bin/trivy", format!("{BIN_DIR}/trivy")],
            "resources": {"limits": {"cpu": "1", "memory": "1Gi"}},
            "volumeMounts": [{"name": "fleetops", "mountPath": BIN_DIR}],
        }],
        "volumes": [{"name": "fleetops", "emptyDir": {}}],
    });
    if let Some(m) = mirror_secret {
        pod["imagePullSecrets"] = json!([{ "name": m }]);
    }
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": PREPARE_COPY_JOB, "namespace": NAMESPACE, "labels": labels()},
        "spec": {
            "backoffLimit": 0,
            "ttlSecondsAfterFinished": 300,
            "activeDeadlineSeconds": DEADLINE_SECS,
            "template": {"metadata": {"labels": labels()}, "spec": pod},
        },
    })
    .to_string()
}

/// What a failed prepare Job means, with Trivy's own error lines.
pub fn prepare_failure_message(job: &str, lines: &[String]) -> String {
    let why = super::docker::error_lines(lines);
    match job {
        PREPARE_COPY_JOB => format!(
            "the Trivy image has no /usr/local/bin/trivy or cp; use a copy of the official image: {why}"
        ),
        PREPARE_JAVA_JOB => format!("downloading the Java database failed: {why}"),
        _ => format!("downloading the vulnerability database failed: {why}"),
    }
}

/// A pod that will never start because an image can't be pulled: one of
/// FleetOps' Trivy containers, or the scanned `image` (empty for Jobs of
/// Trivy containers only). Reads each container's reason from the row's
/// `containers` column, which also covers init containers.
pub fn pull_failure(pods: &[fleetops_core::kube::KubeRow], image: &str) -> Option<String> {
    const STUCK: [&str; 4] = [
        "ImagePullBackOff",
        "ErrImagePull",
        "InvalidImageName",
        "CreateContainerConfigError",
    ];
    let stuck = |r: &str| STUCK.iter().any(|s| r.contains(s));
    for p in pods {
        let containers: Vec<Value> = p
            .columns
            .get("containers")
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        for c in &containers {
            let reason = c["reason"].as_str().unwrap_or_default();
            if !stuck(reason) {
                continue;
            }
            if c["init"].as_bool().unwrap_or(false) || image.is_empty() {
                return Some(format!("the scan pod can't start: {reason}"));
            }
            return Some(format!(
                "the cluster could not pull {image} for the scan: {reason}. \
                 Its nodes or pull secrets don't grant access to it."
            ));
        }
        if stuck(&p.status) {
            return Some(format!("the scan pod can't start: {}", p.status));
        }
    }
    None
}

/// How long a scan pod may wait for a node before the scan fails.
pub const SCHEDULE_GRACE_SECS: i64 = 120;

/// The scheduler's reason, once a pod created at `created` has been
/// unschedulable for [`SCHEDULE_GRACE_SECS`].
pub fn unschedulable(
    conds: &[fleetops_core::kube::KubeCondition],
    created: i64,
    now: i64,
) -> Option<String> {
    if now - created < SCHEDULE_GRACE_SECS {
        return None;
    }
    let c = conds.iter().find(|c| {
        c.type_ == "PodScheduled"
            && c.status == "False"
            && c.reason.as_deref() == Some("Unschedulable")
    })?;
    // A full cluster may be adding a node: only a pod no node could ever
    // take (platform, taints) fails early; the deadline covers the rest.
    if c.message
        .as_deref()
        .is_some_and(|m| m.contains("Insufficient"))
    {
        return None;
    }
    Some(format!(
        "no node can run the scan pod: {}",
        c.message.as_deref().unwrap_or("Unschedulable")
    ))
}

/// Waits for `job`: `Ok(true)` when it failed, an error when its pod can't
/// start, can't be scheduled, or it runs out of time. `image` is the
/// scanned image, empty for prepare Jobs.
async fn wait_job(k: &dyn KubeOps, job: &str, image: &str) -> Result<bool> {
    let until = now_unix() + DEADLINE_SECS + 60;
    loop {
        let o = k.get(KubeKind::Jobs, NAMESPACE, job).await?;
        let done = |ty: &str| {
            o.conditions
                .iter()
                .any(|c| c.type_ == ty && c.status == "True")
        };
        if done("Complete") {
            return Ok(false);
        }
        if done("Failed") {
            return Ok(true);
        }
        if let Ok(pods) = k.workload_pods(KubeKind::Jobs, NAMESPACE, job).await {
            if let Some(why) = pull_failure(&pods.rows, image) {
                return Err(Error::Invalid(why));
            }
            for p in pods.rows.iter().filter(|p| p.status == "Pending") {
                if let Ok(pod) = k.get(KubeKind::Pods, NAMESPACE, &p.name).await {
                    if let Some(why) = unschedulable(&pod.conditions, p.created, now_unix()) {
                        return Err(Error::Invalid(why));
                    }
                }
            }
        }
        if now_unix() > until {
            return Err(Error::Internal(
                "the scan job did not finish in time".into(),
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The whole log: the report is longer than any default tail.
pub fn report_log_options() -> LogOptions {
    LogOptions {
        follow: false,
        tail: None,
        stdout: true,
        stderr: true,
        ..Default::default()
    }
}

async fn job_logs(k: &dyn KubeOps, job: &str) -> Result<Vec<String>> {
    let pods = k.workload_pods(KubeKind::Jobs, NAMESPACE, job).await?;
    let pod = pods
        .rows
        .first()
        .map(|r| r.name.clone())
        .ok_or_else(|| Error::Internal("the scan job left no pod".into()))?;
    Ok(k.logs(NAMESPACE, &pod, &report_log_options())
        .filter_map(|l| async move { l.ok().map(|l| l.text) })
        .collect()
        .await)
}

/// `TRIVY_*` variables the scan container clears, overriding the image's.
const CLEARED_ENV: [&str; 15] = [
    "TRIVY_SERVER",
    "TRIVY_PKG_TYPES",
    "TRIVY_VULN_TYPE",
    "TRIVY_SKIP_FILES",
    "TRIVY_FILE_PATTERNS",
    "TRIVY_IGNORE_POLICY",
    "TRIVY_VEX",
    "TRIVY_IGNORE_STATUS",
    "TRIVY_DETECTION_PRIORITY",
    "TRIVY_DISTRO",
    "TRIVY_INCLUDE_DEV_DEPS",
    "TRIVY_EXIT_CODE",
    "TRIVY_OUTPUT",
    "TRIVY_CACHE_BACKEND",
    "TRIVY_TEMPLATE",
];

/// Where the injected Trivy binary and its scratch space live.
pub const BIN_DIR: &str = "/fleetops";

/// The scan Job, as JSON (which `apply` reads as YAML). Its pod runs the
/// scanned image itself, so the kubelet pulls it with whatever lets the
/// node run the workload (node identity, `pull_secret`), with a Trivy
/// binary copied in by an init container in place of the image's own
/// entrypoint.
pub fn job_manifest(
    cfg: &Effective,
    t: &ScanTarget,
    scan_id: i64,
    pull_secret: Option<&str>,
    mirror_secret: Option<&str>,
) -> String {
    let cache = json!({"name": "cache", "mountPath": "/cache"});
    let bin = json!({"name": "fleetops", "mountPath": BIN_DIR});
    // Init containers need requests too: limits alone would make the whole
    // pod request a CPU and a gigabyte.
    let small = json!({
        "limits": {"cpu": "1", "memory": "1Gi"},
        "requests": {"cpu": "100m", "memory": "256Mi"},
    });
    let download = |name: &str, args: Vec<String>| {
        json!({
            "name": name,
            "image": cfg.trivy_image,
            "args": args,
            "env": job_env(cfg),
            "resources": small,
            "volumeMounts": [cache],
        })
    };
    // Every Trivy setting the scanned image's ENV could carry that changes
    // findings or the report: flags cover most; these are cleared (an
    // empty variable is unset to Trivy) or pinned.
    let env: Vec<Value> = [
        ("TRIVY_IGNORE_UNFIXED", "false"),
        ("TRIVY_SEVERITY", trivy::SEVERITIES),
        ("TMPDIR", BIN_DIR),
    ]
    .into_iter()
    .chain(CLEARED_ENV.map(|n| (n, "")))
    .map(|(n, v)| json!({"name": n, "value": v}))
    .collect();
    let mut pod = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        "initContainers": [
            download("db", trivy::db_args(cfg)),
            download("java-db", trivy::java_db_args(cfg)),
            {
                "name": "copy",
                "image": cfg.trivy_image,
                "command": ["cp", "/usr/local/bin/trivy", format!("{BIN_DIR}/trivy")],
                "resources": small,
                "volumeMounts": [bin],
            },
        ],
        "containers": [{
            "name": "trivy",
            "image": pinned_ref(t),
            "imagePullPolicy": "IfNotPresent",
            "command": [format!("{BIN_DIR}/trivy")],
            "args": trivy::rootfs_args(cfg),
            "env": env,
            "securityContext": {"runAsUser": 0},
            "resources": {
                "limits": {"cpu": "1", "memory": "1Gi"},
                "requests": {"cpu": "100m", "memory": "256Mi"},
            },
            "volumeMounts": [cache, bin],
        }],
        "volumes": [
            {"name": "cache", "persistentVolumeClaim": {"claimName": CACHE_PVC}},
            {"name": "fleetops", "emptyDir": {}},
        ],
    });
    let pulls: Vec<Value> = mirror_secret
        .into_iter()
        .chain(pull_secret)
        .map(|n| json!({ "name": n }))
        .collect();
    if !pulls.is_empty() {
        pod["imagePullSecrets"] = Value::Array(pulls);
    }
    if let Some((os, arch)) = t.platform.as_deref().and_then(|p| p.split_once('/')) {
        pod["nodeSelector"] = json!({"kubernetes.io/os": os, "kubernetes.io/arch": arch});
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
    let mut docs = vec![json!({"apiVersion": "v1", "kind": "Namespace",
        "metadata": {"name": NAMESPACE, "labels": labels()}})
    .to_string()];
    docs.extend(mirror_secret(cfg));
    docs.extend(mirror_auth_secret(cfg));
    apply(k, &docs.join("\n---\n"), "create", "namespaces and secrets").await?;
    ensure_cache(k).await?;
    if cfg.auth.is_none() {
        // A mirror login that was removed: forget its copies.
        let _ = k
            .delete(KubeKind::Secrets, NAMESPACE, MIRROR_AUTH_SECRET)
            .await;
        let _ = k.delete(KubeKind::Secrets, NAMESPACE, MIRROR_SECRET).await;
    }
    let mirror = cfg.auth.is_some().then_some(MIRROR_SECRET);
    for (job, args) in prepare_jobs(cfg) {
        run_prepare(k, job, &prepare_manifest(cfg, job, &args, mirror)).await?;
    }
    run_prepare(k, PREPARE_COPY_JOB, &copy_check_manifest(cfg, mirror)).await
}

/// Creates the cache claim at [`CACHE_SIZE`], growing an older, smaller
/// one, or replacing it when its storage class can't grow: it only holds
/// databases Trivy downloads again.
async fn ensure_cache(k: &dyn KubeOps) -> Result<()> {
    let what = "persistentvolumeclaims";
    match apply(k, &cache_pvc(), "create", what).await {
        Err(e) if refuses_resize(&e.to_string()) => {}
        r => return r,
    }
    tracing::info!("replacing the scan cache with a larger one");
    k.delete(KubeKind::PersistentVolumeClaims, NAMESPACE, CACHE_PVC)
        .await
        .map_err(|e| rbac(e, "delete", what))?;
    let until = now_unix() + 120;
    loop {
        match k
            .get(KubeKind::PersistentVolumeClaims, NAMESPACE, CACHE_PVC)
            .await
        {
            Err(Error::NotFound(_)) => break,
            _ if now_unix() > until => {
                return Err(Error::Internal(
                    "the old scan cache is still in use; FleetOps will retry".into(),
                ))
            }
            _ => tokio::time::sleep(POLL).await,
        }
    }
    apply(k, &cache_pvc(), "create", what).await
}

async fn run_prepare(k: &dyn KubeOps, job: &str, manifest: &str) -> Result<()> {
    let _ = k.delete(KubeKind::Jobs, NAMESPACE, job).await;
    apply(k, manifest, "create", "jobs").await?;
    let out = match wait_job(k, job, "").await {
        Ok(false) => Ok(()),
        Ok(true) => {
            let lines = job_logs(k, job).await.unwrap_or_default();
            Err(Error::Invalid(prepare_failure_message(job, &lines)))
        }
        Err(e) => Err(e),
    };
    let _ = k.delete(KubeKind::Jobs, NAMESPACE, job).await;
    out
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

    let failed = wait_job(k, job, &pinned_ref(t)).await?;
    let lines = job_logs(k, job).await?;
    if failed {
        return Err(Error::Internal(format!(
            "trivy failed: {}",
            super::docker::error_lines(&lines)
        )));
    }
    Ok(trivy::report_from_log(&lines))
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
            let ours = r.name.starts_with("fleetops-scan-")
                && r.name != MIRROR_SECRET
                && r.name != MIRROR_AUTH_SECRET;
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
            platform: None,
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

    fn strs(v: &Value) -> Vec<String> {
        serde_json::from_value(v.clone()).unwrap()
    }

    fn pod(j: &Value) -> &Value {
        &j["spec"]["template"]["spec"]
    }

    #[test]
    fn job_has_limits_ttl_and_deadline() {
        let j = job(None, None);
        assert_eq!(j["metadata"]["name"], "fleetops-scan-9");
        assert_eq!(j["metadata"]["namespace"], NAMESPACE);
        assert_eq!(j["spec"]["backoffLimit"], 0);
        assert_eq!(j["spec"]["ttlSecondsAfterFinished"], 300);
        assert_eq!(j["spec"]["activeDeadlineSeconds"], 1200);
        let c = &pod(&j)["containers"][0];
        assert_eq!(
            c["resources"]["limits"],
            json!({"cpu": "1", "memory": "1Gi"})
        );
        assert_eq!(pod(&j)["restartPolicy"], "Never");
        assert!(
            !strs(&c["args"]).contains(&"--output".to_string()),
            "the report goes to the log"
        );
    }

    #[test]
    fn scan_container_runs_the_image_with_injected_trivy() {
        let j = job(None, None);
        let c = &pod(&j)["containers"][0];
        assert_eq!(c["image"], "nginx@sha256:d", "pinned to the digest");
        assert_eq!(c["imagePullPolicy"], "IfNotPresent");
        assert_eq!(strs(&c["command"]), ["/fleetops/trivy"]);
        assert_eq!(strs(&c["args"]), trivy::rootfs_args(&cfg()));
        assert_eq!(c["securityContext"]["runAsUser"], 0);
        assert_eq!(pod(&j)["automountServiceAccountToken"], false);
        assert_eq!(pod(&j)["enableServiceLinks"], false);
        let mounts: Vec<&str> = c["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mountPath"].as_str().unwrap())
            .collect();
        assert_eq!(mounts, ["/cache", "/fleetops"]);
    }

    #[test]
    fn init_containers_refresh_dbs_then_copy() {
        let c = cfg();
        let j = job(None, None);
        let init = pod(&j)["initContainers"].as_array().unwrap();
        let names: Vec<&str> = init.iter().map(|i| i["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["db", "java-db", "copy"]);
        assert!(init.iter().all(|i| i["image"] == c.trivy_image.as_str()));
        assert_eq!(strs(&init[0]["args"]), trivy::db_args(&c));
        assert_eq!(strs(&init[1]["args"]), trivy::java_db_args(&c));
        assert_eq!(
            strs(&init[2]["command"]),
            ["cp", "/usr/local/bin/trivy", "/fleetops/trivy"]
        );
        for i in init {
            assert_eq!(
                i["resources"]["requests"],
                json!({"cpu": "100m", "memory": "256Mi"}),
                "init limits alone would make the pod request a whole CPU"
            );
        }
    }

    #[test]
    fn scan_container_overrides_trivy_env() {
        let mut c = cfg();
        c.auth = Some(("bot".into(), "pw".into()));
        let j: Value = serde_json::from_str(&job_manifest(
            &c,
            &target("sha256:d", "nginx:1.25"),
            9,
            None,
            Some(MIRROR_SECRET),
        ))
        .unwrap();
        let env: Vec<(String, String)> = pod(&j)["containers"][0]["env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_string(),
                    e["value"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let mut want: Vec<(String, String)> = [
            ("TRIVY_IGNORE_UNFIXED", "false"),
            ("TRIVY_SEVERITY", "UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL"),
            ("TMPDIR", "/fleetops"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        // Cleared: an empty variable is unset to Trivy.
        for n in [
            "TRIVY_SERVER",
            "TRIVY_PKG_TYPES",
            "TRIVY_VULN_TYPE",
            "TRIVY_SKIP_FILES",
            "TRIVY_FILE_PATTERNS",
            "TRIVY_IGNORE_POLICY",
            "TRIVY_VEX",
            "TRIVY_IGNORE_STATUS",
            "TRIVY_DETECTION_PRIORITY",
            "TRIVY_DISTRO",
            "TRIVY_INCLUDE_DEV_DEPS",
            "TRIVY_EXIT_CODE",
            "TRIVY_OUTPUT",
            "TRIVY_CACHE_BACKEND",
            "TRIVY_TEMPLATE",
        ] {
            want.push((n.to_string(), String::new()));
        }
        assert_eq!(env, want);
    }

    #[test]
    fn pull_secrets_become_image_pull_secrets() {
        let j = job(Some("fleetops-scan-9-pull"), Some(MIRROR_SECRET));
        assert_eq!(
            pod(&j)["imagePullSecrets"],
            json!([{"name": MIRROR_SECRET}, {"name": "fleetops-scan-9-pull"}])
        );
        assert!(job(None, None)["spec"]["template"]["spec"]["imagePullSecrets"].is_null());
        let s = job(Some("fleetops-scan-9-pull"), None).to_string();
        assert!(
            !s.contains("DOCKER_CONFIG") && !s.contains("/docker"),
            "{s}"
        );
    }

    #[test]
    fn platform_sets_node_selector() {
        let mut t = target("sha256:d", "nginx:1.25");
        t.platform = Some("linux/arm64".into());
        let j: Value = serde_json::from_str(&job_manifest(&cfg(), &t, 9, None, None)).unwrap();
        assert_eq!(
            pod(&j)["nodeSelector"],
            json!({"kubernetes.io/os": "linux", "kubernetes.io/arch": "arm64"})
        );
        assert!(pod(&job(None, None))["nodeSelector"].is_null());
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
    fn mirror_password_is_never_in_the_job() {
        let mut c = cfg();
        c.auth = Some(("bot".into(), "hunter2".into()));
        let j = job_manifest(
            &c,
            &target("sha256:d", "nginx:1.25"),
            9,
            None,
            Some(MIRROR_SECRET),
        );
        assert!(!j.contains("hunter2"), "{j}");
        let v: Value = serde_json::from_str(&j).unwrap();
        let env = v["spec"]["template"]["spec"]["initContainers"][0]["env"]
            .as_array()
            .unwrap();
        let pw = env.iter().find(|e| e["name"] == "TRIVY_PASSWORD").unwrap();
        assert_eq!(
            pw["valueFrom"]["secretKeyRef"],
            json!({"name": MIRROR_AUTH_SECRET, "key": "password"})
        );
        let secret = mirror_auth_secret(&c).unwrap();
        assert!(!secret.contains("hunter2"), "base64 in data");
    }

    fn row(containers: Value) -> fleetops_core::kube::KubeRow {
        fleetops_core::kube::KubeRow {
            name: "p".into(),
            namespace: Some(NAMESPACE.into()),
            created: 0,
            status: "PodInitializing".into(),
            health: fleetops_core::kube::RowHealth::Warn,
            columns: [("containers".to_string(), containers.to_string())].into(),
        }
    }

    #[test]
    fn pull_failure_names_the_image() {
        let img = "123.dkr.ecr.us-east-1.amazonaws.com/app@sha256:d";
        let init = row(json!([
            {"name": "trivy", "init": false, "state": "waiting", "reason": "PodInitializing"},
            {"name": "db", "init": true, "state": "error", "reason": "ImagePullBackOff"},
        ]));
        assert_eq!(
            pull_failure(&[init], img).as_deref(),
            Some("the scan pod can't start: ImagePullBackOff")
        );
        let main = row(json!([
            {"name": "trivy", "init": false, "state": "error", "reason": "ErrImagePull"},
        ]));
        assert_eq!(
            pull_failure(&[main], img).as_deref(),
            Some("the cluster could not pull 123.dkr.ecr.us-east-1.amazonaws.com/app@sha256:d for the scan: ErrImagePull. Its nodes or pull secrets don't grant access to it.")
        );
        let ok = row(json!([{"name": "trivy", "init": false, "state": "running", "reason": ""}]));
        assert_eq!(pull_failure(&[ok], img), None);
    }

    #[test]
    fn unschedulable_after_grace() {
        let msg = "0/3 nodes are available: 3 node(s) didn't match Pod's node affinity/selector.";
        let cond = |status: &str| fleetops_core::kube::KubeCondition {
            type_: "PodScheduled".into(),
            status: status.into(),
            reason: Some("Unschedulable".into()),
            message: Some(msg.into()),
        };
        assert_eq!(unschedulable(&[cond("False")], 1000, 1119), None);
        assert_eq!(
            unschedulable(&[cond("False")], 1000, 1120),
            Some(format!("no node can run the scan pod: {msg}"))
        );
        assert_eq!(unschedulable(&[cond("True")], 1000, 5000), None);
        let busy = fleetops_core::kube::KubeCondition {
            message: Some("0/3 nodes are available: 3 Insufficient cpu.".into()),
            ..cond("False")
        };
        assert_eq!(
            unschedulable(&[busy], 1000, 5000),
            None,
            "a full cluster may be scaling up: wait for the deadline"
        );
    }

    #[test]
    fn prepare_checks_the_trivy_binary() {
        let j: Value = serde_json::from_str(&copy_check_manifest(&cfg(), None)).unwrap();
        assert_eq!(j["metadata"]["name"], PREPARE_COPY_JOB);
        let p = &j["spec"]["template"]["spec"];
        assert_eq!(
            strs(&p["containers"][0]["command"]),
            ["cp", "/usr/local/bin/trivy", "/fleetops/trivy"]
        );
        assert_eq!(p["volumes"][0]["emptyDir"], json!({}));
        assert!(
            prepare_failure_message(PREPARE_COPY_JOB, &["cp: can't stat".into()])
                .starts_with("the Trivy image has no /usr/local/bin/trivy or cp")
        );
        assert!(prepare_failure_message(PREPARE_JAVA_JOB, &[])
            .starts_with("downloading the Java database failed"));
    }

    #[test]
    fn report_log_reads_the_whole_log() {
        let o = report_log_options();
        assert_eq!(o.tail, None, "a report longer than the default tail is cut");
        assert!(!o.follow);
    }

    #[test]
    fn cache_holds_both_databases() {
        let p: Value = serde_json::from_str(&cache_pvc()).unwrap();
        assert_eq!(p["metadata"]["name"], CACHE_PVC);
        assert_eq!(p["spec"]["resources"]["requests"]["storage"], "10Gi");
    }

    #[test]
    fn resize_refusals_are_recognised() {
        assert!(refuses_resize("persistentvolumeclaims \"fleetops-trivy-cache\" is forbidden: only dynamically provisioned pvc can be resized and the storageclass that provisions the pvc must support resize"));
        assert!(!refuses_resize(
            "namespaces is forbidden: User cannot create"
        ));
    }

    #[test]
    fn prepare_jobs_download_each_database() {
        let c = cfg();
        let jobs = prepare_jobs(&c);
        assert_eq!(jobs.len(), 2);
        let args = |j: &str| -> Vec<String> {
            let v: Value = serde_json::from_str(j).unwrap();
            serde_json::from_value(v["spec"]["template"]["spec"]["containers"][0]["args"].clone())
                .unwrap()
        };
        assert_eq!(jobs[0].0, PREPARE_JOB);
        assert!(args(&prepare_manifest(&c, jobs[0].0, &jobs[0].1, None))
            .contains(&"--download-db-only".to_string()));
        assert_eq!(jobs[1].0, PREPARE_JAVA_JOB);
        assert!(args(&prepare_manifest(&c, jobs[1].0, &jobs[1].1, None))
            .contains(&"--download-java-db-only".to_string()));
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
