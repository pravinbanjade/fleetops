//! Custom roles and their grants (custom roles spec): storage, the license
//! feature, and assignment through the core access routes and teams.

mod common;

use axum::http::StatusCode;
use common::*;
use fleetops_core::now_unix;
use fleetops_core::perm::PermSet;
use fleetops_ee::roles::db as roles;
use serde_json::{json, Value};

const DAY: i64 = 86_400;

fn body(v: Value) -> Option<(&'static str, String)> {
    Some(("application/json", v.to_string()))
}

struct Fx {
    ee: Ee,
    admin: Session,
    bob: Session,
    bob_id: i64,
    env: i64,
    deployer: i64,
}

fn perms(v: &[&str]) -> PermSet {
    v.iter().map(|p| p.parse().unwrap()).collect()
}

async fn fx() -> Fx {
    let k = key();
    let ee = ee_app(&k, None).await;
    let admin = ee.admin().await;
    let r = ee
        .install(&admin, &license(&k, now_unix() + 30 * DAY, None, &[]))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let bob_id = ee.user(&admin, "bob").await.json()["id"].as_i64().unwrap();
    let env = ee
        .store
        .create_environment("e", "docker-local", "unix:///x", 0)
        .await
        .unwrap()
        .id;
    let deployer = roles::create(
        ee.store.pool(),
        "Deployer",
        None,
        perms(&["containers.view", "containers.operate"]),
        1,
    )
    .await
    .unwrap()
    .id;
    let bob = ee.login("bob").await;
    Fx {
        ee,
        admin,
        bob,
        bob_id,
        env,
        deployer,
    }
}

impl Fx {
    async fn call(&self, method: &str, uri: &str, b: Option<Value>) -> Resp {
        self.ee
            .send(method, uri, b.and_then(body), Some(&self.admin))
            .await
    }

    async fn set(&self, b: Value) -> Resp {
        let uri = format!("/api/v1/users/{}/access/{}", self.bob_id, self.env);
        self.call("PUT", &uri, Some(b)).await
    }

    async fn bob_access(&self) -> Value {
        let r = self
            .ee
            .send("GET", "/api/v1/envs", None, Some(&self.bob))
            .await;
        r.json()
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "e")
            .map(|e| e["access"].clone())
            .unwrap_or(Value::Null)
    }

    async fn listed(&self) -> Value {
        let uri = format!("/api/v1/users/{}/access", self.bob_id);
        self.call("GET", &uri, None).await.json()
    }
}

#[tokio::test]
async fn rbac_comes_with_any_active_license() {
    let f = fx().await;
    let r = f.call("GET", "/api/v1/edition", None).await;
    let features = r.json()["features"].clone();
    assert!(
        features.as_array().unwrap().contains(&json!("rbac")),
        "{features}"
    );
}

#[tokio::test]
async fn a_custom_role_assigned_directly_opens_the_environment() {
    let f = fx().await;
    let r = f.set(json!({"role_id": f.deployer})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["role_id"], f.deployer);
    assert_eq!(r.json()["role_name"], "Deployer");
    let a = f.bob_access().await;
    assert_eq!(a["role_name"], "Deployer");
    assert_eq!(
        a["grants"][0]["permissions"],
        json!(["containers.operate", "containers.view"])
    );
    let listed = f.listed().await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["role_name"], "Deployer");
    // The environment Access dialog lists them too (core `list_env` reads
    // this hook; Enterprise tests have no live environments to route to).
    let rows = f.ee.state.custom_roles.env_rows(f.env).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, f.bob_id);
    assert_eq!(rows[0].1.custom.as_ref().unwrap().name, "Deployer");
}

#[tokio::test]
async fn a_custom_role_through_a_team_opens_the_environment() {
    let f = fx().await;
    let r = f
        .call("POST", "/api/v1/ee/teams", Some(json!({"name": "ops"})))
        .await;
    let team = r.json()["id"].as_i64().unwrap();
    let uri = format!("/api/v1/ee/teams/{team}/members/{}", f.bob_id);
    assert_eq!(
        f.call("PUT", &uri, None).await.status,
        StatusCode::NO_CONTENT
    );
    let uri = format!("/api/v1/ee/teams/{team}/access/{}", f.env);
    let r = f
        .call("PUT", &uri, Some(json!({"role_id": f.deployer})))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["role_name"], "Deployer");
    assert_eq!(f.bob_access().await["role_name"], "Deployer");
    let r = f
        .call("GET", &format!("/api/v1/ee/teams/{team}"), None)
        .await;
    assert_eq!(r.json()["access"][0]["role_id"], f.deployer);
    // Switching the team to a built-in role replaces the custom one.
    let r = f.call("PUT", &uri, Some(json!({"role": "viewer"}))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let r = f
        .call("GET", &format!("/api/v1/ee/teams/{team}"), None)
        .await;
    assert_eq!(r.json()["access"].as_array().unwrap().len(), 1);
    assert_eq!(r.json()["access"][0]["role"], "viewer");
    assert!(r.json()["access"][0]["role_id"].is_null());
}

#[tokio::test]
async fn one_direct_grant_per_environment_either_kind() {
    let f = fx().await;
    f.set(json!({"role_id": f.deployer})).await;
    let r = f.set(json!({"role": "viewer"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let listed = f.listed().await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["role"], "viewer");
    f.set(json!({"role_id": f.deployer})).await;
    let listed = f.listed().await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["role_id"], f.deployer);
    let uri = format!("/api/v1/users/{}/access/{}", f.bob_id, f.env);
    assert_eq!(
        f.call("DELETE", &uri, None).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.listed().await, json!([]));
    assert!(f.bob_access().await.is_null());
}

#[tokio::test]
async fn an_unknown_role_is_not_found() {
    let f = fx().await;
    let r = f.set(json!({"role_id": 999})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.text);
}

#[tokio::test]
async fn unlicensed_a_custom_role_cannot_be_assigned_but_keeps_working() {
    let f = fx().await;
    f.set(json!({"role_id": f.deployer})).await;
    let r = f.call("DELETE", "/api/v1/ee/license", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(f.bob_access().await["role_name"], "Deployer");
    let other = roles::create(
        f.ee.store.pool(),
        "Other",
        None,
        perms(&["volumes.view"]),
        1,
    )
    .await
    .unwrap()
    .id;
    let r = f.set(json!({"role_id": other})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text);
    assert_eq!(r.json()["code"], "license_required");
    // The same role again is no stronger, so it is allowed.
    let r = f.set(json!({"role_id": f.deployer})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let r = f.set(json!({"role": "viewer"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
}

#[tokio::test]
async fn reconcile_keeps_the_newer_of_two_direct_grants() {
    let f = fx().await;
    let pool = f.ee.store.pool();
    f.set(json!({"role_id": f.deployer})).await;
    // As if the Community image then stored a built-in grant beside it.
    f.ee.store
        .set_binding(
            f.bob_id,
            f.env,
            fleetops_core::access::EnvRole::Viewer,
            None,
            now_unix() + 10,
        )
        .await
        .unwrap();
    assert_eq!(f.listed().await.as_array().unwrap().len(), 2);
    roles::reconcile_direct_grants(pool).await.unwrap();
    let listed = f.listed().await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["role"], "viewer");
}

#[tokio::test]
async fn deleting_a_role_removes_its_grants() {
    let f = fx().await;
    f.set(json!({"role_id": f.deployer})).await;
    let removed = roles::delete(f.ee.store.pool(), f.deployer).await.unwrap();
    assert_eq!(removed, 1);
    assert_eq!(f.listed().await, json!([]));
    assert!(f.bob_access().await.is_null());
}
