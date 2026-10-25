//! `/ee/roles` (custom roles spec): CRUD, the freeze, signalling, audit,
//! the catalog, and effective access listed per source.

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
    bob_id: i64,
    carol_id: i64,
    env: i64,
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
    let carol_id = ee.user(&admin, "carol").await.json()["id"]
        .as_i64()
        .unwrap();
    let env = ee
        .store
        .create_environment("e", "docker-local", "unix:///x", 0)
        .await
        .unwrap()
        .id;
    Fx {
        ee,
        admin,
        bob_id,
        carol_id,
        env,
    }
}

impl Fx {
    async fn call(&self, method: &str, uri: &str, b: Option<Value>) -> Resp {
        self.ee
            .send(method, uri, b.and_then(body), Some(&self.admin))
            .await
    }

    async fn role(&self, name: &str, perms: &[&str]) -> i64 {
        let r = self
            .call(
                "POST",
                "/api/v1/ee/roles",
                Some(json!({"name": name, "permissions": perms})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
        r.json()["id"].as_i64().unwrap()
    }

    async fn patch(&self, id: i64, b: Value) -> Resp {
        self.call("PATCH", &format!("/api/v1/ee/roles/{id}"), Some(b))
            .await
    }

    async fn unlicense(&self) {
        let r = self.call("DELETE", "/api/v1/ee/license", None).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    }

    fn signals(&self) -> Receiver<i64> {
        self.ee.state.hubs.access().subscribe()
    }
}

fn drained(rx: &mut Receiver<i64>) -> BTreeSet<i64> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

#[tokio::test]
async fn admin_creates_lists_edits_and_deletes_a_role() {
    let f = fx().await;
    let id = f
        .role("Deployer", &["containers.view", "containers.operate"])
        .await;
    let r = f.call("GET", "/api/v1/ee/roles", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json(),
        json!([{"id": id, "name": "Deployer", "description": null,
            "permissions": ["containers.operate", "containers.view"], "users": 0, "teams": 0}])
    );
    let r = f
        .patch(
            id,
            json!({"description": "ship it", "permissions": ["containers.view"]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.json()["description"], "ship it");
    assert_eq!(r.json()["permissions"], json!(["containers.view"]));
    let r = f.call("GET", &format!("/api/v1/ee/roles/{id}"), None).await;
    assert_eq!(r.json()["holders"], json!({"users": [], "teams": []}));
    let r = f
        .call("DELETE", &format!("/api/v1/ee/roles/{id}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = f.call("GET", &format!("/api/v1/ee/roles/{id}"), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let actions = f.ee.actions().await;
    for a in ["role.create", "role.update", "role.delete"] {
        assert!(actions.contains(&a.to_string()), "{a}: {actions:?}");
    }
}

#[tokio::test]
async fn bad_roles_are_refused() {
    let f = fx().await;
    f.role("Deployer", &[]).await;
    let cases = [
        (
            json!({"name": " deployer ", "permissions": []}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"name": "Viewer", "permissions": []}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"name": "", "permissions": []}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"name": "x", "permissions": ["volumes.shell"]}),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (b, want) in cases {
        let r = f.call("POST", "/api/v1/ee/roles", Some(b.clone())).await;
        assert_eq!(r.status, want, "{b}: {}", r.text);
    }
}

#[tokio::test]
async fn the_role_update_audit_names_what_changed() {
    let f = fx().await;
    let id = f.role("R", &["pods.view"]).await;
    f.patch(id, json!({"permissions": ["pods.view", "pods.delete"]}))
        .await;
    let rows = f.ee.store.list_audit(10, None).await.unwrap();
    let up = rows.iter().find(|a| a.action == "role.update").unwrap();
    let d = up.detail.as_ref().unwrap();
    assert_eq!(d["added"], json!(["pods.delete"]));
    assert_eq!(d["removed"], json!([]));
}

#[tokio::test]
async fn unlicensed_roles_freeze() {
    let f = fx().await;
    let id = f.role("R", &["pods.view", "pods.delete"]).await;
    f.unlicense().await;
    let r = f
        .call(
            "POST",
            "/api/v1/ee/roles",
            Some(json!({"name": "New", "permissions": []})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["code"], "license_required");
    for b in [
        json!({"name": "Renamed"}),
        json!({"permissions": ["pods.view", "pods.delete", "pods.shell"]}),
    ] {
        let r = f.patch(id, b.clone()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{b}: {}", r.text);
    }
    let same = f
        .patch(id, json!({"permissions": ["pods.delete", "pods.view"]}))
        .await;
    assert_eq!(same.status, StatusCode::OK, "{}", same.text);
    let fewer = f.patch(id, json!({"permissions": ["pods.view"]})).await;
    assert_eq!(fewer.status, StatusCode::OK, "{}", fewer.text);
    assert_eq!(
        f.call("GET", "/api/v1/ee/roles", None).await.status,
        StatusCode::OK
    );
    let r = f
        .call("DELETE", &format!("/api/v1/ee/roles/{id}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn editing_a_role_signals_its_holders() {
    let f = fx().await;
    let id = f.role("R", &["containers.view"]).await;
    let uri = format!("/api/v1/users/{}/access/{}", f.bob_id, f.env);
    f.call("PUT", &uri, Some(json!({"role_id": id}))).await;
    let mut rx = f.signals();
    f.patch(
        id,
        json!({"permissions": ["containers.view", "containers.operate"]}),
    )
    .await;
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id]));
    f.patch(id, json!({"description": "only words"})).await;
    assert!(drained(&mut rx).is_empty(), "no access changed");
}

#[tokio::test]
async fn deleting_a_team_held_role_signals_members_and_removes_grants() {
    let f = fx().await;
    let id = f.role("R", &["containers.view"]).await;
    let r = f
        .call("POST", "/api/v1/ee/teams", Some(json!({"name": "ops"})))
        .await;
    let team = r.json()["id"].as_i64().unwrap();
    for u in [f.bob_id, f.carol_id] {
        f.call("PUT", &format!("/api/v1/ee/teams/{team}/members/{u}"), None)
            .await;
    }
    let uri = format!("/api/v1/ee/teams/{team}/access/{}", f.env);
    f.call("PUT", &uri, Some(json!({"role_id": id}))).await;
    let r = f.call("GET", &format!("/api/v1/ee/roles/{id}"), None).await;
    assert_eq!(r.json()["holders"]["teams"][0]["name"], "ops");
    assert_eq!(r.json()["holders"]["teams"][0]["env"], "e");
    let mut rx = f.signals();
    let r = f
        .call("DELETE", &format!("/api/v1/ee/roles/{id}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(drained(&mut rx), BTreeSet::from([f.bob_id, f.carol_id]));
    let r = f
        .call("GET", &format!("/api/v1/ee/teams/{team}"), None)
        .await;
    assert_eq!(r.json()["access"], json!([]));
    let rows = f.ee.store.list_audit(10, None).await.unwrap();
    let del = rows.iter().find(|a| a.action == "role.delete").unwrap();
    assert_eq!(del.detail.as_ref().unwrap()["grants_removed"], 1);
}

#[tokio::test]
async fn the_catalog_is_the_grid() {
    let f = fx().await;
    let r = f.call("GET", "/api/v1/ee/roles/catalog", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let c = r.json();
    let cells: usize = c["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["cells"].as_array().unwrap().len())
        .sum();
    assert_eq!(cells, 70);
    assert_eq!(
        c["rows"][0],
        json!({"key": "containers", "label": "Containers", "group": "docker",
        "cells": ["view", "create", "operate", "delete", "shell", "reveal"]})
    );
    assert_eq!(c["verbs"][5], json!({"key": "reveal", "label": "Reveal"}));
}

#[tokio::test]
async fn only_admins_manage_roles() {
    let f = fx().await;
    let bob = f.ee.login("bob").await;
    let r = f.ee.send("GET", "/api/v1/ee/roles", None, Some(&bob)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn user_access_lists_each_source_separately() {
    let f = fx().await;
    let id = f
        .role("Deployer", &["containers.view", "containers.operate"])
        .await;
    let uri = format!("/api/v1/users/{}/access/{}", f.bob_id, f.env);
    f.call("PUT", &uri, Some(json!({"role_id": id}))).await;
    let r = f
        .call("POST", "/api/v1/ee/teams", Some(json!({"name": "ops"})))
        .await;
    let team = r.json()["id"].as_i64().unwrap();
    f.call(
        "PUT",
        &format!("/api/v1/ee/teams/{team}/members/{}", f.bob_id),
        None,
    )
    .await;
    f.call(
        "PUT",
        &format!("/api/v1/ee/teams/{team}/access/{}", f.env),
        Some(json!({"role": "viewer"})),
    )
    .await;
    let r = f
        .call(
            "GET",
            &format!("/api/v1/ee/users/{}/access", f.bob_id),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let e = &r.json()[0];
    assert_eq!(e["env_name"], "e");
    let sources = e["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert!(sources[0]["team_id"].is_null());
    assert_eq!(sources[0]["role_name"], "Deployer");
    assert_eq!(sources[1]["team_name"], "ops");
    assert_eq!(sources[1]["role"], "viewer");
}
