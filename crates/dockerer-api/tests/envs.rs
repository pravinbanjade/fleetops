mod common;

use axum::http::StatusCode;
use common::*;
use dockerer_api::envs::bootstrap_local;
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn bootstrap_registers_reachable_local_once() {
    let app = app().await;
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert_eq!(rec.unwrap().name, "local");
    assert!(app.state.envs.get("local").is_ok());
    let again = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(again.is_none());
    assert_eq!(app.store.list_environments().await.unwrap().len(), 1);
}

#[tokio::test]
async fn bootstrap_skips_unreachable_endpoint() {
    let app = app().await;
    app.fake.reachable.store(false, Ordering::SeqCst);
    let rec = bootstrap_local(&app.store, &app.state.envs, "unix:///var/run/docker.sock")
        .await
        .unwrap();
    assert!(rec.is_none());
    assert!(app.store.list_environments().await.unwrap().is_empty());
}

#[tokio::test]
async fn admin_manages_environments() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.add_env(&admin, "local").await;

    let list = app
        .send(req("GET", "/api/v1/envs", None, Some(&admin)))
        .await;
    assert_eq!(list.json[0]["name"], "local");
    assert_eq!(list.json[0]["kind"], "docker-local");

    let r = app
        .send(req(
            "PATCH",
            "/api/v1/envs/local",
            Some(json!({"name": "home", "url": "tcp://10.0.0.2:2375"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["url"], "tcp://10.0.0.2:2375");
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/home/status", None, Some(&admin)))
            .await
            .status,
        StatusCode::OK
    );

    let r = app
        .send(req("DELETE", "/api/v1/envs/home", None, Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send(req(
            "GET",
            "/api/v1/envs/home/containers",
            None,
            Some(&admin)
        ))
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(&actions[..3], &["env.delete", "env.update", "env.create"]);
}

#[tokio::test]
async fn environment_input_is_validated() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let bad_name = json!({"name": "a/b", "kind": "docker-local", "url": "unix:///x.sock"});
    let bad_url = json!({"name": "x", "kind": "docker-local", "url": "ssh://host"});
    let bad_kind = json!({"name": "x", "kind": "k8s", "url": "unix:///x.sock"});
    for (body, status) in [(bad_name, 400), (bad_url, 400), (bad_kind, 422)] {
        let r = app
            .send(req("POST", "/api/v1/envs", Some(body), Some(&admin)))
            .await;
        assert_eq!(r.status.as_u16(), status, "{:?}", r.json);
    }
    app.add_env(&admin, "local").await;
    let dup = app
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "local", "kind": "docker-local", "url": "unix:///y.sock"})),
            Some(&admin),
        ))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn users_can_read_but_not_manage_environments() {
    let app = app().await;
    let admin = app.with_local().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    assert_eq!(
        app.send(req("GET", "/api/v1/envs", None, Some(&bob)))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.send(req("GET", "/api/v1/envs/local/status", None, Some(&bob)))
            .await
            .status,
        StatusCode::OK
    );
    let r = app
        .send(req(
            "POST",
            "/api/v1/envs",
            Some(json!({"name": "x", "kind": "docker-local", "url": "unix:///x.sock"})),
            Some(&bob),
        ))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(req("DELETE", "/api/v1/envs/local", None, Some(&bob)))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn status_reports_up_and_down() {
    let app = app().await;
    let admin = app.with_local().await;
    let r = app
        .send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
        .await;
    assert_eq!(r.json["state"], "up");
    assert_eq!(r.json["version"]["api_version"], "1.54");

    app.fake.reachable.store(false, Ordering::SeqCst);
    let r = app
        .send(req("GET", "/api/v1/envs/local/status", None, Some(&admin)))
        .await;
    assert_eq!(r.json["state"], "down");
    let r = app
        .send(req(
            "GET",
            "/api/v1/envs/local/containers",
            None,
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.json["code"], "env_unavailable");
}

#[tokio::test]
async fn environments_survive_restart() {
    let app = app().await;
    let admin = app.with_local().await;
    drop(admin);
    // A fresh registry over the same store sees the stored environment.
    let state = dockerer_api::AppState::new(
        app.store.clone(),
        dockerer_api::ApiConfig::default(),
        std::sync::Arc::new(common::fake_docker::FakeFactory(app.fake.clone())),
    );
    assert!(state.envs.get("local").is_err());
    state.envs.load(&app.store).await.unwrap();
    assert_eq!(state.envs.get("local").unwrap().name(), "local");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_rename_and_delete_keep_registry_and_db_in_step() {
    let t = app().await;
    let admin = t.setup_admin().await;
    for i in 0..40 {
        let (a, b) = (format!("a{i}"), format!("b{i}"));
        t.add_env(&admin, &a).await;
        let rename = t.send(req(
            "PATCH",
            &format!("/api/v1/envs/{a}"),
            Some(json!({"name": b})),
            Some(&admin),
        ));
        let delete = t.send(req(
            "DELETE",
            &format!("/api/v1/envs/{a}"),
            None,
            Some(&admin),
        ));
        tokio::join!(rename, delete);
        let in_db: Vec<String> = t
            .store
            .list_environments()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        for name in [&a, &b] {
            assert_eq!(
                t.state.envs.get(name).is_ok(),
                in_db.contains(name),
                "iteration {i}: registry and database disagree about '{name}' (db: {in_db:?})"
            );
        }
    }
}
