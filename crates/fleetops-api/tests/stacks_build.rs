mod common;

use common::git::*;
use common::*;
use fleetops_api::stacks::git::SyncTrigger;
use fleetops_core::streams::{BuildContext, BuildSpec};
use fleetops_git::testing::ServerOpts;
use serde_json::{json, Value};

const BUILD: &str = r#"
services:
  api:
    build:
      context: ./app
      args: { V: "1" }
      target: prod
"#;

fn builds(g: &GitApp) -> Vec<BuildSpec> {
    g.fake.streams.lock().unwrap().builds.clone()
}

fn tar_paths(spec: &BuildSpec) -> Vec<String> {
    let BuildContext::Tar(t) = &spec.context else {
        panic!("not a tar: {:?}", spec.context);
    };
    let mut a = tar::Archive::new(t.as_slice());
    a.entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect()
}

async fn plan(g: &GitApp, from: Value) -> Value {
    g.send(req(
        "POST",
        &format!("{STACKS}/shop/plan"),
        Some(from),
        Some(&g.admin),
    ))
    .await
    .json
}

/// Plans `from` and deploys it; returns the finished task.
async fn deploy(g: &GitApp, from: Value) -> Value {
    let p = plan(g, from.clone()).await;
    let token = p["plan"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("no plan: {p}"))
        .to_string();
    let d = g
        .send(req(
            "POST",
            &format!("{STACKS}/shop/deploy"),
            Some(json!({"from": from, "plan_token": token})),
            Some(&g.admin),
        ))
        .await;
    assert_eq!(d.status, axum::http::StatusCode::ACCEPTED, "{:?}", d.json);
    g.wait_task(d.json["task_id"].as_str().unwrap()).await
}

/// The task's progress messages, as text.
fn log(g: &GitApp, task: &Value) -> String {
    let t = g.state.tasks.get(task["id"].as_str().unwrap()).unwrap();
    serde_json::to_string(&t.feed().backlog).unwrap()
}

fn draft() -> Value {
    json!({"source": "draft"})
}

/// A git stack whose repo holds `BUILD` and app files; synced.
async fn git_build_app(opts: ServerOpts, msg: &str) -> GitApp {
    let g = git_app(Some(("manual", false)), opts).await;
    g.repo.commit(
        &[
            ("compose.yaml", BUILD),
            ("app/Dockerfile", "FROM alpine AS prod\n"),
            ("app/main.txt", msg),
        ],
        "one",
    );
    g.sync(SyncTrigger::Manual(1)).await;
    g
}

async fn save_draft(g: &GitApp, compose: &str, variables: &[&str]) {
    let r = g
        .send(req(
            "PUT",
            &format!("{STACKS}/shop/draft"),
            Some(json!({"compose": compose, "variables": variables})),
            Some(&g.admin),
        ))
        .await;
    assert_eq!(r.status, axum::http::StatusCode::NO_CONTENT, "{:?}", r.json);
}

fn image_of(g: &GitApp, service: &str) -> String {
    g.fake
        .state
        .lock()
        .unwrap()
        .containers
        .iter()
        .find(|c| c.name == format!("shop-{service}-1"))
        .unwrap()
        .image
        .clone()
}

fn pos(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .position(|c| c.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix} in {calls:?}"))
}

#[tokio::test]
async fn git_stack_builds_before_creating_containers() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    let b = builds(&g);
    assert_eq!(b.len(), 1);
    let spec = &b[0];
    let tag = spec.tag.clone().unwrap();
    assert!(tag.starts_with("shop-api:"), "{tag}");
    assert_eq!(spec.args["V"], "1");
    assert_eq!(spec.target.as_deref(), Some("prod"));
    assert_eq!(spec.labels["io.fleetops.stack.project"], "shop");
    assert_eq!(spec.labels["io.fleetops.stack.service"], "api");
    let paths = tar_paths(spec);
    assert!(
        paths.contains(&"Dockerfile".to_string()) && paths.contains(&"main.txt".to_string()),
        "{paths:?}"
    );
    let calls = g.fake.calls();
    assert!(
        pos(&calls, "build ") < pos(&calls, "create_raw:shop-api-1"),
        "{calls:?}"
    );
    assert_eq!(image_of(&g, "api"), tag);
    let rec = g.store.revision_builds(g.stack.id, 1).await.unwrap();
    assert_eq!(rec["api"].image, tag);
    assert_eq!(rec["api"].tree.len(), 40);
}

