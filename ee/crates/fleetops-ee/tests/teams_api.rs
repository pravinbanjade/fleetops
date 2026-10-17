mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_core::now_unix;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tokio::sync::broadcast::Receiver;

const DAY: i64 = 86_400;

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

struct Fx {
    ee: Ee,
    admin: Session,
    bob: Session,
    bob_id: i64,
    carol_id: i64,
    env: i64,
    kube: i64,
}

/// A licensed server with users bob and carol and environments "e"
/// (Docker) and "k" (Kubernetes), stored but never connected.
async fn fx() -> Fx {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let r = ee
        .install(&admin, &license(&k, now_unix() + 30 * DAY, None, &[]))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let bob_id = ee.user(&admin, "bob").await.json()["id"].as_i64().unwrap();
    let carol_id = ee.user(&admin, "carol").await.json()["id"]
        .as_i64()
        .unwrap();
    let env = ee
        .store
        .create_environment("e", "docker-local", "unix:///x", 0)
        .await
        .unwrap()
        .id;
    let kube = ee
        .store
        .create_environment("k", "kube-config", "", 0)
        .await
        .unwrap()
        .id;
    let bob = ee.login("bob").await;
    Fx {
        ee,
        admin,
        bob,
        bob_id,
        carol_id,
        env,
        kube,
    }
}

impl Fx {
    async fn call(&self, method: &str, uri: &str, b: Option<Value>) -> Resp {
        self.ee
            .send(method, uri, b.and_then(body), Some(&self.admin))
            .await
    }

    async fn team(&self, name: &str) -> i64 {
        let r = self
            .call("POST", "/api/v1/ee/teams", Some(json!({"name": name})))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        r.json()["id"].as_i64().unwrap()
    }

    async fn add(&self, team: i64, user: i64) -> Resp {
        self.call(
            "PUT",
            &format!("/api/v1/ee/teams/{team}/members/{user}"),
            None,
        )
        .await
    }

    async fn grant(&self, team: i64, env: i64, g: Value) -> Resp {
        self.call(
            "PUT",
            &format!("/api/v1/ee/teams/{team}/access/{env}"),
            Some(g),
        )
        .await
    }

    async fn bob_envs(&self) -> Vec<String> {
        let r = self
            .ee
            .send("GET", "/api/v1/envs", None, Some(&self.bob))
            .await;
        r.json()
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect()
    }

    async fn unlicense(&self) {
        let r = self.call("DELETE", "/api/v1/ee/license", None).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        assert_eq!(r.json()["status"], "none");
    }

    fn signals(&self) -> Receiver<i64> {
        self.ee.state.hubs.access().subscribe()
    }
}

