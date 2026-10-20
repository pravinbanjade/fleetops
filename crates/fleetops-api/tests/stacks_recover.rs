mod common;

use common::fake_docker::summary;
use common::*;
use fleetops_api::stacks::recover_all;
use fleetops_core::compose::docker::{LABEL_PROJECT, LABEL_REVISION, LABEL_SERVICE};
use std::sync::atomic::Ordering;

const COMPOSE: &str = "services:\n  web: {image: x}\n";

/// A stack whose revision 3 was cut off mid-deploy: the old web container
/// set aside, the new one created, the revision still `deploying`.
async fn interrupted(app: &TestApp) -> i64 {
    let env = app.store.list_environments().await.unwrap()[0].id;
    let st = app
        .store
        .create_stack(env, "shop", COMPOSE, &[], None, 1)
        .await
        .unwrap();
    for n in 1..=3 {
        app.store
            .create_revision(st.id, COMPOSE, &[], "deploy", None, None, None, n)
            .await
            .unwrap();
        match n {
            1 => app.store.finish_revision(st.id, 1, None).await.unwrap(),
            2 => app
                .store
                .finish_revision(st.id, 2, Some("x"))
                .await
                .unwrap(),
            _ => {}
        }
    }
    let labels = |rev: &str| {
        [
            (LABEL_PROJECT, "shop"),
            (LABEL_SERVICE, "web"),
            (LABEL_REVISION, rev),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    };
    let mut old = summary(
        "old1",
        "shop-web-1-fleetops-old-3-run",
        "exited",
        Some("shop"),
    );
    old.labels = labels("1");
    let mut new = summary("n3", "shop-web-1", "running", Some("shop"));
    new.labels = labels("3");
    app.fake.add(old);
    app.fake.add(new);
    st.id
}

#[tokio::test]
async fn interrupted_deploy_restored() {
    let app = app().await;
    app.with_local().await;
    let st = interrupted(&app).await;
    recover_all(&app.state).await;
    let web = app.fake.container("shop-web-1").unwrap();
    assert_eq!((web.id.as_str(), web.state.as_str()), ("old1", "running"));
    assert!(app.fake.container("n3").is_none());
    let r = app.store.revision(st, 3).await.unwrap();
    assert_eq!(
        (r.status.as_str(), r.error.as_deref()),
        ("failed", Some("interrupted"))
    );
}

#[tokio::test]
async fn locked_deploy_left_alone() {
    let app = app().await;
    app.with_local().await;
    let st = interrupted(&app).await;
    app.state.stack_locks.lock().unwrap().insert(st);
    recover_all(&app.state).await;
    assert_eq!(app.store.revision(st, 3).await.unwrap().status, "deploying");
    assert_eq!(app.fake.container("shop-web-1").unwrap().id, "n3");
}

#[tokio::test]
async fn unreachable_env_skipped_then_recovered() {
    let app = app().await;
    app.with_local().await;
    let st = interrupted(&app).await;
    app.fake.reachable.store(false, Ordering::SeqCst);
    recover_all(&app.state).await;
    assert_eq!(app.store.revision(st, 3).await.unwrap().status, "deploying");
    app.fake.reachable.store(true, Ordering::SeqCst);
    recover_all(&app.state).await;
    assert_eq!(app.store.revision(st, 3).await.unwrap().status, "failed");
    assert_eq!(app.fake.container("shop-web-1").unwrap().id, "old1");
}
