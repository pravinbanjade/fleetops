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

use fleetops_core::helm::{ChartRef, Deploy, HelmMode};
use fleetops_kube::helm::render::{ChartRenderer, HelmCli, RenderInput};
use std::path::Path;

/// Writes a chart named `name` at `version` into `dir`: the 3d test chart,
/// plus `extra` templates.
fn write_chart(
    dir: &Path,
    name: &str,
    version: &str,
    extra: &[(&str, &str)],
) -> std::path::PathBuf {
    let c = dir.join(format!("{name}-{version}"));
    std::fs::create_dir_all(c.join("templates")).unwrap();
    let base = std::path::PathBuf::from(chart());
    std::fs::write(
        c.join("Chart.yaml"),
        format!("apiVersion: v2\nname: {name}\nversion: {version}\nappVersion: \"1.0.0\"\n"),
    )
    .unwrap();
    std::fs::copy(base.join("values.yaml"), c.join("values.yaml")).unwrap();
    for t in ["deploy.yaml", "extra.yaml"] {
        std::fs::copy(base.join("templates").join(t), c.join("templates").join(t)).unwrap();
    }
    for (file, text) in extra {
        std::fs::write(c.join("templates").join(file), text).unwrap();
    }
    c
}

/// Packages charts into a directory served over HTTP on 127.0.0.1, with an index.
async fn serve_repo(charts: &[std::path::PathBuf]) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    for c in charts {
        let out = helm_cmd()
            .args([
                "package",
                c.to_str().unwrap(),
                "-d",
                dir.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = helm_cmd()
        .args(["repo", "index", dir.path().to_str().unwrap(), "--url", &url])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let root = dir.path().to_path_buf();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let root = root.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/')
                    .to_string();
                let body = std::fs::read(root.join(&path)).ok();
                let head = match &body {
                    Some(b) => format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        b.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .into()
                    }
                };
                let _ = s.write_all(head.as_bytes()).await;
                if let Some(b) = body {
                    let _ = s.write_all(&b).await;
                }
            });
        }
    });
    (url, dir)
}