/// The user ids signalled since `rx` subscribed.
fn drained(rx: &mut Receiver<i64>) -> BTreeSet<i64> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[tokio::test]
async fn admin_builds_a_team_and_member_sees_the_env() {
    let f = fx().await;
    let t = f.team("ops").await;
    assert_eq!(f.add(t, f.bob_id).await.status, StatusCode::NO_CONTENT);
    let r = f.grant(t, f.env, json!({"role": "viewer"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["env_name"], "e");
    assert_eq!(f.bob_envs().await, ["e"]);

    let r = f.call("GET", &format!("/api/v1/ee/teams/{t}"), None).await;
    assert_eq!(r.json()["members"][0]["username"], "bob");
    assert_eq!(r.json()["access"][0]["role"], "viewer");
    let r = f.call("GET", "/api/v1/ee/teams", None).await;
    assert_eq!(
        r.json(),
        json!([{"id": t, "name": "ops", "description": null, "members": 1, "environments": 1}])
    );

    let r = f
        .call(
            "DELETE",
            &format!("/api/v1/ee/teams/{t}/members/{}", f.bob_id),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(f.bob_envs().await.is_empty());
}

#[tokio::test]
async fn non_admins_get_403() {
    let f = fx().await;
    let t = f.team("ops").await;
    for (m, uri) in [
        ("GET", "/api/v1/ee/teams".to_string()),
        ("POST", "/api/v1/ee/teams".to_string()),
        ("GET", format!("/api/v1/ee/teams/{t}")),
        ("PATCH", format!("/api/v1/ee/teams/{t}")),
        ("DELETE", format!("/api/v1/ee/teams/{t}")),
        ("PUT", format!("/api/v1/ee/teams/{t}/members/{}", f.bob_id)),
        (
            "DELETE",
            format!("/api/v1/ee/teams/{t}/members/{}", f.bob_id),
        ),
        ("PUT", format!("/api/v1/ee/teams/{t}/access/{}", f.env)),
        ("DELETE", format!("/api/v1/ee/teams/{t}/access/{}", f.env)),
    ] {
        let r =
            f.ee.send(
                m,
                &uri,
                body(json!({"name": "x", "role": "viewer"})),
                Some(&f.bob),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{m} {uri}");
    }
}

#[tokio::test]
async fn review_focus_direct_grant_survives_team_removal() {
    let f = fx().await;
    let r = f
        .call(
            "PUT",
            &format!("/api/v1/users/{}/access/{}", f.bob_id, f.env),
            Some(json!({"role": "viewer"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let t = f.team("ops").await;
    f.add(t, f.bob_id).await;
    f.grant(t, f.env, json!({"role": "operator"})).await;
    let r = f
        .call("DELETE", &format!("/api/v1/ee/teams/{t}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.bob_envs().await, ["e"]);
    let r = f
        .call("GET", &format!("/api/v1/users/{}/access", f.bob_id), None)
        .await;
    assert_eq!(r.json()[0]["role"], "viewer");
}

#[tokio::test]
async fn review_focus_duplicate_name_is_409() {
    let f = fx().await;
    f.team("Payments").await;
    let r = f
        .call(
            "POST",
            "/api/v1/ee/teams",
            Some(json!({"name": " payments "})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
    let other = f.team("ops").await;
    let r = f
        .call(
            "PATCH",
            &format!("/api/v1/ee/teams/{other}"),
            Some(json!({"name": "PAYMENTS"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text);
}

#[tokio::test]
async fn bad_names_are_400() {
    let f = fx().await;
    let r = f
        .call("POST", "/api/v1/ee/teams", Some(json!({"name": "a/b"})))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text);
}

#[tokio::test]
async fn unknown_user_env_or_team_is_404() {
    let f = fx().await;
    let t = f.team("ops").await;
    assert_eq!(f.add(t, 999).await.status, StatusCode::NOT_FOUND);
    assert_eq!(f.add(999, f.bob_id).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        f.grant(t, 999, json!({"role": "viewer"})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call("GET", "/api/v1/ee/teams/999", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call("DELETE", "/api/v1/ee/teams/999", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn frozen_refuses_additions() {
    let f = fx().await;
    let t = f.team("ops").await;
    f.grant(t, f.env, json!({"role": "viewer"})).await;
    f.unlicense().await;
    let refused = [
        f.call("POST", "/api/v1/ee/teams", Some(json!({"name": "new"})))
            .await,
        f.call(
            "PATCH",
            &format!("/api/v1/ee/teams/{t}"),
            Some(json!({"name": "renamed"})),
        )
        .await,
        f.add(t, f.bob_id).await,
        f.grant(t, f.kube, json!({"role": "viewer"})).await,
        f.grant(t, f.env, json!({"role": "operator"})).await,
    ];
    for r in refused {
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
        assert_eq!(r.json()["code"], "license_required");
    }
}

#[tokio::test]
async fn frozen_allows_removals() {
    let f = fx().await;
    let t = f.team("ops").await;
    f.add(t, f.bob_id).await;
    f.add(t, f.carol_id).await;
    f.grant(t, f.env, json!({"role": "operator"})).await;
    f.grant(
        t,
        f.kube,
        json!({"role": "viewer", "namespaces": ["a", "b"]}),
    )
    .await;
    f.unlicense().await;

    assert_eq!(
        f.bob_envs().await,
        ["e", "k"],
        "team access outlives the license"
    );
    let r = f.grant(t, f.env, json!({"role": "viewer"})).await;
    assert_eq!(r.status, StatusCode::OK, "lower: {}", r.text);
    let r = f
        .grant(t, f.kube, json!({"role": "viewer", "namespaces": ["a"]}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "narrow: {}", r.text);
    let r = f
        .call(
            "DELETE",
            &format!("/api/v1/ee/teams/{t}/members/{}", f.carol_id),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = f
        .call(
            "DELETE",
            &format!("/api/v1/ee/teams/{t}/access/{}", f.kube),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(f.bob_envs().await, ["e"]);
    let r = f
        .call("DELETE", &format!("/api/v1/ee/teams/{t}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(f.bob_envs().await.is_empty());
}

#[tokio::test]
async fn review_focus_frozen_resave_same_grant_ok() {
    let f = fx().await;
    let t = f.team("ops").await;
    f.grant(t, f.kube, json!({"role": "operator", "namespaces": ["a"]}))
        .await;
    f.unlicense().await;
    let r = f
        .grant(t, f.kube, json!({"role": "operator", "namespaces": ["a"]}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

#[tokio::test]
async fn signals_affected_users() {
    let f = fx().await;
    let t = f.team("ops").await;
    let mut rx = f.signals();
    f.add(t, f.bob_id).await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id]));
    f.add(t, f.bob_id).await;
    assert!(drained(&mut rx).is_empty(), "adding again changes nothing");
    f.add(t, f.carol_id).await;
    drained(&mut rx);

    f.grant(t, f.env, json!({"role": "viewer"})).await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id, f.carol_id]));
    f.call(
        "DELETE",
        &format!("/api/v1/ee/teams/{t}/access/{}", f.env),
        None,
    )
    .await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id, f.carol_id]));

    f.call(
        "DELETE",
        &format!("/api/v1/ee/teams/{t}/members/{}", f.carol_id),
        None,
    )
    .await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.carol_id]));
}

#[tokio::test]
async fn review_focus_delete_signals_former_members() {
    let f = fx().await;
    let t = f.team("ops").await;
    f.add(t, f.bob_id).await;
    f.add(t, f.carol_id).await;
    let mut rx = f.signals();
    f.call("DELETE", &format!("/api/v1/ee/teams/{t}"), None)
        .await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id, f.carol_id]));
}

#[tokio::test]
async fn audit_rows() {
    let f = fx().await;
    let t = f.team("ops").await;
    f.call(
        "PATCH",
        &format!("/api/v1/ee/teams/{t}"),
        Some(json!({"name": "ops2", "description": "on call"})),
    )
    .await;
    f.add(t, f.bob_id).await;
    f.grant(t, f.env, json!({"role": "viewer"})).await;
    f.call(
        "DELETE",
        &format!("/api/v1/ee/teams/{t}/access/{}", f.env),
        None,
    )
    .await;
    f.call(
        "DELETE",
        &format!("/api/v1/ee/teams/{t}/members/{}", f.bob_id),
        None,
    )
    .await;
    f.call("DELETE", &format!("/api/v1/ee/teams/{t}"), None)
        .await;

    let rows = f.ee.store.list_audit(500, None).await.unwrap();
    let team_rows: Vec<_> = rows
        .iter()
        .filter(|e| e.action.starts_with("team."))
        .collect();
    let mut actions: Vec<_> = team_rows.iter().map(|e| e.action.as_str()).collect();
    actions.sort();
    assert_eq!(
        actions,
        [
            "team.access.grant",
            "team.access.revoke",
            "team.create",
            "team.delete",
            "team.member.add",
            "team.member.remove",
            "team.update",
        ]
    );
    for e in &team_rows {
        assert_eq!(e.target_type.as_deref(), Some("team"));
        assert_eq!(e.target_id.as_deref(), Some(t.to_string().as_str()));
        if e.action.starts_with("team.access.") {
            assert_eq!(e.environment_id, Some(f.env), "{}", e.action);
        }
    }
}

#[tokio::test]
async fn effective_access_lists_sources() {
    let f = fx().await;
    let r = f
        .call(
            "PUT",
            &format!("/api/v1/users/{}/access/{}", f.bob_id, f.kube),
            Some(json!({"role": "viewer", "namespaces": ["a"]})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let t = f.team("ops").await;
    f.add(t, f.bob_id).await;
    f.grant(t, f.kube, json!({"role": "operator", "namespaces": ["b"]}))
        .await;
    f.grant(t, f.env, json!({"role": "viewer"})).await;

    let r = f
        .call(
            "GET",
            &format!("/api/v1/ee/users/{}/access", f.bob_id),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(
        r.json(),
        json!([
            {"env_id": f.env, "env_name": "e", "env_kind": "docker-local", "role": "viewer", "namespaces": null,
             "sources": [{"team_id": t, "team_name": "ops", "role": "viewer", "namespaces": null}]},
            {"env_id": f.kube, "env_name": "k", "env_kind": "kube-config", "role": "operator", "namespaces": ["a", "b"],
             "sources": [
                {"team_id": null, "team_name": null, "role": "viewer", "namespaces": ["a"]},
                {"team_id": t, "team_name": "ops", "role": "operator", "namespaces": ["b"]}
             ]}
        ])
    );
    let r = f.call("GET", "/api/v1/ee/users/999/access", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = f.call("GET", "/api/v1/ee/users/1/access", None).await;
    assert_eq!(r.json(), json!([]), "admins have everything already");
}

#[tokio::test]
async fn env_team_access_lists_teams_and_members() {
    let f = fx().await;
    let t = f.team("payments").await;
    f.add(t, f.bob_id).await;
    f.add(t, f.carol_id).await;
    f.grant(
        t,
        f.kube,
        json!({"role": "operator", "namespaces": ["pay"]}),
    )
    .await;
    f.team("idle").await;

    let r = f.call("GET", "/api/v1/ee/envs/k/teams", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(
        r.json(),
        json!([{"team_id": t, "team_name": "payments", "role": "operator", "namespaces": ["pay"],
                "members": ["bob", "carol"]}])
    );
    assert_eq!(
        f.call("GET", "/api/v1/ee/envs/e/teams", None).await.json(),
        json!([])
    );
    assert_eq!(
        f.call("GET", "/api/v1/ee/envs/nope/teams", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let r =
        f.ee.send("GET", "/api/v1/ee/envs/k/teams", None, Some(&f.bob))
            .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}