#[tokio::test]
async fn inline_stack_with_a_context_is_blocked_with_exact_copy() {
    let g = git_app(None, ServerOpts::default()).await;
    save_draft(&g, "services:\n  api:\n    build: ./api\n", &[]).await;
    let p = plan(&g, draft()).await;
    assert!(p["plan"].is_null());
    assert_eq!(
        p["issues"],
        json!([{"path": "services.api.build", "message": "service api: a build context needs a git stack; use dockerfile_inline for a Dockerfile without files"}])
    );
}

const INLINE: &str = "services:\n  web:\n    build:\n      dockerfile_inline: \"FROM alpine\\nRUN echo ${MSG}\"\n      args: { T: \"${TOKEN}\" }\n";

#[tokio::test]
async fn inline_dockerfile_builds_on_an_inline_stack() {
    let g = git_app(None, ServerOpts::default()).await;
    save_draft(&g, INLINE, &["MSG=hi", "TOKEN=t"]).await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    let b = builds(&g);
    assert_eq!(b.len(), 1);
    assert_eq!(
        b[0].context,
        BuildContext::Dockerfile(b"FROM alpine\nRUN echo hi".to_vec())
    );
    assert!(b[0].tag.as_deref().unwrap().starts_with("shop-web:"));
}

#[tokio::test]
async fn present_image_is_not_rebuilt() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let tag = image_of(&g, "api");
    // Only the environment changes: the service is recreated, the build
    // hash and so the tag stay, and the image is already there.
    g.repo.commit(
        &[(
            "compose.yaml",
            &format!("{BUILD}    environment: {{ A: \"2\" }}\n"),
        )],
        "env",
    );
    g.sync(SyncTrigger::Manual(1)).await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    assert_eq!(builds(&g).len(), 1);
    assert_eq!(image_of(&g, "api"), tag);
}

#[tokio::test]
async fn failed_build_changes_nothing() {
    let g = git_app(None, ServerOpts::default()).await;
    save_draft(&g, "services:\n  web:\n    image: nginx:1.27\n", &[]).await;
    deploy(&g, draft()).await;
    let before = g.fake.calls().len();
    let containers = g.fake.state.lock().unwrap().containers.clone();
    save_draft(
        &g,
        "services:\n  web:\n    build:\n      dockerfile_inline: \"FROM alpine\\nRUN fail\"\n",
        &[],
    )
    .await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "failed", "{t}");
    assert!(
        t["error"].as_str().unwrap().contains("build shop-web:"),
        "{t}"
    );
    let after = g.fake.state.lock().unwrap().containers.clone();
    assert_eq!(
        containers
            .iter()
            .map(|c| (&c.name, &c.state))
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|c| (&c.name, &c.state))
            .collect::<Vec<_>>()
    );
    assert!(!g.fake.calls()[before..]
        .iter()
        .any(|c| c.starts_with("create_raw")));
}

#[tokio::test]
async fn build_arg_never_in_plan_or_log() {
    let g = git_app(None, ServerOpts::default()).await;
    save_draft(&g, INLINE, &["MSG=hi", "TOKEN=Zq9-arg-value"]).await;
    let p = plan(&g, draft()).await;
    let t = deploy(&g, draft()).await;
    assert!(!p.to_string().contains("Zq9-arg-value"), "{p}");
    assert!(!t.to_string().contains("Zq9-arg-value"), "{t}");
    let text = log(&g, &t);
    assert!(text.contains("building shop-web:"), "{text}");
    assert!(!text.contains("Zq9-arg-value"), "{text}");
    let labels = &g.fake.state.lock().unwrap().containers[0].labels;
    assert!(!format!("{labels:?}").contains("Zq9-arg-value"));
}

#[tokio::test]
async fn two_services_with_one_context_get_their_own_images() {
    let g = git_app(Some(("manual", false)), ServerOpts::default()).await;
    g.repo.commit(
        &[
            (
                "compose.yaml",
                "services:\n  a: { build: ./app }\n  b: { build: ./app }\n",
            ),
            ("app/Dockerfile", "FROM alpine\n"),
        ],
        "one",
    );
    g.sync(SyncTrigger::Manual(1)).await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    let tags: Vec<String> = builds(&g).iter().map(|b| b.tag.clone().unwrap()).collect();
    assert_eq!(tags.len(), 2);
    assert!(
        tags[0].starts_with("shop-a:") && tags[1].starts_with("shop-b:"),
        "{tags:?}"
    );
}

