mod common;

use common::{
    fake_swarm::{connected_swarm as swarm, connected_swarm_with, with_custom_in, Swarm},
    *,
};
use fleetops_api::stacks::Timing;
use fleetops_api::ApiConfig;
use fleetops_core::perm::{Row, Verb};
use serde_json::{json, Value};
use std::time::Duration;

async fn send(sw: &Swarm, method: &str, path: &str, body: Option<Value>) -> Resp {
    sw.t.send(req(method, path, body, Some(&sw.admin))).await
}

const STACKS: &str = "/api/v1/envs/prod/stacks";

const FILE: &str = r#"
services:
  web:
    image: nginx:1.27
    deploy:
      replicas: 3
      placement: { constraints: ["node.role == worker"] }
    configs: [nginx]
  api:
    image: api:1
configs:
  nginx: { content: "server {}" }
"#;

async fn create(sw: &Swarm, name: &str, compose: &str) {
    let r = send(
        sw,
        "POST",
        STACKS,
        Some(json!({"name": name, "compose": compose, "variables": []})),
    )
    .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
}

async fn plan(sw: &Swarm, name: &str) -> Value {
    let r = send(
        sw,
        "POST",
        &format!("{STACKS}/{name}/plan"),
        Some(json!({"source": "draft"})),
    )
    .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    r.json
}

fn service<'a>(plan: &'a Value, name: &str) -> &'a Value {
    plan["plan"]["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {plan}"))
}

#[tokio::test]
async fn stacks_work_on_a_swarm() {
    let sw = swarm().await;
    create(&sw, "blog", FILE).await;
    let r = send(&sw, "GET", &format!("{STACKS}/blog"), None).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let r = send(
        &sw,
        "PUT",
        &format!("{STACKS}/blog/draft"),
        Some(json!({"compose": FILE, "variables": ["A=1"]})),
    )
    .await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    let r = send(&sw, "GET", STACKS, None).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json[0]["name"], "blog");
    let r = send(
        &sw,
        "POST",
        &format!("{STACKS}/validate"),
        Some(json!({"name": "blog", "compose": FILE})),
    )
    .await;
    assert_eq!(r.json["issues"], json!([]), "{:?}", r.json);
    let r = send(
        &sw,
        "POST",
        &format!("{STACKS}/validate"),
        Some(json!({"name": "blog", "compose": "services:\n  web:\n    image: x\n    build: .\n"})),
    )
    .await;
    assert_eq!(
        r.json["issues"][0]["message"],
        "Swarm cannot build images; push the image to a registry and name it in image:"
    );
}

#[tokio::test]
async fn plan_on_a_swarm() {
    let sw = swarm().await;
    create(&sw, "blog", FILE).await;
    let p = plan(&sw, "blog").await;
    assert_eq!(p["issues"], json!([]), "{p}");
    let web = service(&p, "web");
    assert_eq!(web["action"], "update");
    assert_eq!(web["reasons"], json!(["not deployed by FleetOps"]));
    assert_eq!(service(&p, "api")["action"], "create");
    let configs = p["plan"]["configs"].as_array().unwrap();
    assert_eq!(configs.len(), 1, "{configs:?}");
    assert_eq!(configs[0]["action"], "create");
    let name = configs[0]["name"].as_str().unwrap();
    assert!(
        name.starts_with("blog_nginx_") && name.len() == "blog_nginx_".len() + 10,
        "{name}"
    );
    assert_eq!(p["plan"]["blocking"], json!([]));
}

#[tokio::test]
async fn a_protected_service_blocks_the_plan() {
    let sw = swarm().await;
    sw.daemon.add_fleetops_server("n1");
    create(
        &sw,
        "fleetops",
        "services:\n  fleetops:\n    image: registry.example/ops/fleetops-server:2\n",
    )
    .await;
    let p = plan(&sw, "fleetops").await;
    assert_eq!(
        p["plan"]["blocking"],
        json!(["service 'fleetops_fleetops' runs FleetOps; change it outside FleetOps"])
    );
}

// Deploys.

const SHOP: &str = r#"
services:
  web:
    image: nginx:1.27
    deploy: { replicas: 3 }
    secrets: [db]
