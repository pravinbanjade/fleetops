//! A grant limited to stacks on a swarm node's pages (swarm spec §6): the
//! containers of its stacks only, nothing node-wide.

mod common;

use common::{
    fake_swarm::{container_json, with_custom_in, Swarm},
    *,
};
use fleetops_core::perm::{Row, Verb};
use serde_json::{json, Value};

const CELLS: &[(Row, Verb)] = &[
    (Row::Containers, Verb::View),
    (Row::Containers, Verb::Operate),
    (Row::Containers, Verb::Shell),
    (Row::Containers, Verb::Reveal),
    (Row::Images, Verb::View),
    (Row::Volumes, Verb::View),
    (Row::Networks, Verb::View),
    (Row::Stacks, Verb::View),
];

const N: &str = "/api/v1/envs/prod@n2";

async fn scoped() -> (Swarm, Session) {
    let (sw, bob) = with_custom_in(CELLS, Some(&["blog"])).await;
    sw.daemon
        .containers
        .lock()
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(container_json("beef", "shop"));
    (sw, bob)
}

async fn bob_get(sw: &Swarm, bob: &Session, path: &str) -> Resp {
    sw.t.send(req("GET", path, None, Some(bob))).await
}

fn sub(n: u32, topic: &str, id: Option<&str>, opts: Value) -> Value {
    json!({"op": "sub", "sub": n, "topic": topic, "env": "prod@n2", "id": id, "opts": opts})
}

#[tokio::test]
async fn node_view_lists_only_stack_containers() {
    let (sw, bob) = scoped().await;
    let r = bob_get(&sw, &bob, &format!("{N}/containers")).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    let ids: Vec<&str> = r
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["c0ffee"]);
    let r =
        sw.t.send(req(
            "GET",
            &format!("{N}/containers"),
            None,
            Some(&sw.admin),
        ))
        .await;
    assert_eq!(r.json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn node_view_container_routes_follow_stacks() {
    let (sw, bob) = scoped().await;
    assert_eq!(
        bob_get(&sw, &bob, &format!("{N}/containers/c0ffee"))
            .await
            .status,
        200
    );
    // Another stack's container reads exactly as a missing one.
    for id in ["beef", "nope"] {
        let r = bob_get(&sw, &bob, &format!("{N}/containers/{id}")).await;
        assert_eq!(r.status, 404, "{id}: {:?}", r.json);
        let m = r.json["message"].as_str().unwrap();
        assert!(
            m.ends_with(&format!("no such container '{id}'")),
            "{id}: {m}"
        );
    }
    let r = bob_get(&sw, &bob, &format!("{N}/containers/beef/files?path=/")).await;
    assert_eq!(r.status, 404, "{:?}", r.json);
    let r =
        sw.t.send(req(
            "POST",
            &format!("{N}/containers/beef/restart"),
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 404, "{:?}", r.json);

    let mut ws = Ws::open(sw.t.serve().await, &bob).await;
    ws.send(sub(1, "logs", Some("c0ffee"), json!({"follow": false})))
        .await;
    ws.until(|v| is(v, 1, "data")).await;
    ws.send(sub(2, "logs", Some("beef"), json!({"follow": false})))
        .await;
    let err = ws.until(|v| v["sub"] == 2 && v["type"] == "error").await;
    assert!(
        err.to_string().contains("no such container 'beef'"),
        "{err}"
    );
    ws.send(sub(3, "exec", Some("beef"), json!({"cmd": ["sh"]})))
        .await;
    let err = ws.until(|v| v["sub"] == 3 && v["type"] == "error").await;
    assert!(
        err.to_string().contains("no such container 'beef'"),
        "{err}"
    );
    ws.send(sub(4, "stats", Some("beef"), json!({}))).await;
    ws.until(|v| v["sub"] == 4 && v["type"] == "error").await;
    assert!(sw
        .t
        .audit_actions()
        .await
        .contains(&"access.denied".to_string()));
}

#[tokio::test]
async fn node_view_hides_images_volumes_networks() {
    let (sw, bob) = scoped().await;
    for page in ["images", "volumes", "networks", "df", "compose-projects"] {
        let r = bob_get(&sw, &bob, &format!("{N}/{page}")).await;
        assert_eq!(r.status, 403, "{page}: {:?}", r.json);
        assert!(
            r.json["message"]
                .as_str()
                .unwrap()
                .ends_with("only access to the whole swarm covers this"),
            "{page}: {:?}",
            r.json
        );
    }
    let r =
        sw.t.send(req(
            "POST",
            &format!("{N}/containers"),
            Some(json!({"image": "x"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}

#[tokio::test]
async fn node_view_events_follow_stacks() {
    let (sw, bob) = scoped().await;
    let ev = |id: &str, stack: &str| {
        json!({"Type": "container", "Action": "start", "Actor": {"ID": id,
               "Attributes": {"name": id, "com.docker.stack.namespace": stack}},
               "scope": "local", "time": 1_792_576_800, "timeNano": 1_792_576_800_000_000_000i64})
    };
    *sw.daemon.events.lock().unwrap() = vec![
        ev("beef", "shop"),
        json!({"Type": "image", "Action": "pull", "Actor": {"ID": "nginx", "Attributes": {"name": "nginx"}},
               "scope": "local", "time": 1_792_576_800, "timeNano": 1_792_576_800_000_000_000i64}),
        ev("c0ffee", "blog"),
    ];
    let mut ws = Ws::open(sw.t.serve().await, &bob).await;
    ws.send(sub(1, "events", None, json!({}))).await;
    let first = ws.until(|v| is(v, 1, "data")).await;
    assert_eq!(first["data"]["id"], "c0ffee", "{first}");
    let more = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        ws.until(|v| is(v, 1, "data")),
    )
    .await;
    assert!(more.is_err(), "{more:?}");
}

/// Engine info counts only the stacks' containers, and no images.
#[tokio::test]
async fn node_view_info_counts_only_stack_containers() {
    let (sw, bob) = scoped().await;
    let r = bob_get(&sw, &bob, &format!("{N}/info")).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["Containers"], 1, "{:?}", r.json);
    assert_eq!(r.json["ContainersRunning"], 1);
    assert!(r.json["Images"].is_null(), "{:?}", r.json);
    assert_eq!(r.json["ServerVersion"], "27.3.1");
    let r =
        sw.t.send(req("GET", &format!("{N}/info"), None, Some(&sw.admin)))
            .await;
    assert_eq!(r.json["Containers"], 2);
    assert_eq!(r.json["Images"], 12);
}

/// Reveal held on a stack shows its containers' env values.
#[tokio::test]
async fn node_view_reveal_follows_stacks() {
    let (sw, bob) = scoped().await;
    let r = bob_get(&sw, &bob, &format!("{N}/containers/c0ffee")).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["Config"]["Env"], json!(["TOKEN=t0p"]));
}
