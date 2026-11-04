//! A real Trivy Job on the kind cluster from `make kind-up`:
//! `FLEETOPS_KUBE_IT=$PWD/target/kube-it cargo test -p fleetops-ee --features kube-it --test scan_kube_it`.
#![cfg(feature = "kube-it")]

use fleetops_core::kube::{ApplyMode, KubeKind, KubeOps};
use fleetops_core::streams::LogOptions;
use fleetops_ee::scan::{
    config::{effective, GlobalConfig},
    inventory::{kube_uses, SCAN_NAMESPACE},
    kube,
    model::ScanTarget,
    trivy::{parse_report, report_from_log},
};
use fleetops_kube::KubeClient;
use futures_util::StreamExt;
use std::collections::BTreeSet;
use std::path::PathBuf;

const NS: &str = "fleetops-it";

fn ops() -> KubeClient {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = PathBuf::from(std::env::var("FLEETOPS_KUBE_IT").expect("run after `make kind-up`"));
    let raw = std::fs::read_to_string(dir.join("kubeconfig")).unwrap();
    let p = fleetops_kube::kubeconfig::prepare(&raw, None, &Default::default()).unwrap();
    KubeClient::new(fleetops_kube::kubeconfig::client(&p.yaml, &Default::default()).unwrap())
}

#[tokio::test]
async fn scans_a_pod_image() {
    let k = ops();
    let name = format!("scan-it-{}", std::process::id());
    let pod = format!(
        "apiVersion: v1\nkind: Pod\nmetadata: {{name: {name}, namespace: {NS}}}\nspec:\n  terminationGracePeriodSeconds: 1\n  containers:\n    - {{name: main, image: alpine:3.20, imagePullPolicy: IfNotPresent, command: [sleep, '3600']}}\n"
    );
    let r = k
        .apply(&pod, NS, ApplyMode::Commit { force: false }, &|_| Ok(()))
        .await
        .unwrap();
    assert!(r.iter().all(|r| r.error.is_none()), "{r:?}");
    let mut found = None;
    for _ in 0..60 {
        let uses = kube_uses(1, k.image_uses().await.unwrap());
        found = uses
            .into_iter()
            .find(|u| u.workload == format!("Pod/{name}") && u.key.digest().is_some());
        if found.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let u = found.expect("the pod's image use, with its digest");
    let cfg = effective(&GlobalConfig::default(), None, None, None);
    kube::prepare(&k, &cfg).await.unwrap();
    let t = ScanTarget {
        env_id: 1,
        node: String::new(),
        key: u.key.clone(),
        image_ref: u.image_ref.clone(),
        namespace: u.namespace.clone(),
        pull_secrets: u.pull_secrets.clone(),
        platform: u.platform.clone(),
    };
    assert!(t.platform.is_some(), "kind's node has os/arch labels");
    let report = kube::scan(&k, &t, &cfg, 4242).await.unwrap();
    let node_pull = findings(&report);
    let remote = remote_scan(&k, &cfg, &kube::pinned_ref(&t)).await;
    assert_eq!(
        node_pull, remote,
        "node pull finds what `trivy image` finds"
    );
    assert!(
        k.get(KubeKind::Jobs, SCAN_NAMESPACE, &kube::job_name(4242))
            .await
            .is_err(),
        "the job is deleted"
    );
    k.delete(KubeKind::Pods, NS, &name).await.unwrap();
}

type FindingKey = (String, String, String);

fn findings(report: &[u8]) -> BTreeSet<FindingKey> {
    parse_report(report)
        .unwrap()
        .into_iter()
        .map(|f| (f.vuln_id, f.pkg_name, f.installed_version))
        .collect()
}

/// `trivy image --image-src remote` of the same digest, as a plain Job on
/// the shared cache: the scan FleetOps ran before node pull.
async fn remote_scan(
    k: &KubeClient,
    cfg: &fleetops_ee::scan::config::Effective,
    image: &str,
) -> BTreeSet<FindingKey> {
    let job = "fleetops-scan-parity";
    let manifest = serde_json::json!({
        "apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": job, "namespace": SCAN_NAMESPACE},
        "spec": {"backoffLimit": 0, "template": {"spec": {
            "restartPolicy": "Never",
            "containers": [{"name": "trivy", "image": cfg.trivy_image,
                "args": ["image", "--scanners", "vuln", "--format", "json", "--cache-dir", "/cache",
                    "--quiet", "--image-src", "remote", "--skip-db-update", "--skip-java-db-update", image],
                "volumeMounts": [{"name": "cache", "mountPath": "/cache"}]}],
            "volumes": [{"name": "cache", "persistentVolumeClaim": {"claimName": kube::CACHE_PVC}}],
        }}},
    })
    .to_string();
    let _ = k.delete(KubeKind::Jobs, SCAN_NAMESPACE, job).await;
    let r = k
        .apply(
            &manifest,
            SCAN_NAMESPACE,
            ApplyMode::Commit { force: true },
            &|_| Ok(()),
        )
        .await
        .unwrap();
    assert!(r.iter().all(|r| r.error.is_none()), "{r:?}");
    for _ in 0..600 {
        let o = k.get(KubeKind::Jobs, SCAN_NAMESPACE, job).await.unwrap();
        if o.conditions
            .iter()
            .any(|c| c.type_ == "Complete" && c.status == "True")
        {
            break;
        }
        assert!(
            !o.conditions
                .iter()
                .any(|c| c.type_ == "Failed" && c.status == "True"),
            "the parity scan failed"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let pods = k
        .workload_pods(KubeKind::Jobs, SCAN_NAMESPACE, job)
        .await
        .unwrap();
    let opts = LogOptions {
        follow: false,
        tail: None,
        ..Default::default()
    };
    let lines: Vec<String> = k
        .logs(SCAN_NAMESPACE, &pods.rows[0].name, &opts)
        .filter_map(|l| async move { l.ok().map(|l| l.text) })
        .collect()
        .await;
    let _ = k.delete(KubeKind::Jobs, SCAN_NAMESPACE, job).await;
    findings(&report_from_log(&lines))
}

#[tokio::test]
async fn scans_a_distroless_image() {
    let k = ops();
    let name = format!("scan-distroless-{}", std::process::id());
    let image = "gcr.io/distroless/static-debian12:nonroot";
    let dep = format!(
        "apiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: {name}, namespace: {NS}}}\nspec:\n  replicas: 0\n  selector: {{matchLabels: {{app: {name}}}}}\n  template:\n    metadata: {{labels: {{app: {name}}}}}\n    spec:\n      containers:\n        - {{name: main, image: '{image}'}}\n"
    );
    let r = k
        .apply(&dep, NS, ApplyMode::Commit { force: false }, &|_| Ok(()))
        .await
        .unwrap();
    assert!(r.iter().all(|r| r.error.is_none()), "{r:?}");
    let uses = kube_uses(1, k.image_uses().await.unwrap());
    let u = uses
        .into_iter()
        .find(|u| u.workload == format!("Deployment/{name}"))
        .expect("the template use");
    let cfg = effective(&GlobalConfig::default(), None, None, None);
    kube::prepare(&k, &cfg).await.unwrap();
    let t = ScanTarget {
        env_id: 1,
        node: String::new(),
        key: u.key.clone(),
        image_ref: u.image_ref.clone(),
        namespace: u.namespace.clone(),
        pull_secrets: u.pull_secrets.clone(),
        platform: None,
    };
    let report = kube::scan(&k, &t, &cfg, 4243).await.unwrap();
    parse_report(&report).unwrap();
    k.delete(KubeKind::Deployments, NS, &name).await.unwrap();
}
