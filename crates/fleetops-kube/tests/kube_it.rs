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
