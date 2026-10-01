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