secrets:
  db: { environment: DB_PW }
"#;

const SHOP_STACKS: &str = "/api/v1/envs/prod/stacks/shop";

async fn fast() -> Swarm {
    connected_swarm_with(fast_config()).await
}

async fn shop(sw: &Swarm, compose: &str, pw: &str) {
    let r = send(sw, "GET", SHOP_STACKS, None).await;
    let body = json!({"compose": compose, "variables": [format!("DB_PW={pw}")]});
    let r = if r.status == 404 {
        let mut b = body.clone();
        b["name"] = json!("shop");
        send(sw, "POST", STACKS, Some(b)).await
    } else {
        send(sw, "PUT", &format!("{SHOP_STACKS}/draft"), Some(body)).await
    };
    assert!(r.status == 201 || r.status == 204, "{:?}", r.json);
}

/// Plans `from`, deploys it, and waits for the task: (task, its messages).
async fn deploy_from(sw: &Swarm, from: Value) -> (Value, Vec<Value>) {
    let r = send(
        sw,
        "POST",
        &format!("{SHOP_STACKS}/plan"),
        Some(from.clone()),
    )
    .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let token = r.json["plan"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("{:?}", r.json))
        .to_string();
    let r = send(
        sw,
        "POST",
        &format!("{SHOP_STACKS}/deploy"),
        Some(json!({"from": from, "plan_token": token})),
    )
    .await;
    assert_eq!(r.status, 202, "{:?}", r.json);
    let id = r.json["task_id"].as_str().unwrap().to_string();
    for _ in 0..2000 {
        let t = send(sw, "GET", &format!("/api/v1/tasks/{id}"), None).await;
        if t.json["status"] != "running" {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let backlog = sw.t.state.tasks.get(&id).unwrap().feed().backlog;
            return (t.json, backlog);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("task {id} never finished");
}

async fn deploy(sw: &Swarm) -> (Value, Vec<Value>) {
    deploy_from(sw, json!({"source": "draft"})).await
}

async fn revisions(sw: &Swarm) -> Vec<Value> {
    send(sw, "GET", &format!("{SHOP_STACKS}/revisions"), None)
        .await
        .json
        .as_array()
        .unwrap()
        .clone()
}

fn position(paths: &[String], needle: &str) -> usize {
    paths
        .iter()
        .position(|p| p.contains(needle))
        .unwrap_or_else(|| panic!("no {needle} in {paths:?}"))
}

#[tokio::test]
async fn swarm_deploy_creates_objects_then_services() {
    let sw = fast().await;
    shop(&sw, SHOP, "s3cr3t-value").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    let paths = sw.daemon.write_paths();
    let (n, sec, svc) = (
        position(&paths, "/networks/create"),
        position(&paths, "/secrets/create"),
        position(&paths, "/services/create"),
    );
    assert!(n < sec && sec < svc, "{paths:?}");
    let net = sw.daemon.body_of("/networks/create");
    assert_eq!(net["Name"], "shop_default");
    assert_eq!(net["Labels"]["com.docker.stack.namespace"], "shop");
    let secret = sw.daemon.body_of("/secrets/create");
    assert_eq!(secret["Labels"]["com.docker.stack.namespace"], "shop");
    let body = sw.daemon.body_of("/services/create");
    assert_eq!(body["Name"], "shop_web");
    assert_eq!(body["Labels"]["com.docker.stack.namespace"], "shop");
    assert!(
        body["Labels"]["io.fleetops.stack.spec-hash"].is_string(),
        "{body}"
    );
    let r = &body["TaskTemplate"]["ContainerSpec"]["Secrets"][0];
    assert_eq!(r["SecretName"], secret["Name"]);
    let name = secret["Name"].as_str().unwrap();
    assert!(
        name.starts_with("shop_db_") && name.len() == "shop_db_".len() + 10,
        "{name}"
    );
    let created = sw.daemon.secrets.lock().unwrap().clone();
    let id = created
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["Spec"]["Name"] == name)
        .unwrap()["ID"]
        .clone();
    assert_eq!(r["SecretID"], id);
    assert_eq!(revisions(&sw).await[0]["status"], "deployed");
    let a = sw.t.store.list_audit(100, None).await.unwrap();
    let d = a
        .iter()
        .find(|a| a.action == "stack.deploy")
        .expect("audit row");
    assert_eq!(d.result, "success");
}

#[tokio::test]
async fn a_second_deploy_of_the_same_file_changes_nothing() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    assert_eq!(deploy(&sw).await.0["status"], "succeeded");
    let before = sw.daemon.write_paths().len();
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    let after = sw.daemon.write_paths();
    assert_eq!(after.len(), before, "{:?}", &after[before..]);
}

