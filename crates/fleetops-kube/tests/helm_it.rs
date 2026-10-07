//! Helm interop against a real kind cluster with the real `helm` CLI:
//! `make test-kube`. FleetOps' rollbacks and uninstalls must leave a
//! history the CLI can carry on from.
#![cfg(feature = "kube-it")]

mod common;

use common::NS;
use fleetops_core::helm::CHANGED;
use fleetops_core::kube::{KubeOps, MASK};
use fleetops_core::Error;
use k8s_openapi::api::{apps::v1::Deployment, core::v1::ConfigMap};
use kube::api::{Api, DeleteParams};
use std::process::Command;

fn release() -> &'static str {
    if std::env::var_os("FLEETOPS_IT_TUNNEL").is_some() {
        "demo-tunnel"
    } else {
        "demo-direct"
    }
}

fn chart() -> String {
    format!("{}/../../scripts/helm-it-chart", env!("CARGO_MANIFEST_DIR"))
}

fn helm_cmd() -> Command {
    let mut c = Command::new(std::env::var("HELM").unwrap_or_else(|_| "helm".into()));
    c.env("KUBECONFIG", common::dir().join("kubeconfig"));
    c
}

/// Runs `helm` against kind; panics with its output on failure.
fn helm(args: &[&str]) -> String {
    let out = helm_cmd()
        .args(args)
        .args(["--namespace", NS])
        .output()
        .expect("helm is required for the Helm suite (brew install helm)");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "helm {args:?}: {text}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn history() -> serde_json::Value {
    serde_json::from_str(&helm(&["history", release(), "-o", "json"])).unwrap()
}

#[tokio::test]
async fn rollback_and_uninstall_leave_a_history_helm_can_continue() {
    let ops = common::ops();
    let name = release();
    let _ = helm_cmd()
        .args([
            "uninstall",
            name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .status();
    helm(&["install", name, &chart()]);
    helm(&[
        "upgrade",
        name,
        &chart(),
        "--set",
        "extra=true",
        "--set",
        "note=hello",
    ]);

    // Listed, with the chart; values masked.
    let rows = ops.helm_list(Some(NS)).await.unwrap();
    let row = rows.iter().find(|r| r.name == name).expect("listed");
    assert_eq!(
        (row.revision, row.status.as_str(), row.chart.as_str()),
        (2, "deployed", "demo")
    );
    let d = ops.helm_get(NS, name, None).await.unwrap();
    assert!(
        d.values.contains("note") && !d.values.contains("hunter2"),
        "{}",
        d.values
    );
    assert_eq!(ops.helm_history(NS, name).await.unwrap().len(), 2);

    // A deployment deleted by hand is recreated, not a 404.
    let deps: Api<Deployment> = Api::namespaced(common::kube_client(), NS);
    deps.delete(name, &DeleteParams::default()).await.unwrap();
    common::eventually("the deployment to go", || async {
        deps.get_opt(name).await.unwrap().is_none()
    })
    .await;

    // Dry run first: nothing changes.
    let plan = ops.helm_rollback(NS, name, 1, 2, true, None).await.unwrap();
    assert_eq!(plan.current, 2);
    assert!(
        plan.changes.iter().all(|c| c.error.is_none()),
        "{:?}",
        plan.changes
    );
    let cms: Api<ConfigMap> = Api::namespaced(common::kube_client(), NS);
    assert!(cms
        .get_opt(&format!("{name}-extra"))
        .await
        .unwrap()
        .is_some());

    // A stale commit is refused.
    let stale = ops
        .helm_rollback(NS, name, 1, 1, false, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&stale, Error::Conflict(m) if m == CHANGED),
        "{stale}"
    );

    // Roll back to 1: the annotation and the ConfigMap go, the deployment is back.
    let done = ops
        .helm_rollback(NS, name, 1, 2, false, None)
        .await
        .unwrap();
    assert_eq!(
        (done.revision, done.status.as_deref()),
        (Some(3), Some("deployed")),
        "{:?}",
        done.changes
    );
    assert!(cms
        .get_opt(&format!("{name}-extra"))
        .await
        .unwrap()
        .is_none());
    let dep = deps.get(name).await.unwrap();
    assert!(!dep
        .metadata
        .annotations
        .unwrap_or_default()
        .contains_key("fleetops.dev/note"));
    let h = history();
    assert_eq!(h[2]["revision"], 3);
    assert_eq!(h[2]["status"], "deployed");
    assert!(h[2]["description"]
        .as_str()
        .unwrap()
        .contains("Rollback to 1"));
    assert_eq!(h[1]["status"], "superseded");

    // Roll forward to 2 (creates the ConfigMap), then the CLI carries on.
    let fwd = ops
        .helm_rollback(NS, name, 2, 3, false, None)
        .await
        .unwrap();
    assert_eq!(fwd.status.as_deref(), Some("deployed"), "{:?}", fwd.changes);
    let cm = cms.get(&format!("{name}-extra")).await.unwrap();
    assert_eq!(
        cm.metadata.labels.unwrap()["app.kubernetes.io/managed-by"],
        "Helm"
    );
    helm(&["upgrade", name, &chart(), "--set", "extra=true"]);
    assert_eq!(history().as_array().unwrap().len(), 5);

    // Values never come back unmasked.
    let d = ops.helm_get(NS, name, Some(1)).await.unwrap();
    assert!(!d.values.contains("hunter2"));
    assert!(!d.manifest.contains("hunter2") || d.manifest.contains(MASK));

    // Uninstall: objects and records gone.
    let cur = ops
        .helm_list(Some(NS))
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == name)
        .unwrap()
        .revision;
    let u = ops
        .helm_uninstall(NS, name, cur, false, None)
        .await
        .unwrap();
    assert_eq!(u.status.as_deref(), Some("uninstalled"), "{:?}", u.changes);
    assert!(!helm(&["list", "-q"]).lines().any(|l| l == name));
    common::eventually("the deployment to go", || async {
        deps.get_opt(name).await.unwrap().is_none()
    })
    .await;
}

