mod common;

use axum::http::StatusCode;
use common::git::*;
use common::*;
use fleetops_api::stacks::git::{
    cache_dir, sync_stack, SyncOutcome, SyncTrigger, BUSY, UNREACHABLE,
};
use fleetops_git::testing::ServerOpts;
use fleetops_store::TokenChange;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn first_sync_auto_deploys_as_system() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let (n, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    let rev = g.store.revision(g.stack.id, n).await.unwrap();
    assert_eq!(rev.status, "deployed");
    assert_eq!(rev.git_commit.as_deref(), Some(head.as_str()));
    assert_eq!(rev.created_by, None);
    let draft = g.store.draft(g.stack.id).await.unwrap().unwrap();
    assert_eq!(draft.compose, WEB_A);
    assert_eq!(g.git().await.pending_commit, None);
    let audit = g.store.list_audit(1000, None).await.unwrap();
    let deploy = audit.iter().find(|e| e.action == "stack.deploy").unwrap();
    assert_eq!(deploy.user_id, None);
    assert_eq!(deploy.detail.as_ref().unwrap()["by"], "system (git)");
    assert_eq!(deploy.detail.as_ref().unwrap()["commit"], head.as_str());
    let sync = audit.iter().find(|e| e.action == "stack.git.sync").unwrap();
    assert_eq!(sync.detail.as_ref().unwrap()["trigger"], "poll");
    assert_eq!(sync.detail.as_ref().unwrap()["outcome"], "deploying");
}

#[tokio::test]
async fn without_auto_deploy_sets_pending() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let out = g.sync(SyncTrigger::Manual(1)).await;
    assert_eq!(
        out,
        SyncOutcome::Pending {
            commit: head.clone()
        }
    );
    let git = g.git().await;
    assert_eq!(git.pending_commit.as_deref(), Some(head.as_str()));
    assert_eq!(git.last_fetched_commit.as_deref(), Some(head.as_str()));
    assert_eq!(git.last_sync_error, None);
    assert!(g.revisions().await.is_empty());
    assert_eq!(
        g.store.draft(g.stack.id).await.unwrap().unwrap().compose,
        WEB_A
    );
}

#[tokio::test]
async fn same_file_new_commit_is_up_to_date() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let (_, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    let readme = g.repo.commit(&[("README.md", "hello\n")], "docs");
    let out = g.sync(SyncTrigger::Poll).await;
    assert_eq!(out, SyncOutcome::UpToDate { commit: readme });
    assert_eq!(g.git().await.pending_commit, None);
    assert_eq!(g.revisions().await.len(), 1);
}

#[tokio::test]
async fn failed_auto_deploy_not_retried_for_same_commit() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    *g.fake.fail_on.lock().unwrap() = Some("create_raw".into());
    let (_, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "failed", "{task:?}");
    let error = g.git().await.last_sync_error.unwrap();
    assert!(error.starts_with("auto-deploy of "), "{error}");
    let head = g.git().await.last_fetched_commit.unwrap();
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending { commit: head }
    );
    assert_eq!(g.revisions().await.len(), 1);
    assert_eq!(
        g.git().await.last_sync_error.as_deref(),
        Some(error.as_str())
    );
    *g.fake.fail_on.lock().unwrap() = None;
    g.repo.commit(&[("compose.yaml", WEB_B)], "two");
    let (_, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    assert_eq!(g.git().await.last_sync_error, None);
}

#[tokio::test]
async fn failed_auto_deploy_not_retried_by_a_commit_that_keeps_the_file() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    *g.fake.fail_on.lock().unwrap() = Some("create_raw".into());
    let (_, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "failed", "{task:?}");
    let error = g.git().await.last_sync_error.unwrap();
    *g.fake.fail_on.lock().unwrap() = None;
    let readme = g.repo.commit(&[("README.md", "hello\n")], "docs");
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending {
            commit: readme.clone()
        }
    );
    assert_eq!(g.revisions().await.len(), 1);
    let git = g.git().await;
    assert_eq!(git.pending_commit.as_deref(), Some(readme.as_str()));
    assert_eq!(git.last_sync_error.as_deref(), Some(error.as_str()));
}

#[tokio::test]
async fn attaching_auto_deploys_a_draft_that_matches_the_repo() {
    let g = git_app(None, ServerOpts::default()).await;
    g.store
        .save_draft(g.stack.id, WEB_A, &[], Some(1), 1)
        .await
        .unwrap();
    g.store
        .set_stack_git(
            g.stack.id,
            &settings(&g.server, "poll", true),
            TokenChange::Keep,
            Some("s"),
            None,
            1,
        )
        .await
        .unwrap();
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let (_, task) = g.sync_and_deploy(SyncTrigger::Settings(1)).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
}

#[tokio::test]
async fn rollback_pauses_and_resume_deploys() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "a");
    g.sync_and_deploy(SyncTrigger::Poll).await;
    let b = g.repo.commit(&[("compose.yaml", WEB_B)], "b");
    let (n, _) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(n, 2);
    // Roll back to revision 1 through the API.
    let from = json!({"source": "revision", "revision": 1});
    let p = g
        .send(req(
            "POST",
            &format!("{STACKS}/shop/plan"),
            Some(from.clone()),
            Some(&g.admin),
        ))
        .await;
    let token = p.json["plan"]["token"].as_str().unwrap().to_string();
    let d = g
        .send(req(
            "POST",
            &format!("{STACKS}/shop/deploy"),
            Some(json!({"from": from, "plan_token": token})),
            Some(&g.admin),
        ))
        .await;
    assert_eq!(d.status, StatusCode::ACCEPTED, "{:?}", d.json);
    let task = g.wait_task(d.json["task_id"].as_str().unwrap()).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    assert!(g.git().await.paused);
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending { commit: b.clone() }
    );
    assert_eq!(g.revisions().await.len(), 3);
    g.store.set_git_paused(g.stack.id, false).await.unwrap();
    let (n, task) = g.sync_and_deploy(SyncTrigger::Resume(1)).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    let rev = g.store.revision(g.stack.id, n).await.unwrap();
    assert_eq!(rev.git_commit.as_deref(), Some(b.as_str()));
    assert_eq!(rev.compose, WEB_B);
}