#[tokio::test]
async fn a_changed_value_makes_a_new_secret_and_removes_the_old() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    let old = sw.daemon.body_of("/secrets/create")["Name"].clone();
    let old_id = sw
        .daemon
        .secrets
        .lock()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["Spec"]["Name"] == old)
        .unwrap()["ID"]
        .as_str()
        .unwrap()
        .to_string();
    shop(&sw, SHOP, "b").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    let new = sw.daemon.body_of("/secrets/create")["Name"].clone();
    assert_ne!(old, new);
    let paths = sw.daemon.write_paths();
    assert!(
        paths
            .iter()
            .any(|p| p.contains("/services/") && p.contains("/update")),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("DELETE") && p.contains(&format!("/secrets/{old_id}"))),
        "{paths:?}"
    );
}

#[tokio::test]
async fn a_paused_update_fails_the_deploy() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    *sw.daemon.update_outcome.lock().unwrap() = Some(("paused".into(), "task failed".into()));
    shop(&sw, &SHOP.replace("nginx:1.27", "nginx:1.28"), "a").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "failed");
    assert!(
        task["error"]
            .as_str()
            .unwrap()
            .starts_with("deploy failed: web: update paused: task failed"),
        "{task}"
    );
    assert_eq!(revisions(&sw).await[0]["status"], "failed");
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.starts_with("DELETE") && p.contains("/services/")));
}

#[tokio::test]
async fn a_rejected_task_fails_the_deploy() {
    let sw = fast().await;
    *sw.daemon.on_create_task.lock().unwrap() = Some(json!({
        "ID": "tnew", "Version": {"Index": 1}, "Slot": 1, "NodeID": "n2",
        "UpdatedAt": "2026-10-25T10:00:00Z", "DesiredState": "shutdown",
        "Status": {"State": "rejected", "Message": "preparing", "Err": "No such image: nginx:9"}
    }));
    shop(&sw, SHOP, "a").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "failed");
    assert!(
        task["error"]
            .as_str()
            .unwrap()
            .starts_with("deploy failed: web: task rejected: No such image: nginx:9"),
        "{task}"
    );
}

#[tokio::test]
async fn old_rejected_tasks_do_not_fail_a_deploy() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    let id = sw
        .daemon
        .services
        .lock()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["Spec"]["Name"] == "shop_web")
        .unwrap()["ID"]
        .as_str()
        .unwrap()
        .to_string();
    sw.daemon
        .tasks
        .lock()
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(json!({
            "ID": "old", "Version": {"Index": 1}, "ServiceID": id, "Slot": 1, "NodeID": "n2",
            "UpdatedAt": "2026-10-25T09:00:00Z", "DesiredState": "shutdown",
            "Status": {"State": "rejected", "Message": "preparing", "Err": "an old failure"}
        }));
    // The update takes a while, so the feed has shown the old task by then.
    *sw.daemon.update_outcome.lock().unwrap() = Some(("updating".into(), String::new()));
    let daemon = sw.daemon.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        daemon.set_update_state("shop_web", "completed");
    });
    shop(&sw, &SHOP.replace("nginx:1.27", "nginx:1.28"), "a").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "succeeded", "{task}");
}

#[tokio::test]
async fn a_pending_task_times_out() {
    let sw = fast().await;
    *sw.daemon.on_create_task.lock().unwrap() = Some(json!({
        "ID": "tp", "Version": {"Index": 1}, "Slot": 1,
        "UpdatedAt": "2026-10-25T10:00:00Z", "DesiredState": "running",
        "Status": {"State": "pending", "Message": "no suitable node (scheduling constraints)"}
    }));
    shop(&sw, SHOP, "a").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "failed");
    let e = task["error"].as_str().unwrap();
    assert!(
        e.starts_with("deploy failed: web: a task is still pending after"),
        "{e}"
    );
    assert!(e.contains("no suitable node"), "{e}");
}

