//! A real Trivy Job on the kind cluster from `make kind-up`:
//! `FLEETOPS_KUBE_IT=$PWD/target/kube-it cargo test -p fleetops-ee --features kube-it --test scan_kube_it`.
#![cfg(feature = "kube-it")]

use fleetops_core::kube::{ApplyMode, KubeKind, KubeOps};
use fleetops_ee::scan::{
    config::{effective, GlobalConfig},
    inventory::{kube_uses, SCAN_NAMESPACE},
    kube,
    model::ScanTarget,
    trivy::parse_report,
};
use fleetops_kube::KubeClient;
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
    };
    let report = kube::scan(&k, &t, &cfg, 4242).await.unwrap();
    parse_report(&report).unwrap();
    assert!(
        k.get(KubeKind::Jobs, SCAN_NAMESPACE, &kube::job_name(4242))
            .await
            .is_err(),
        "the job is deleted"
    );
    k.delete(KubeKind::Pods, NS, &name).await.unwrap();
}