#[tokio::test]
async fn fetch_error_recorded_and_pending_kept() {
    let g = git_app(
        Some(("manual", false)),
        ServerOpts {
            token: Some("t0ken".into()),
            ..Default::default()
        },
    )
    .await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.sync(SyncTrigger::Manual(1)).await;
    g.store
        .set_stack_git(
            g.stack.id,
            &settings(&g.server, "manual", false),
            TokenChange::Clear,
            None,
            None,
            2,
        )
        .await
        .unwrap();
    let out = g.sync(SyncTrigger::Manual(1)).await;
    assert_eq!(
        out,
        SyncOutcome::Failed {
            error: "the repository needs an access token".into()
        }
    );
    let git = g.git().await;
    assert_eq!(git.pending_commit.as_deref(), Some(head.as_str()));
    assert_eq!(
        git.last_sync_error.as_deref(),
        Some("the repository needs an access token")
    );
}

#[tokio::test]
async fn unreachable_env_keeps_pending() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.fake.reachable.store(false, Ordering::SeqCst);
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending { commit: head }
    );
    assert_eq!(g.git().await.last_sync_error.as_deref(), Some(UNREACHABLE));
    assert!(g.revisions().await.is_empty());
}

#[tokio::test]
async fn invalid_compose_in_repo() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    g.repo.commit(
        &[("compose.yaml", "services:\n  web:\n    imgae: nginx\n")],
        "typo",
    );
    match g.sync(SyncTrigger::Poll).await {
        SyncOutcome::Failed { error } => assert!(error.contains("services.web.imgae"), "{error}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(g.revisions().await.is_empty());
    assert!(g.git().await.pending_commit.is_some());
}

#[tokio::test]
async fn concurrent_triggers_coalesce() {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    let g = git_app(
        Some(("manual", false)),
        ServerOpts {
            hold: Some(hold.clone()),
            ..Default::default()
        },
    )
    .await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let first = tokio::spawn({
        let (state, stack) = (g.state.clone(), g.stack.clone());
        async move {
            sync_stack(&state, &stack, SyncTrigger::Webhook)
                .await
                .unwrap()
        }
    });
    eventually("the first fetch to start", || {
        g.server.fetches.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(g.sync(SyncTrigger::Webhook).await, SyncOutcome::Queued);
    assert_eq!(g.sync(SyncTrigger::Poll).await, SyncOutcome::Queued);
    let newer = g.repo.commit(&[("compose.yaml", WEB_B)], "two");
    hold.add_permits(100);
    let out = tokio::time::timeout(Duration::from_secs(30), first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out, SyncOutcome::Pending { commit: newer });
    assert_eq!(g.server.fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn auto_deploy_retried_once_the_environment_is_back() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.fake.reachable.store(false, Ordering::SeqCst);
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending {
            commit: head.clone()
        }
    );
    // A second pass while still down keeps the reason in view.
    g.sync(SyncTrigger::Poll).await;
    assert_eq!(g.git().await.last_sync_error.as_deref(), Some(UNREACHABLE));
    g.fake.reachable.store(true, Ordering::SeqCst);
    let (n, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
    let rev = g.store.revision(g.stack.id, n).await.unwrap();
    assert_eq!(rev.git_commit.as_deref(), Some(head.as_str()));
}

#[tokio::test]
async fn auto_deploy_retried_after_a_running_deploy() {
    let g = git_app(Some(("poll", true)), ServerOpts::default()).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    // Someone else's deploy holds the stack.
    assert!(g.state.stack_locks.lock().unwrap().insert(g.stack.id));
    assert_eq!(
        g.sync(SyncTrigger::Poll).await,
        SyncOutcome::Pending { commit: head }
    );
    assert_eq!(g.git().await.last_sync_error.as_deref(), Some(BUSY));
    g.state.stack_locks.lock().unwrap().remove(&g.stack.id);
    let (_, task) = g.sync_and_deploy(SyncTrigger::Poll).await;
    assert_eq!(task["status"], "succeeded", "{task:?}");
}

#[tokio::test]
async fn detach_during_a_sync_keeps_the_draft_and_drops_the_cache() {
    let g = git_app(
        Some(("manual", false)),
        ServerOpts {
            delay: Duration::from_millis(300),
            ..Default::default()
        },
    )
    .await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let running = tokio::spawn({
        let (state, stack) = (g.state.clone(), g.stack.clone());
        async move { sync_stack(&state, &stack, SyncTrigger::Manual(1)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The route removes the cache too, but that can land before the
    // fetch writes it.
    g.store.detach_stack_git(g.stack.id).await.unwrap();
    let out = running.await.unwrap().unwrap();
    assert!(matches!(out, SyncOutcome::Failed { .. }), "{out:?}");
    assert_eq!(
        g.store.draft(g.stack.id).await.unwrap().unwrap().compose,
        ""
    );
    assert!(!cache_dir(&g.state, g.stack.id).unwrap().exists());
}