/// Deploys the code `msg` as a new commit; returns the api image.
async fn deploy_code(g: &GitApp, msg: &str) -> String {
    g.repo.commit(&[("app/main.txt", msg)], msg);
    g.sync(SyncTrigger::Manual(1)).await;
    let t = deploy(g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    image_of(g, "api")
}

fn forget_image(g: &GitApp, tag: &str) {
    g.fake.state.lock().unwrap().images.remove(tag);
}

#[tokio::test]
async fn rollback_reuses_a_present_image() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    let two = deploy_code(&g, "two").await;
    assert_ne!(one, two);
    let t = deploy(&g, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    assert_eq!(builds(&g).len(), 2);
    assert_eq!(image_of(&g, "api"), one);
}

#[tokio::test]
async fn rollback_fetches_a_missing_commit() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    deploy_code(&g, "two").await;
    forget_image(&g, &one);
    let _ = std::fs::remove_dir_all(
        fleetops_api::stacks::git::cache_dir(&g.state, g.stack.id).unwrap(),
    );
    let t = deploy(&g, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    let b = builds(&g);
    assert_eq!(b.len(), 3);
    assert_eq!(b[2].tag.as_deref(), Some(one.as_str()));
}

#[tokio::test]
async fn rollback_blocked_when_commit_cannot_be_fetched() {
    let g = git_build_app(
        ServerOpts {
            v0_only: true,
            ..Default::default()
        },
        "one",
    )
    .await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    let c1 = g.git().await.last_fetched_commit.unwrap();
    g.repo.force_reset(
        &[
            ("compose.yaml", BUILD),
            ("app/Dockerfile", "FROM alpine AS prod\n"),
            ("app/main.txt", "rewritten"),
        ],
        "rewritten",
    );
    g.sync(SyncTrigger::Manual(1)).await;
    deploy(&g, draft()).await;
    forget_image(&g, &one);
    let _ = std::fs::remove_dir_all(
        fleetops_api::stacks::git::cache_dir(&g.state, g.stack.id).unwrap(),
    );
    let p = plan(&g, json!({"source": "revision", "revision": 1})).await;
    assert!(p["plan"].is_null(), "{p}");
    assert_eq!(
        p["issues"][0]["message"],
        format!(
            "the image built for revision 1 is gone and commit {} cannot be fetched; deploy from the branch instead",
            &c1[..7]
        )
    );
}

#[tokio::test]
async fn detached_rollback_blocked_when_image_gone() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    g.store.detach_stack_git(g.stack.id).await.unwrap();
    let p = plan(&g, json!({"source": "revision", "revision": 1})).await;
    assert!(p["plan"].is_object(), "the image is still there: {p}");
    forget_image(&g, &one);
    // Make the rollback recreate the service: the running container uses
    // another image.
    deploy_detached_other(&g).await;
    let p = plan(&g, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(
        p["issues"][0]["message"],
        "the image built for revision 1 is gone and this stack no longer has a git source"
    );
}

async fn deploy_detached_other(g: &GitApp) {
    save_draft(g, "services:\n  api:\n    image: nginx:1.27\n", &[]).await;
    let t = deploy(g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
}

#[tokio::test]
async fn cleanup_keeps_current_and_previous() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    let two = deploy_code(&g, "two").await;
    let three = deploy_code(&g, "three").await;
    let images: Vec<String> = g
        .fake
        .state
        .lock()
        .unwrap()
        .images
        .keys()
        .cloned()
        .collect();
    assert!(
        images.contains(&two) && images.contains(&three),
        "{images:?}"
    );
    assert!(!images.contains(&one), "{images:?}");
}

#[tokio::test]
async fn cleanup_refusal_is_logged_not_fatal() {
    let g = git_build_app(ServerOpts::default(), "one").await;
    deploy(&g, draft()).await;
    let one = image_of(&g, "api");
    deploy_code(&g, "two").await;
    *g.fake.fail_on.lock().unwrap() = Some("rmi".into());
    g.repo.commit(&[("app/main.txt", "three")], "three");
    g.sync(SyncTrigger::Manual(1)).await;
    let t = deploy(&g, draft()).await;
    assert_eq!(t["status"], "succeeded", "{t}");
    let text = log(&g, &t);
    assert!(
        text.contains(&format!("could not remove image {one}")),
        "{text}"
    );
    assert!(g.fake.state.lock().unwrap().images.contains_key(&one));
}
