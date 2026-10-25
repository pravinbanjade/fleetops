mod common;

use common::{
    fake_swarm::{connected_swarm as swarm, with_custom_in, Swarm},
    *,
};
use fleetops_core::perm::{Row, Verb};
use serde_json::{json, Value};

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
async fn scoped_stack_grants_are_refused_on_a_swarm() {
    let (sw, bob) = with_custom_in(&[(Row::Stacks, Verb::View)], Some(&["blog"])).await;
    let r = sw.t.send(req("GET", STACKS, None, Some(&bob))).await;
    assert_eq!(r.status, 403, "{:?}", r.json);
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