/// A fresh release named `name`, removed first if a previous run left it.
fn reinstall(name: &str, args: &[&str]) {
    let _ = helm_cmd()
        .args([
            "uninstall",
            name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .status();
    let chart_dir = chart();
    let mut a = vec!["install", name, &chart_dir];
    a.extend_from_slice(args);
    helm(&a);
}

fn history_of(name: &str) -> serde_json::Value {
    serde_json::from_str(&helm(&["history", name, "-o", "json"])).unwrap()
}

#[tokio::test]
async fn rolling_back_a_failed_upgrade_keeps_it_failed_and_supersedes_the_deployed_one() {
    let ops = common::ops();
    let name = &format!("{}-failed", release());
    reinstall(name, &[]);
    // An image that never pulls: `--wait` gives up and Helm marks revision 2 failed.
    let out = helm_cmd()
        .args([
            "upgrade",
            name,
            &chart(),
            "--namespace",
            NS,
            "--set",
            "image=busybox:no-such-tag",
            "--wait",
            "--timeout",
            "5s",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the upgrade was meant to fail");
    let h = history_of(name);
    assert_eq!(
        (h[0]["status"].as_str(), h[1]["status"].as_str()),
        (Some("deployed"), Some("failed"))
    );

    let done = ops
        .helm_rollback(NS, name, 1, 2, false, None)
        .await
        .unwrap();
    assert_eq!(
        done.status.as_deref(),
        Some("deployed"),
        "{:?}",
        done.changes
    );
    let h = history_of(name);
    let statuses: Vec<_> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(statuses, ["superseded", "failed", "deployed"], "{h}");
    helm(&["uninstall", name, "--wait"]);
}

#[tokio::test]
async fn an_object_kept_by_an_upgrade_is_taken_back_by_a_rollback() {
    let ops = common::ops();
    let name = &format!("{}-keep", release());
    reinstall(name, &["--set", "extra=true", "--set", "keep=true"]);
    // Revision 2 drops the ConfigMap, but its keep policy leaves it in the cluster.
    helm(&["upgrade", name, &chart(), "--set", "extra=false"]);
    let cms: Api<ConfigMap> = Api::namespaced(common::kube_client(), NS);
    assert!(cms
        .get_opt(&format!("{name}-extra"))
        .await
        .unwrap()
        .is_some());

    let plan = ops.helm_rollback(NS, name, 1, 2, true, None).await.unwrap();
    assert!(
        plan.changes.iter().all(|c| c.error.is_none()),
        "{:?}",
        plan.changes
    );
    let done = ops
        .helm_rollback(NS, name, 1, 2, false, None)
        .await
        .unwrap();
    assert_eq!(
        done.status.as_deref(),
        Some("deployed"),
        "{:?}",
        done.changes
    );
    let _ = helm_cmd()
        .args(["uninstall", name, "--namespace", NS, "--wait"])
        .status();
    let _ = cms
        .delete(&format!("{name}-extra"), &DeleteParams::default())
        .await;
}
