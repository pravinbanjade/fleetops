mod common;

use base64::Engine;
use common::{
    fake_swarm::{connected_swarm as swarm, with_custom, Swarm},
    *,
};
use fleetops_core::perm::{Row, Verb};
use serde_json::{json, Value};

const E: &str = "/api/v1/envs/prod/swarm";

async fn send(sw: &Swarm, method: &str, path: &str, body: Option<Value>) -> Resp {
    sw.t.send(req(method, path, body, Some(&sw.admin))).await
}

#[tokio::test]
async fn secret_create_never_echoes_the_value() {
    let sw = swarm().await;
    let r = send(
        &sw,
        "POST",
        &format!("{E}/secrets"),
        Some(json!({"name": "db_password", "value": "s3cret-value"})),
    )
    .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(r.json, json!({"id": "secret-new"}));
    let body = sw.daemon.body_of("/secrets/create");
    assert_eq!(body["Name"], "db_password");
    assert_eq!(
        body["Data"],
        base64::engine::general_purpose::STANDARD.encode("s3cret-value")
    );
    let rows = sw.t.store.list_audit(1000, None).await.unwrap();
    assert!(rows.iter().any(|a| a.action == "swarm.secret.create"));
    assert!(!rows
        .iter()
        .any(|a| a.detail.as_ref().is_some_and(|d| d.to_string().contains("s3cret"))));
    let r = send(
        &sw,
        "POST",
        &format!("{E}/secrets"),
        Some(json!({"name": "big", "value": "x".repeat(512_001)})),
    )
    .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .ends_with("a secret value is at most 500 KiB"));
    let r = send(&sw, "DELETE", &format!("{E}/secrets/sec1"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(sw.t.audit_actions().await.contains(&"swarm.secret.remove".to_string()));
}

#[tokio::test]
async fn config_content_needs_reveal() {
    let (sw, bob) = with_custom(&[(Row::SwarmConfigs, Verb::View)]).await;
    let r =
        sw.t.send(req("GET", &format!("{E}/configs/cfg1/data"), None, Some(&bob)))
            .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
    let r = send(&sw, "GET", &format!("{E}/configs/cfg1/data"), None).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json, json!({"text": "server {}", "size": 9}));
    assert!(sw.t.audit_actions().await.contains(&"swarm.config.reveal".to_string()));
    let r = send(&sw, "GET", &format!("{E}/configs/nope/data"), None).await;
    assert_eq!(r.status, 404, "{:?}", r.json);
}

#[tokio::test]
async fn config_create_and_remove() {
    let sw = swarm().await;
    let r = send(
        &sw,
        "POST",
        &format!("{E}/configs"),
        Some(json!({"name": "nginx_conf", "value": "server {}"})),
    )
    .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(sw.daemon.body_of("/configs/create")["Name"], "nginx_conf");
    let r = send(&sw, "DELETE", &format!("{E}/configs/cfg2"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
}

#[tokio::test]
async fn swarm_networks_create_and_remove() {
    let sw = swarm().await;
    let r = send(
        &sw,
        "POST",
        &format!("{E}/networks"),
        Some(json!({"name": "backend", "attachable": true, "encrypted": true, "subnet": "10.20.0.0/24"})),
    )
    .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(r.json["id"], "net-new");
    let body = sw.daemon.body_of("/networks/create");
    assert_eq!(body["Driver"], "overlay");
    assert_eq!(body["Attachable"], true);
    assert!(body["Options"].get("encrypted").is_some(), "{body}");
    assert_eq!(body["IPAM"]["Config"][0]["Subnet"], "10.20.0.0/24");
    // A local bridge is not the swarm's to remove.
    let r = send(&sw, "DELETE", &format!("{E}/networks/b"), None).await;
    assert_eq!(r.status, 404, "{:?}", r.json);
    let r = send(&sw, "DELETE", &format!("{E}/networks/net1"), None).await;
    assert_eq!(r.status, 204, "{:?}", r.json);
    assert!(sw.t.audit_actions().await.contains(&"swarm.network.remove".to_string()));
}

#[tokio::test]
async fn object_writes_need_their_cells() {
    let (sw, bob) = with_custom(&[
        (Row::SwarmSecrets, Verb::View),
        (Row::SwarmConfigs, Verb::View),
        (Row::Networks, Verb::View),
    ])
    .await;
    for what in ["secrets", "configs"] {
        let r =
            sw.t.send(req(
                "POST",
                &format!("{E}/{what}"),
                Some(json!({"name": "x", "value": "y"})),
                Some(&bob),
            ))
            .await;
        assert_eq!(r.status, 403, "{what}: {:?}", r.json);
    }
    let r =
        sw.t.send(req(
            "POST",
            &format!("{E}/networks"),
            Some(json!({"name": "x"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, 403, "{:?}", r.json);
}