#[tokio::test]
async fn swarm_deploy_never_echoes_secret_values() {
    let sw = fast().await;
    shop(&sw, SHOP, "s3cr3t-value").await;
    let plan = send(
        &sw,
        "POST",
        &format!("{SHOP_STACKS}/plan"),
        Some(json!({"source": "draft"})),
    )
    .await;
    assert!(!plan.json.to_string().contains("s3cr3t-value"));
    let (task, messages) = deploy(&sw).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    assert!(!format!("{task}{messages:?}").contains("s3cr3t-value"));
    let audit = sw.t.store.list_audit(100, None).await.unwrap();
    assert!(!format!("{audit:?}").contains("s3cr3t-value"));
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode("s3cr3t-value");
    for (method, path, body) in sw.daemon.writes.lock().unwrap().iter() {
        let text = body.to_string();
        assert!(!text.contains("s3cr3t-value"), "{method} {path}");
        if text.contains(&b64) {
            assert!(path.contains("/secrets/create"), "{method} {path}");
        }
    }
}

#[tokio::test]
async fn rollback_redeploys_an_earlier_revision() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    shop(&sw, &SHOP.replace("nginx:1.27", "nginx:1.28"), "a").await;
    deploy(&sw).await;
    let (task, _) = deploy_from(&sw, json!({"source": "revision", "revision": 1})).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    let body = sw.daemon.body_of("/update");
    assert_eq!(body["TaskTemplate"]["ContainerSpec"]["Image"], "nginx:1.27");
    let revs = revisions(&sw).await;
    assert_eq!(revs[0]["kind"], "rollback");
    assert_eq!(revs[0]["status"], "deployed");
}

#[tokio::test]
async fn an_interrupted_swarm_deploy_is_marked_failed() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    let stack = sw.t.store.stack_by_name(sw.env_id, "shop").await.unwrap();
    sw.t.store
        .create_revision(
            stack.id,
            SHOP,
            &[],
            "deploy",
            None,
            None,
            None,
            1,
            &[],
            &Default::default(),
        )
        .await
        .unwrap();
    fleetops_api::stacks::recover_all(&sw.t.state).await;
    let revs = revisions(&sw).await;
    assert_eq!(revs[0]["status"], "failed");
    assert_eq!(revs[0]["error"], "interrupted");
}

