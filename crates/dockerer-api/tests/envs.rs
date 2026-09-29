mod common;

use common::*;
use dockerer_api::envs::bootstrap_local;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn bootstrap_registers_reachable_local_once() {
    let app = app().await;
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert_eq!(rec.unwrap().name, "local");
    assert!(app.state.envs.get("local").is_ok());
    let again = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(again.is_none());
    assert_eq!(app.store.list_environments().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bootstrap_skips_unreachable_endpoint() {
    let app = app().await;
    app.fake.reachable.store(false, Ordering::SeqCst);
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(rec.is_none());
    assert!(app.store.list_environments().await.unwrap().is_empty());
}
