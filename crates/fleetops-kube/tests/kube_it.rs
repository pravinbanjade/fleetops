//! Against a real kind cluster: `make test-kube`.
#![cfg(feature = "kube-it")]

mod common;

use common::NS;
use futures_util::{AsyncBufReadExt, SinkExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, ListParams, LogParams, TerminalSize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

use fleetops_core::kube::{KubeKind, KubeOps, MASK};
use fleetops_core::streams::{ExecOptions, LogOptions};
use fleetops_core::Error;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::PostParams;

const WAIT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn through_the_tunnel_lists_pods_follows_logs_and_resizes_exec() {
    let pods: Api<Pod> = Api::namespaced(common::tunnel_client(), NS);

    let names: Vec<String> = pods
        .list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .filter_map(|p| p.metadata.name)
        .collect();
    assert!(names.iter().any(|n| n == "ticker"), "{names:?}");

    // Two different lines prove the stream follows rather than ends.
    let lp = LogParams {
        container: Some("main".into()),
        follow: true,
        tail_lines: Some(1),
        ..LogParams::default()
    };
    let mut lines = pods.log_stream("ticker", &lp).await.unwrap().lines();
    let a = timeout(WAIT, lines.try_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let b = timeout(WAIT, lines.try_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        a.starts_with("tick") && b.starts_with("tick") && a != b,
        "{a:?} {b:?}"
    );

    // A TTY exec that sees the size we set: the upgrade survived both hops.
    let ap = AttachParams::interactive_tty().container("main");
    let mut p = pods.exec("ticker", vec!["sh"], &ap).await.unwrap();
    let mut size = p.terminal_size().unwrap();
    size.send(TerminalSize {
        width: 123,
        height: 45,
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut stdin = p.stdin().unwrap();
    let mut stdout = p.stdout().unwrap();
    stdin.write_all(b"stty size; exit\n").await.unwrap();
    let mut out = Vec::new();
    timeout(WAIT, stdout.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("45 123"), "{out}");
}

#[tokio::test]
async fn every_kind_lists() {
    let ops = common::ops();
    for kind in KubeKind::ALL {
        let ns = if kind.cluster_scoped() {
            None
        } else {
            Some(NS)
        };
        ops.list(kind, ns)
            .await
            .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
    }
    let ns = ops.list(KubeKind::Namespaces, None).await.unwrap();
    assert!(ns.rows.iter().any(|r| r.name == NS));
    let nodes = ops.list(KubeKind::Nodes, None).await.unwrap();
    assert!(!nodes.rows.is_empty());
    let deps = ops.list(KubeKind::Deployments, Some(NS)).await.unwrap();
    let web = deps.rows.iter().find(|r| r.name == "web").expect("web");
    assert!(web.columns["ready"].ends_with("/2"), "{web:?}");
    // All namespaces includes ours.
    let all = ops.list(KubeKind::Pods, None).await.unwrap();
    assert!(all.rows.iter().any(|r| r.namespace.as_deref() == Some(NS)));
}

#[tokio::test]
async fn a_capped_list_says_it_is_truncated() {
    let ops = common::ops().with_list_cap(1);
    let pods = ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    assert_eq!(pods.rows.len(), 1);
    assert!(pods.truncated);
}

#[tokio::test]
async fn a_pod_shows_yaml_owners_and_containers() {
    let ops = common::ops();
    let pods = ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    let web = pods
        .rows
        .iter()
        .find(|r| r.name.starts_with("web-"))
        .expect("a web pod");
    let o = ops.get(KubeKind::Pods, NS, &web.name).await.unwrap();
    assert!(
        o.yaml.contains("kind: Pod") && !o.yaml.contains("managedFields"),
        "{}",
        o.yaml
    );
    let kinds: Vec<_> = o.owners.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["ReplicaSet", "Deployment"]);
    assert_eq!(o.owners[1].link, Some(KubeKind::Deployments));
    assert_eq!(o.containers, ["web"]);
    assert!(o.conditions.iter().any(|c| c.type_ == "Ready"));
    let t = ops.get(KubeKind::Pods, NS, "ticker").await.unwrap();
    assert_eq!(t.containers, ["main", "side"]);
}

#[tokio::test]
async fn secret_yaml_never_contains_its_value() {
    let ops = common::ops();
    let s = ops.get(KubeKind::Secrets, NS, "creds").await.unwrap();
    assert!(s.yaml.contains(MASK), "{}", s.yaml);
    // Neither the plain value nor its base64 form, anywhere (the
    // last-applied annotation holds it too).
    assert!(!s.yaml.contains("hunter2-do-not-leak"), "{}", s.yaml);
    assert!(!s.yaml.contains("aHVudGVyMi1kby1ub3QtbGVhaw"), "{}", s.yaml);
}

#[tokio::test]
async fn scale_changes_the_desired_replicas() {
    let ops = common::ops();
    ops.scale(KubeKind::StatefulSets, NS, "db", 2)
        .await
        .unwrap();
    common::eventually("db wants 2", || async {
        let o = ops.get(KubeKind::StatefulSets, NS, "db").await.unwrap();
        o.row.columns["ready"].ends_with("/2")
    })
    .await;
    ops.scale(KubeKind::StatefulSets, NS, "db", 1)
        .await
        .unwrap();
    assert!(matches!(
        ops.scale(KubeKind::Pods, NS, "ticker", 2).await,
        Err(Error::Invalid(_))
    ));
}

#[tokio::test]
async fn restart_stamps_the_template_and_records_events() {
    let ops = common::ops();
    ops.restart(KubeKind::Deployments, NS, "web").await.unwrap();
    let o = ops.get(KubeKind::Deployments, NS, "web").await.unwrap();
    assert!(
        o.yaml.contains("kubectl.kubernetes.io/restartedAt"),
        "{}",
        o.yaml
    );
    common::eventually("a ScalingReplicaSet event", || async {
        let ev = ops.events(KubeKind::Deployments, NS, "web").await.unwrap();
        ev.iter().any(|e| e.reason == "ScalingReplicaSet")
    })
    .await;
}

#[tokio::test]
async fn delete_removes_the_object() {
    let ops = common::ops();
    let name = format!("tmp-{}", std::process::id());
    let cms: kube::Api<ConfigMap> = kube::Api::namespaced(common::tunnel_client(), NS);
    let cm: ConfigMap =
        serde_json::from_value(serde_json::json!({"metadata": {"name": name}})).unwrap();
    cms.create(&PostParams::default(), &cm).await.unwrap();
    ops.delete(KubeKind::ConfigMaps, NS, &name).await.unwrap();
    assert!(matches!(
        ops.get(KubeKind::ConfigMaps, NS, &name).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn logs_and_exec_default_to_the_first_container() {
    use futures_util::StreamExt;
    let ops = common::ops();
    let opts = LogOptions {
        follow: false,
        tail: Some(3),
        ..LogOptions::default()
    };
    let lines: Vec<_> = ops.logs(NS, "ticker", &opts).collect().await;
    assert!(!lines.is_empty());
    for l in lines {
        assert!(
            l.unwrap().text.starts_with("tick"),
            "side container's logs leaked in"
        );
    }
    let exec = ExecOptions {
        cmd: Some(vec!["cat".into(), "/proc/1/cmdline".into()]),
        ..ExecOptions::default()
    };
    let mut session = ops.exec_start(NS, "ticker", &exec).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = session.output.next().await {
        out.extend(chunk.unwrap());
    }
    assert!(
        String::from_utf8_lossy(&out).contains("tick"),
        "{}",
        String::from_utf8_lossy(&out)
    );
}

#[tokio::test]
async fn exec_resizes_and_reports_the_exit_code() {
    use futures_util::StreamExt;
    let ops = common::ops();
    let opts = ExecOptions {
        cmd: Some(vec!["sh".into()]),
        container: Some("main".into()),
        ..ExecOptions::default()
    };
    let mut s = ops.exec_start(NS, "ticker", &opts).await.unwrap();
    s.resize.send((123, 45)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.input.send(b"stty size; exit 3\n".to_vec()).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = timeout(WAIT, s.output.next()).await.unwrap() {
        out.extend(chunk.unwrap());
    }
    assert!(String::from_utf8_lossy(&out).contains("45 123"));
    assert_eq!(timeout(WAIT, s.exit_code).await.unwrap().unwrap(), Some(3));
}

#[tokio::test]
async fn a_narrow_token_gets_the_rbac_reason() {
    let ops = fleetops_kube::KubeClient::new(common::tunnel_client_with("narrow-token"));
    ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    match ops.list(KubeKind::Deployments, Some(NS)).await {
        Err(Error::Forbidden(m)) => assert!(m.contains("deployments"), "{m}"),
        other => panic!("expected forbidden, got {other:?}"),
    }
}

#[tokio::test]
async fn a_kubeconfig_environment_is_up_with_its_nodes() {
    use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion};
    let p = fleetops_kube::kubeconfig::prepare(&common::read("kubeconfig"), None).unwrap();
    let f = fleetops_kube::env::KubeEnvFactory::new(Default::default());
    let env = f
        .build(&EnvSpec {
            id: 9,
            name: "kind".into(),
            kind: EnvKind::KubeConfig,
            url: p.summary.server.clone(),
            credential: Some(Credential::Kubeconfig(p.yaml)),
        })
        .unwrap();
    match env.status().await {
        EnvStatus::Up {
            version: EnvVersion::Kube(v),
        } => {
            assert!(v.nodes >= Some(1) && v.nodes_ready >= Some(1), "{v:?}");
            assert!(v.git_version.starts_with('v'));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_namespace_scoped_token_still_reports_a_version() {
    // The narrow ServiceAccount cannot list nodes; the cluster is still up.
    let ops = fleetops_kube::KubeClient::new(common::tunnel_client_with("narrow-token"));
    let v = ops.version().await.unwrap();
    assert!(v.git_version.starts_with('v'));
    assert_eq!((v.nodes, v.nodes_ready), (None, None));
}

/// A one-context token kubeconfig for the kind cluster.
fn token_kubeconfig(token: &str) -> String {
    use base64::Engine;
    let ca = base64::engine::general_purpose::STANDARD
        .encode(std::fs::read(common::dir().join("ca.crt")).unwrap());
    format!(
        "apiVersion: v1\nkind: Config\ncurrent-context: t\nclusters:\n- name: t\n  cluster: {{server: '{}', certificate-authority-data: '{ca}'}}\ncontexts:\n- name: t\n  context: {{cluster: t, user: t}}\nusers:\n- name: t\n  user: {{token: '{token}'}}\n",
        common::read("server")
    )
}

#[tokio::test]
async fn the_create_check_rejects_bad_credentials() {
    let good = fleetops_kube::kubeconfig::prepare(&token_kubeconfig(&common::read("token")), None)
        .unwrap();
    fleetops_kube::kubeconfig::check(&good.yaml).await.unwrap();
    // /version answers anonymously, so only an authenticated call catches this.
    let bad =
        fleetops_kube::kubeconfig::prepare(&token_kubeconfig("not-a-real-token"), None).unwrap();
    let e = fleetops_kube::kubeconfig::check(&bad.yaml)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("credentials rejected"), "{e}");
}

use common::kube_client;
use fleetops_core::kube::{ApplyAction, ApplyMode, WatchEvent};
use futures_util::StreamExt;

fn uniq(prefix: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        % 1_000_000_000;
    format!("{prefix}-{n}")
}

/// The next watch event, failing the test on an error or a timeout.
async fn next_event(w: &mut fleetops_core::streams::BoxStream<WatchEvent>) -> WatchEvent {
    timeout(WAIT, w.next()).await.unwrap().unwrap().unwrap()
}

#[tokio::test]
async fn a_watch_sees_a_snapshot_then_add_and_delete() {
    let ops = common::ops();
    let mut w = ops.watch(KubeKind::ConfigMaps, Some(NS));
    let first = next_event(&mut w).await;
    assert!(matches!(first, WatchEvent::Snapshot { .. }), "{first:?}");
    let name = uniq("watch");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let mut cm = ConfigMap::default();
    cm.metadata.name = Some(name.clone());
    cms.create(&PostParams::default(), &cm).await.unwrap();
    loop {
        match next_event(&mut w).await {
            WatchEvent::Added { row } if row.name == name => break,
            _ => continue,
        }
    }
    cms.delete(&name, &Default::default()).await.unwrap();
    loop {
        match next_event(&mut w).await {
            WatchEvent::Deleted { name: n, .. } if n == name => break,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn apply_creates_then_is_unchanged_and_a_dry_run_writes_nothing() {
    let ops = common::ops();
    let name = uniq("apply");
    let yaml =
        format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\ndata:\n  a: \"1\"\n");
    let dry = ops.apply(&yaml, NS, ApplyMode::DryRun).await.unwrap();
    assert_eq!(dry[0].action, Some(ApplyAction::Created), "{dry:?}");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    assert!(
        cms.get_opt(&name).await.unwrap().is_none(),
        "a dry run writes nothing"
    );
    let r = ops
        .apply(&yaml, NS, ApplyMode::Commit { force: false })
        .await
        .unwrap();
    assert_eq!(r[0].action, Some(ApplyAction::Created), "{:?}", r[0].error);
    let again = ops
        .apply(&yaml, NS, ApplyMode::Commit { force: false })
        .await
        .unwrap();
    assert_eq!(again[0].action, Some(ApplyAction::Unchanged), "{again:?}");
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_field_owned_by_another_manager_conflicts_until_forced() {
    let ops = common::ops();
    let name = uniq("owned");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let theirs = serde_json::json!({"apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": name}, "data": {"k": "theirs"}});
    cms.patch(
        &name,
        &kube::api::PatchParams::apply("someone-else"),
        &kube::api::Patch::Apply(&theirs),
    )
    .await
    .unwrap();
    let ours =
        format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\ndata:\n  k: ours\n");
    let r = ops
        .apply(&ours, NS, ApplyMode::Commit { force: false })
        .await
        .unwrap();
    assert_eq!(
        r[0].error.as_ref().map(|e| e.code.as_str()),
        Some("conflict"),
        "{r:?}"
    );
    let forced = ops
        .apply(&ours, NS, ApplyMode::Commit { force: true })
        .await
        .unwrap();
    assert_eq!(
        forced[0].action,
        Some(ApplyAction::Configured),
        "{forced:?}"
    );
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_stale_edit_is_a_conflict() {
    let ops = common::ops();
    let name = uniq("stale");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let mut cm = ConfigMap::default();
    cm.metadata.name = Some(name.clone());
    cms.create(&PostParams::default(), &cm).await.unwrap();
    let opened = ops
        .get_editable(KubeKind::ConfigMaps, NS, &name)
        .await
        .unwrap();
    let mut changed = cms.get(&name).await.unwrap();
    changed.data = Some([("x".to_string(), "1".to_string())].into());
    cms.replace(&name, &PostParams::default(), &changed)
        .await
        .unwrap();
    let edit = format!("{}data:\n  y: \"2\"\n", opened.yaml);
    let r = ops
        .replace(
            KubeKind::ConfigMaps,
            NS,
            &name,
            &edit,
            &opened.resource_version,
            ApplyMode::Commit { force: false },
        )
        .await;
    assert!(matches!(r, Err(Error::Conflict(_))), "{r:?}");
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn editing_a_secret_label_keeps_its_values_and_never_shows_them() {
    use k8s_openapi::api::core::v1::Secret;
    let ops = common::ops();
    let name = uniq("sec");
    let secrets: Api<Secret> = Api::namespaced(kube_client(), NS);
    let mut s = Secret::default();
    s.metadata.name = Some(name.clone());
    s.string_data = Some([("password".to_string(), "hunter2".to_string())].into());
    secrets.create(&PostParams::default(), &s).await.unwrap();
    let opened = ops
        .get_editable(KubeKind::Secrets, NS, &name)
        .await
        .unwrap();
    assert!(
        !opened.yaml.contains("aHVudGVyMg==") && opened.yaml.contains(MASK),
        "{}",
        opened.yaml
    );
    let edit = opened
        .yaml
        .replacen("metadata:\n", "metadata:\n  labels:\n    team: a\n", 1);
    let r = ops
        .replace(
            KubeKind::Secrets,
            NS,
            &name,
            &edit,
            &opened.resource_version,
            ApplyMode::Commit { force: false },
        )
        .await
        .unwrap();
    assert!(!r.after.unwrap().contains("aHVudGVyMg=="));
    let live = secrets.get(&name).await.unwrap();
    assert_eq!(live.data.unwrap()["password"].0, b"hunter2");
    assert_eq!(live.metadata.labels.unwrap()["team"], "a");
    secrets.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_narrow_token_cannot_create_a_cluster_role() {
    let ops = fleetops_kube::KubeClient::new(common::tunnel_client_with("narrow-token"));
    let yaml = "apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: fleetops-it-nope\nrules: []\n";
    let r = ops
        .apply(yaml, NS, ApplyMode::Commit { force: false })
        .await
        .unwrap();
    let e = r[0].error.as_ref().expect("forbidden");
    assert_eq!(e.code, "forbidden", "{e:?}");
    assert!(e.message.contains("forbidden"), "{}", e.message);
}
