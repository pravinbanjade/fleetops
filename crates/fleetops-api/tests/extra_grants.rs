mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use fleetops_core::access::{EnvRole, ExtraGrants, Grant};
use fleetops_core::kube::KubeKind;
use fleetops_core::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Grants set by the test, by (user id, environment id).
#[derive(Default)]
struct Fixed(Mutex<HashMap<(i64, i64), Grant>>);

#[async_trait]
impl ExtraGrants for Fixed {
    async fn env_grants(&self, user_id: i64, env_id: i64) -> Result<Vec<Grant>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(user_id, env_id))
            .cloned()
            .into_iter()
            .collect())
    }
    async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Vec<Grant>>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|((u, _), _)| *u == user_id)
            .map(|((_, e), g)| (*e, vec![g.clone()]))
            .collect())
    }
}

fn ns(role: EnvRole, n: &[&str]) -> Grant {
    Grant {
        role,
        namespaces: Some(n.iter().map(|s| s.to_string()).collect()),
    }
}

async fn ids(t: &TestApp, user: &str, env: &str) -> (i64, i64) {
    let (u, _) = t.store.find_credentials(user).await.unwrap().unwrap();
    let e = t
        .store
        .list_environments()
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == env)
        .unwrap();
    (u.id, e.id)
}

async fn docker() -> (TestApp, Arc<Fixed>, Session, (i64, i64)) {
    let extra = Arc::new(Fixed::default());
    let t = app_with_extra(extra.clone()).await;
    let admin = t.with_local().await;
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    let key = ids(&t, "bob", "local").await;
    (t, extra, bob, key)
}

async fn create_container(t: &TestApp, bob: &Session) -> StatusCode {
    t.send(req(
        "POST",
        "/api/v1/envs/local/containers",
        Some(json!({"image": "alpine"})),
        Some(bob),
    ))
    .await
    .status
}

#[tokio::test]
async fn extra_grant_alone_opens_an_env() {
    let (t, extra, bob, key) = docker().await;
    extra
        .0
        .lock()
        .unwrap()
        .insert(key, Grant::full(EnvRole::Viewer));
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&bob))).await;
    assert_eq!(r.json[0]["name"], "local");
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(create_container(&t, &bob).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn direct_viewer_plus_extra_operator_is_operator() {
    let (t, extra, bob, key) = docker().await;
    t.grant("bob", "local", "viewer", None).await;
    extra
        .0
        .lock()
        .unwrap()
        .insert(key, Grant::full(EnvRole::Operator));
    assert_eq!(create_container(&t, &bob).await, StatusCode::CREATED);
}

#[tokio::test]
async fn no_grants_reads_as_not_found() {
    let (t, _extra, bob, _) = docker().await;
    let r = t
        .send(req(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = t.send(req("GET", "/api/v1/envs", None, Some(&bob))).await;
    assert_eq!(r.json, json!([]));
}

#[tokio::test]
async fn kube_namespaces_union() {
    let extra = Arc::new(Fixed::default());
    let t = app_with_extra(extra.clone()).await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    for n in ["a", "b", "c"] {
        t.fake
            .kube
            .object(KubeKind::Deployments, n, &format!("d-{n}"));
    }
    t.create_user(&admin, "bob", "bob-password-1", "user").await;
    t.grant("bob", "k", "viewer", Some(&["a"])).await;
    let key = ids(&t, "bob", "k").await;
    extra
        .0
        .lock()
        .unwrap()
        .insert(key, ns(EnvRole::Viewer, &["b"]));
    let bob = session_from(&t.login("bob", "bob-password-1").await);
    for (n, want) in [("a", 200), ("b", 200), ("c", 403)] {
        let r = t
            .send(req(
                "GET",
                &format!("/api/v1/envs/k/k8s/deployments?namespace={n}"),
                None,
                Some(&bob),
            ))
            .await;
        assert_eq!(r.status, want, "namespace {n}");
    }
}