#[tokio::test]
async fn git_auto_deploy_works_on_a_swarm() {
    use fleetops_api::stacks::git::{sync_stack, SyncOutcome, SyncTrigger};
    use fleetops_git::testing::{GitServer, ServerOpts, TestRepo};
    let sw = connected_swarm_with(ApiConfig {
        git_allow_http: true,
        data_dir: Some(tempfile::tempdir().unwrap().keep()),
        ..fast_config()
    })
    .await;
    shop(&sw, "", "a").await;
    let repo = TestRepo::new("main");
    repo.commit(&[("compose.yaml", SHOP_PLACED)], "one");
    let server = GitServer::start(&repo.root(), ServerOpts::default()).await;
    let admin_id = send(&sw, "GET", "/api/v1/auth/me", None).await.json["user"]["id"]
        .as_i64()
        .unwrap();
    let stack = sw.t.store.stack_by_name(sw.env_id, "shop").await.unwrap();
    sw.t.store
        .set_stack_git(
            stack.id,
            &common::git::settings(&server, "poll", true),
            fleetops_store::TokenChange::Keep,
            Some("hook-secret"),
            Some(admin_id),
            1,
        )
        .await
        .unwrap();
    let stack = sw.t.store.stack_by_id(stack.id).await.unwrap();
    match sync_stack(&sw.t.state, &stack, SyncTrigger::Poll)
        .await
        .unwrap()
    {
        SyncOutcome::Deploying { task_id, .. } => {
            for _ in 0..2000 {
                let t = sw.t.state.tasks.get(&task_id).unwrap().info();
                if t.status != fleetops_api::tasks::TaskStatus::Running {
                    assert_eq!(
                        t.status,
                        fleetops_api::tasks::TaskStatus::Succeeded,
                        "{t:?}"
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        other => panic!("expected a deploy, got {other:?}"),
    }
    let body = sw.daemon.body_of("/services/create");
    assert_eq!(
        body["TaskTemplate"]["Placement"]["Constraints"],
        json!(["node.role == worker"])
    );
}

const SHOP_PLACED: &str = "services:\n  web:\n    image: nginx:1.27\n    deploy:\n      placement: { constraints: [\"node.role == worker\"] }\n";

fn fast_config() -> ApiConfig {
    ApiConfig {
        stack_timing: Timing {
            exit_check: Duration::from_millis(5),
            poll: Duration::from_millis(20),
            health_cap: Duration::from_secs(2),
            swarm_pending: Duration::from_millis(300),
        },
        swarm_task_poll: Duration::from_millis(20),
        ..Default::default()
    }
}

// Removal, actions and per-service status.

#[tokio::test]
async fn removing_a_swarm_stack_removes_its_objects() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    let before = sw.daemon.write_paths().len();
    let r = send(&sw, "DELETE", &format!("{SHOP_STACKS}?volumes=true"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    let paths = sw.daemon.write_paths()[before..].to_vec();
    assert!(paths.iter().all(|p| p.starts_with("DELETE")), "{paths:?}");
    let svc = position(&paths, "/services/");
    let sec = position(&paths, "/secrets/");
    let net = position(&paths, "/networks/");
    assert!(svc < sec && svc < net, "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("/volumes")), "{paths:?}");
    let r = send(&sw, "GET", SHOP_STACKS, None).await;
    assert_eq!(r.status, 404);
    assert!(!sw
        .daemon
        .services
        .lock()
        .unwrap()
        .to_string()
        .contains("shop_web"));
}

#[tokio::test]
async fn stack_actions_are_refused_on_a_swarm() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    let r = send(&sw, "POST", &format!("{SHOP_STACKS}/restart"), None).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(r.json["message"].as_str().unwrap().ends_with(
        "a Swarm stack's services are scaled or redeployed one by one on the Services page"
    ));
}

#[tokio::test]
async fn stack_services_show_drift() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    let r = send(&sw, "GET", &format!("{SHOP_STACKS}/services"), None).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json[0]["key"], "web");
    assert_eq!(r.json[0]["managed"], true);
    assert_eq!(r.json[0]["drift"], json!([]));
    assert_eq!(r.json[0]["service"]["name"], "shop_web");
    {
        let mut services = sw.daemon.services.lock().unwrap();
        let web = services
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["Spec"]["Name"] == "shop_web")
            .unwrap();
        web["Spec"]["Mode"]["Replicated"]["Replicas"] = json!(7);
    }
    let r = send(&sw, "GET", &format!("{SHOP_STACKS}/services"), None).await;
    assert_eq!(
        r.json[0]["drift"],
        json!(["replicas: 3 → 7"]),
        "{:?}",
        r.json
    );
}

#[tokio::test]
async fn stack_services_need_swarm_services_view() {
    let (sw, bob) = common::fake_swarm::with_custom(&[(Row::Stacks, Verb::View)]).await;
    let r = send(
        &sw,
        "POST",
        STACKS,
        Some(json!({"name": "shop", "compose": SHOP, "variables": []})),
    )
    .await;
    assert_eq!(r.status, 201);
    let r =
        sw.t.send(req(
            "GET",
            &format!("{SHOP_STACKS}/services"),
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}

#[tokio::test]
async fn an_update_that_rolls_back_after_the_first_look_fails_the_deploy() {
    let sw = fast().await;
    shop(&sw, SHOP, "a").await;
    deploy(&sw).await;
    *sw.daemon.update_outcome.lock().unwrap() = Some((
        "rollback_completed".into(),
        "rolled back: task failed".into(),
    ));
    shop(&sw, &SHOP.replace("nginx:1.27", "nginx:1.28"), "a").await;
    let (task, _) = deploy(&sw).await;
    assert_eq!(task["status"], "failed", "{task}");
    assert!(
        task["error"]
            .as_str()
            .unwrap()
            .starts_with("deploy failed: web: update rollback_completed: rolled back: task failed"),
        "{task}"
    );
}

#[tokio::test]
async fn removing_a_stack_that_runs_fleetops_is_refused() {
    let sw = fast().await;
    sw.daemon.add_fleetops_server("n1");
    create(
        &sw,
        "fleetops",
        "services:\n  fleetops:\n    image: registry.example/ops/fleetops-server:2\n",
    )
    .await;
    let before = sw.daemon.write_paths().len();
    let r = send(&sw, "DELETE", &format!("{STACKS}/fleetops"), None).await;
    assert_eq!(r.status, 409, "{:?}", r.json);
    assert!(
        r.json["message"]
            .as_str()
            .unwrap()
            .ends_with("service 'fleetops_fleetops' runs FleetOps; remove it outside FleetOps"),
        "{:?}",
        r.json
    );
    assert_eq!(sw.daemon.write_paths().len(), before);
}

// Stack scoping (swarm spec §6).

const STACK_CELLS: &[(Row, Verb)] = &[
    (Row::Stacks, Verb::View),
    (Row::Stacks, Verb::Create),
    (Row::Stacks, Verb::Operate),
    (Row::Stacks, Verb::Delete),
];

async fn bob_says(
    sw: &Swarm,
    bob: &Session,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Resp {
    sw.t.send(req(method, path, body, Some(bob))).await
}

#[tokio::test]
async fn scoped_stack_lists_and_routes() {
    let (sw, bob) = with_custom_in(STACK_CELLS, Some(&["blog", "news"])).await;
    create(&sw, "blog", FILE).await;
    create(&sw, "shop", FILE).await;
    let r = bob_says(&sw, &bob, "GET", STACKS, None).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let names: Vec<&str> = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["blog"]);
    let r = bob_says(&sw, &bob, "GET", &format!("{STACKS}/shop"), None).await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .ends_with("stack 'shop' is not in your access"));
    let r = bob_says(
        &sw,
        &bob,
        "POST",
        &format!("{STACKS}/shop/plan"),
        Some(json!({"source": "draft"})),
    )
    .await;
    assert_eq!(r.status, 403);
    let new = |n: &str| Some(json!({"name": n, "compose": FILE, "variables": []}));
    assert_eq!(
        bob_says(&sw, &bob, "POST", STACKS, new("other"))
            .await
            .status,
        403
    );
    assert_eq!(
        bob_says(&sw, &bob, "POST", STACKS, new("news"))
            .await
            .status,
        201
    );
    assert_eq!(
        bob_says(&sw, &bob, "GET", &format!("{STACKS}/blog"), None)
            .await
            .status,
        200
    );
}

const BLOG_EXTERNAL: &str = r#"
services:
  web:
    image: nginx:1.27
    secrets: [tls]
secrets:
  tls: { external: true, name: shop_tls }
"#;

fn add_shop_secret(sw: &Swarm) {
    sw.daemon.secrets.lock().unwrap().as_array_mut().unwrap().push(json!(
        {"ID": "sec9", "Version": {"Index": 1}, "Spec": {"Name": "shop_tls", "Labels": {"com.docker.stack.namespace": "shop"}}}
    ));
}

#[tokio::test]
async fn scoped_deploys_cannot_use_other_stacks_externals() {
    let (sw, bob) = with_custom_in(STACK_CELLS, Some(&["blog"])).await;
    add_shop_secret(&sw);
    create(&sw, "blog", BLOG_EXTERNAL).await;
    let r = bob_says(
        &sw,
        &bob,
        "POST",
        &format!("{STACKS}/blog/plan"),
        Some(json!({"source": "draft"})),
    )
    .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(
        r.json["plan"]["blocking"],
        json!(["external secret shop_tls does not exist or is outside your stacks"])
    );
    let p = plan(&sw, "blog").await;
    assert_eq!(p["plan"]["blocking"], json!([]), "{p}");
}

#[tokio::test]
async fn git_auto_deploys_use_the_authors_scope() {
    use fleetops_api::stacks::git::{sync_stack, SyncOutcome, SyncTrigger};
    use fleetops_git::testing::{GitServer, ServerOpts, TestRepo};
    let (sw, bob) = common::fake_swarm::with_grants_on(
        vec![common::fake_swarm::custom_grant(
            STACK_CELLS,
            Some(&["blog"]),
        )],
        ApiConfig {
            git_allow_http: true,
            data_dir: Some(tempfile::tempdir().unwrap().keep()),
            ..fast_config()
        },
    )
    .await;
    add_shop_secret(&sw);
    create(&sw, "blog", "").await;
    let bob_id = bob_says(&sw, &bob, "GET", "/api/v1/auth/me", None)
        .await
        .json["user"]["id"]
        .as_i64()
        .unwrap();
    let repo = TestRepo::new("main");
    repo.commit(&[("compose.yaml", BLOG_EXTERNAL)], "one");
    let server = GitServer::start(&repo.root(), ServerOpts::default()).await;
    let stack = sw.t.store.stack_by_name(sw.env_id, "blog").await.unwrap();
    sw.t.store
        .set_stack_git(
            stack.id,
            &common::git::settings(&server, "poll", true),
            fleetops_store::TokenChange::Keep,
            Some("hook-secret"),
            Some(bob_id),
            1,
        )
        .await
        .unwrap();
    let stack = sw.t.store.stack_by_id(stack.id).await.unwrap();
    match sync_stack(&sw.t.state, &stack, SyncTrigger::Poll)
        .await
        .unwrap()
    {
        SyncOutcome::Failed { error } => {
            assert!(
                error.contains("external secret shop_tls does not exist or is outside your stacks"),
                "{error}"
            )
        }
        other => panic!("expected a refused auto-deploy, got {other:?}"),
    }
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/create")));
}

#[tokio::test]
async fn scoped_stack_services_follow_the_stack() {
    let mut cells = STACK_CELLS.to_vec();
    cells.push((Row::SwarmServices, Verb::View));
    let (sw, bob) = with_custom_in(&cells, Some(&["blog"])).await;
    create(&sw, "blog", FILE).await;
    create(&sw, "shop", FILE).await;
    assert_eq!(
        bob_says(&sw, &bob, "GET", &format!("{STACKS}/blog/services"), None)
            .await
            .status,
        200
    );
    assert_eq!(
        bob_says(&sw, &bob, "GET", &format!("{STACKS}/shop/services"), None)
            .await
            .status,
        403
    );
}

#[tokio::test]
async fn a_scoped_plan_does_not_tell_missing_from_hidden_externals() {
    let (sw, bob) = with_custom_in(STACK_CELLS, Some(&["blog"])).await;
    add_shop_secret(&sw);
    create(
        &sw,
        "blog",
        "services:\n  web:\n    image: x\n    secrets: [a, b]\nsecrets:\n  a: { external: true, name: shop_tls }\n  b: { external: true, name: nope }\n",
    )
    .await;
    let r = bob_says(
        &sw,
        &bob,
        "POST",
        &format!("{STACKS}/blog/plan"),
        Some(json!({"source": "draft"})),
    )
    .await;
    assert_eq!(
        r.json["plan"]["blocking"],
        json!([
            "external secret shop_tls does not exist or is outside your stacks",
            "external secret nope does not exist or is outside your stacks",
        ]),
        "{:?}",
        r.json
    );
}

/// Stack `blog` on a fast git-enabled swarm, its repo naming shop's secret,
/// its git settings saved by `author`; Bob is limited to `blog`.
async fn git_blog(author: Option<&str>) -> (Swarm, Session, fleetops_store::StackRecord) {
    use fleetops_git::testing::{GitServer, ServerOpts, TestRepo};
    let (sw, bob) = common::fake_swarm::with_grants_on(
        vec![common::fake_swarm::custom_grant(
            STACK_CELLS,
            Some(&["blog"]),
        )],
        ApiConfig {
            git_allow_http: true,
            data_dir: Some(tempfile::tempdir().unwrap().keep()),
            ..fast_config()
        },
    )
    .await;
    add_shop_secret(&sw);
    create(&sw, "blog", "").await;
    let author = match author {
        Some("bob") => Some(
            bob_says(&sw, &bob, "GET", "/api/v1/auth/me", None)
                .await
                .json["user"]["id"]
                .as_i64()
                .unwrap(),
        ),
        Some(_) => Some(
            send(&sw, "GET", "/api/v1/auth/me", None).await.json["user"]["id"]
                .as_i64()
                .unwrap(),
        ),
        None => None,
    };
    let repo = TestRepo::new("main");
    repo.commit(&[("compose.yaml", BLOG_EXTERNAL)], "one");
    let server = GitServer::start(&repo.root(), ServerOpts::default()).await;
    std::mem::forget(repo);
    let stack = sw.t.store.stack_by_name(sw.env_id, "blog").await.unwrap();
    sw.t.store
        .set_stack_git(
            stack.id,
            &common::git::settings(&server, "poll", true),
            fleetops_store::TokenChange::Keep,
            Some("hook-secret"),
            author,
            1,
        )
        .await
        .unwrap();
    std::mem::forget(server);
    let stack = sw.t.store.stack_by_id(stack.id).await.unwrap();
    (sw, bob, stack)
}

#[tokio::test]
async fn a_manual_sync_uses_the_callers_scope_too() {
    use fleetops_api::stacks::git::{sync_stack, SyncOutcome, SyncTrigger};
    let (sw, bob, stack) = git_blog(Some("admin")).await;
    let bob_id = bob_says(&sw, &bob, "GET", "/api/v1/auth/me", None)
        .await
        .json["user"]["id"]
        .as_i64()
        .unwrap();
    match sync_stack(&sw.t.state, &stack, SyncTrigger::Manual(bob_id))
        .await
        .unwrap()
    {
        SyncOutcome::Failed { error } => assert!(
            error.contains("external secret shop_tls does not exist or is outside your stacks"),
            "{error}"
        ),
        other => panic!("expected a refused deploy, got {other:?}"),
    }
}

#[tokio::test]
async fn an_auto_deploy_without_an_author_stops() {
    use fleetops_api::stacks::git::{sync_stack, SyncOutcome, SyncTrigger};
    let (sw, _, stack) = git_blog(None).await;
    match sync_stack(&sw.t.state, &stack, SyncTrigger::Poll)
        .await
        .unwrap()
    {
        SyncOutcome::Failed { error } => assert!(
            error.contains("save the stack's git settings again"),
            "{error}"
        ),
        other => panic!("expected a refused auto-deploy, got {other:?}"),
    }
    assert!(!sw
        .daemon
        .write_paths()
        .iter()
        .any(|p| p.contains("/services/create")));
}

#[tokio::test]
async fn previews_follow_the_viewers_scope() {
    use common::fake_swarm::{custom_grant, with_grants};
    use fleetops_core::access::{EnvRole, Grant};
    // A Viewer of the whole swarm previews the real plan.
    let (sw, viewer) = with_grants(vec![Grant::full(EnvRole::Viewer)]).await;
    add_shop_secret(&sw);
    create(&sw, "blog", BLOG_EXTERNAL).await;
    let r =
        sw.t.send(req(
            "POST",
            &format!("{STACKS}/blog/plan"),
            Some(json!({"source": "draft"})),
            Some(&viewer),
        ))
        .await;
    assert_eq!(r.json["plan"]["blocking"], json!([]), "{:?}", r.json);
    // One limited to blog sees nothing of shop's, even in a preview.
    let (sw, viewer) = with_grants(vec![custom_grant(
        &[(Row::Stacks, Verb::View)],
        Some(&["blog"]),
    )])
    .await;
    add_shop_secret(&sw);
    create(&sw, "blog", BLOG_EXTERNAL).await;
    let r =
        sw.t.send(req(
            "POST",
            &format!("{STACKS}/blog/plan"),
            Some(json!({"source": "draft"})),
            Some(&viewer),
        ))
        .await;
    assert_eq!(
        r.json["plan"]["blocking"],
        json!(["external secret shop_tls does not exist or is outside your stacks"]),
        "{:?}",
        r.json
    );
}