fn helm_cli() -> HelmCli {
    HelmCli {
        binary: std::env::var("HELM").unwrap_or_else(|_| "helm".into()),
        insecure_repos: true,
        timeout: fleetops_kube::helm::render::RENDER_TIMEOUT,
        // The server uses its own binary; a test binary cannot stand in.
        discard: Some(fleetops_kube::helm::render::Discard {
            program: "sh".into(),
            args: vec!["-c".into(), "cat >/dev/null".into()],
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn deploy(
    ops: &fleetops_kube::KubeClient,
    url: &str,
    chart: &str,
    version: &str,
    name: &str,
    values_yaml: &str,
    upgrade: bool,
    allowed: Option<&std::collections::BTreeSet<String>>,
) -> fleetops_core::Result<fleetops_core::helm::HelmPlan> {
    let source = ChartRef {
        repo_url: Some(url.into()),
        chart: Some(chart.into()),
        version: Some(version.into()),
        oci: None,
    };
    // As the API does: helm reads the typed YAML plus what was restored.
    let stored = if upgrade {
        ops.helm_config(NS, name).await?
    } else {
        serde_json::Value::Null
    };
    let (full, overlay) = fleetops_kube::helm::values::restore(
        &fleetops_kube::helm::values::parse(values_yaml)?,
        &stored,
    )?;
    let input = RenderInput {
        name: name.into(),
        namespace: NS.into(),
        chart: source.clone(),
        values: full,
        values_yaml: values_yaml.into(),
        overlay,
        upgrade,
        kube_version: ops.version().await?.git_version,
        api_versions: ops.api_versions().await?,
    };
    let rendered = helm_cli().render(&input).await?;
    let d = Deploy {
        namespace: NS.into(),
        name: name.into(),
        config: rendered.config.clone(),
        rendered,
        source,
        upgrade,
    };
    let plan = ops.helm_deploy(&d, HelmMode::Plan, allowed).await?;
    ops.helm_deploy(
        &d,
        HelmMode::Commit {
            expected_current: plan.current,
        },
        allowed,
    )
    .await
}

fn uniq(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        if std::env::var_os("FLEETOPS_IT_TUNNEL").is_some() {
            "t"
        } else {
            "d"
        }
    )
}

#[tokio::test]
async fn install_then_upgrade_leaves_a_history_helm_reads() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let (url, _repo) = serve_repo(&[
        write_chart(tmp.path(), "fodemo", "0.1.0", &[]),
        write_chart(tmp.path(), "fodemo", "0.2.0", &[]),
    ])
    .await;
    let name = uniq("fo-install");
    let _ = helm_cmd()
        .args([
            "uninstall",
            &name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .output();
    let p = deploy(
        &ops,
        &url,
        "fodemo",
        "0.1.0",
        &name,
        &serde_json::json!({"password": "s3cret", "extra": true}).to_string(),
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        (p.revision, p.status.as_deref()),
        (Some(1), Some("deployed")),
        "{p:?}"
    );
    let e = deploy(
        &ops,
        &url,
        "fodemo",
        "0.1.0",
        &name,
        &serde_json::json!({}).to_string(),
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("exists; upgrade it"), "{e}");

    // An upgrade that keeps the masked password.
    let sent = format!("password: {MASK}\nextra: false\nmode: 0644\n");
    let p = deploy(&ops, &url, "fodemo", "0.2.0", &name, &sent, true, None)
        .await
        .unwrap();
    assert_eq!(
        (p.revision, p.status.as_deref()),
        (Some(2), Some("deployed")),
        "{p:?}"
    );

    let hist: serde_json::Value =
        serde_json::from_str(&helm(&["history", &name, "-o", "json"])).unwrap();
    let statuses: Vec<_> = hist
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(statuses, ["superseded", "deployed"]);
    let vals = helm(&["get", "values", &name, "-o", "json"]);
    assert!(
        vals.contains("s3cret")
            && vals.contains("\"extra\":false")
            && vals.contains("\"mode\":420"),
        "{vals}"
    );
    assert_eq!(
        ops.helm_get(NS, &name, None)
            .await
            .unwrap()
            .chart_source
            .unwrap()
            .chart
            .as_deref(),
        Some("fodemo")
    );
    // The CLI carries on from a FleetOps release.
    helm(&[
        "upgrade",
        &name,
        &format!("{}/fodemo-0.2.0", tmp.path().display()),
        "--reuse-values",
    ]);
    helm(&["uninstall", &name, "--wait"]);
}

#[tokio::test]
async fn a_cli_release_is_upgraded_and_then_remembers_its_source() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let c1 = write_chart(tmp.path(), "fodemo", "0.1.0", &[]);
    let (url, _repo) =
        serve_repo(&[c1.clone(), write_chart(tmp.path(), "fodemo", "0.2.0", &[])]).await;
    let name = uniq("fo-cli");
    let _ = helm_cmd()
        .args([
            "uninstall",
            &name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .output();
    helm(&["install", &name, c1.to_str().unwrap()]);
    assert!(ops
        .helm_get(NS, &name, None)
        .await
        .unwrap()
        .chart_source
        .is_none());
    let p = deploy(
        &ops,
        &url,
        "fodemo",
        "0.2.0",
        &name,
        &serde_json::json!({}).to_string(),
        true,
        None,
    )
    .await
    .unwrap();
    assert_eq!(p.status.as_deref(), Some("deployed"), "{p:?}");
    assert!(ops
        .helm_get(NS, &name, None)
        .await
        .unwrap()
        .chart_source
        .is_some());
    helm(&["uninstall", &name, "--wait"]);
}

const PRE_CM: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {{ .Release.Name }}-pre\n  annotations:\n    \"helm.sh/hook\": pre-install\n    \"helm.sh/hook-weight\": \"-1\"\ndata: {k: v}\n";
const PRE_JOB_OK: &str = "apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: {{ .Release.Name }}-job\n  annotations:\n    \"helm.sh/hook\": pre-install\n    \"helm.sh/hook-delete-policy\": hook-succeeded\nspec:\n  backoffLimit: 0\n  template:\n    spec:\n      restartPolicy: Never\n      containers: [{name: m, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [sh, -c, 'exit 0']}]\n";
const PRE_JOB_FAIL: &str = "apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: {{ .Release.Name }}-job\n  annotations:\n    \"helm.sh/hook\": pre-install\nspec:\n  backoffLimit: 0\n  template:\n    spec:\n      restartPolicy: Never\n      containers: [{name: m, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [sh, -c, 'exit 1']}]\n";

#[tokio::test]
async fn pre_install_hooks_run_before_the_release_objects() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let (url, _repo) = serve_repo(&[write_chart(
        tmp.path(),
        "fohooked",
        "0.1.0",
        &[("cm.yaml", PRE_CM), ("job.yaml", PRE_JOB_OK)],
    )])
    .await;
    let name = uniq("fo-hooked");
    let _ = helm_cmd()
        .args([
            "uninstall",
            &name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .output();
    let p = deploy(
        &ops,
        &url,
        "fohooked",
        "0.1.0",
        &name,
        &serde_json::json!({}).to_string(),
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(p.status.as_deref(), Some("deployed"), "{p:?}");
    let cms: Api<ConfigMap> = Api::namespaced(common::kube_client(), NS);
    let deps: Api<Deployment> = Api::namespaced(common::kube_client(), NS);
    let cm = cms.get(&format!("{name}-pre")).await.unwrap();
    let dep = deps.get(&name).await.unwrap();
    assert!(cm.metadata.creation_timestamp <= dep.metadata.creation_timestamp);
    let jobs: Api<k8s_openapi::api::batch::v1::Job> = Api::namespaced(common::kube_client(), NS);
    assert!(
        jobs.get_opt(&format!("{name}-job"))
            .await
            .unwrap()
            .is_none(),
        "hook-succeeded deletes it"
    );
    helm(&["uninstall", &name, "--wait"]);
    let _ = cms
        .delete(&format!("{name}-pre"), &DeleteParams::default())
        .await;
}

#[tokio::test]
async fn a_failing_hook_leaves_the_release_failed_and_names_the_hook() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let (url, _repo) = serve_repo(&[write_chart(
        tmp.path(),
        "fofailing",
        "0.1.0",
        &[("job.yaml", PRE_JOB_FAIL)],
    )])
    .await;
    let name = uniq("fo-failing");
    let _ = helm_cmd()
        .args([
            "uninstall",
            &name,
            "--namespace",
            NS,
            "--ignore-not-found",
            "--wait",
        ])
        .output();
    let p = deploy(
        &ops,
        &url,
        "fofailing",
        "0.1.0",
        &name,
        &serde_json::json!({}).to_string(),
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(p.status.as_deref(), Some("failed"), "{p:?}");
    assert!(
        p.note.contains(&format!("pre-install hook Job {name}-job")),
        "{}",
        p.note
    );
    let deps: Api<Deployment> = Api::namespaced(common::kube_client(), NS);
    assert!(
        deps.get_opt(&name).await.unwrap().is_none(),
        "objects wait for pre-install hooks"
    );
    let hist: serde_json::Value =
        serde_json::from_str(&helm(&["history", &name, "-o", "json"])).unwrap();
    assert_eq!(hist[0]["status"], "failed");
    helm(&["uninstall", &name, "--wait"]);
    let jobs: Api<k8s_openapi::api::batch::v1::Job> = Api::namespaced(common::kube_client(), NS);
    let _ = jobs
        .delete(&format!("{name}-job"), &DeleteParams::background())
        .await;
}

#[tokio::test]
async fn a_limited_grant_refuses_a_cluster_scoped_object_before_writing() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let role = "apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata: {name: '{{ .Release.Name }}-cr'}\nrules: []\n";
    let (url, _repo) = serve_repo(&[write_chart(
        tmp.path(),
        "focluster",
        "0.1.0",
        &[("cr.yaml", role)],
    )])
    .await;
    let name = uniq("fo-limited");
    let allowed: std::collections::BTreeSet<String> = [NS.to_string()].into();
    let e = deploy(
        &ops,
        &url,
        "focluster",
        "0.1.0",
        &name,
        &serde_json::json!({}).to_string(),
        false,
        Some(&allowed),
    )
    .await
    .unwrap_err();
    assert!(matches!(e, Error::Forbidden(_)), "{e}");
    assert!(
        ops.helm_history(NS, &name).await.is_err(),
        "no release record was written"
    );
}

const GADGET_CRD: &str = "apiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata: {name: gadgets.fo.example}\nspec:\n  group: fo.example\n  scope: Namespaced\n  names: {plural: gadgets, singular: gadget, kind: Gadget}\n  versions:\n    - name: v1\n      served: true\n      storage: true\n      schema: {openAPIV3Schema: {type: object, x-kubernetes-preserve-unknown-fields: true}}\n";
const GADGET: &str = "apiVersion: fo.example/v1\nkind: Gadget\nmetadata: {name: '{{ .Release.Name }}'}\nspec: {size: 1}\n";

#[tokio::test]
async fn chart_crds_are_installed_once_and_outlive_the_release() {
    let ops = common::ops();
    let tmp = tempfile::tempdir().unwrap();
    let c = write_chart(tmp.path(), "foop", "0.1.0", &[("gadget.yaml", GADGET)]);
    std::fs::create_dir_all(c.join("crds")).unwrap();
    std::fs::write(c.join("crds").join("gadgets.yaml"), GADGET_CRD).unwrap();
    let (url, _repo) = serve_repo(&[c]).await;
    let (a, b) = (uniq("fo-crd-a"), uniq("fo-crd-b"));
    for n in [&a, &b] {
        let _ = helm_cmd()
            .args([
                "uninstall",
                n,
                "--namespace",
                NS,
                "--ignore-not-found",
                "--wait",
            ])
            .output();
    }
    // A cluster that has never seen the CRD, as in CI: with one left from
    // an earlier run, helm resolves the chart's own kinds and the test
    // would pass without exercising the first install.
    let crds: Api<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition> =
        Api::all(common::kube_client());
    let _ = crds.delete("gadgets.fo.example", &Default::default()).await;
    for _ in 0..60 {
        if crds.get_opt("gadgets.fo.example").await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(crds.get_opt("gadgets.fo.example").await.unwrap().is_none());
    let p = deploy(&ops, &url, "foop", "0.1.0", &a, "", false, None)
        .await
        .unwrap();
    assert_eq!(p.status.as_deref(), Some("deployed"), "{p:?}");
    let manifest = helm(&["get", "manifest", &a]);
    assert!(
        !manifest.contains("CustomResourceDefinition"),
        "CRDs stay out of the manifest: {manifest}"
    );
    assert!(manifest.contains("kind: Gadget"));
    // A second release of the same chart finds the CRD and leaves it be.
    let p = deploy(&ops, &url, "foop", "0.1.0", &b, "", false, None)
        .await
        .unwrap();
    assert_eq!(p.status.as_deref(), Some("deployed"), "{p:?}");
    helm(&["uninstall", &a, "--wait"]);
    helm(&["uninstall", &b, "--wait"]);
    assert!(
        crds.get_opt("gadgets.fo.example").await.unwrap().is_some(),
        "uninstall never deletes a chart's CRDs"
    );
}
