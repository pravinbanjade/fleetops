mod common;

use axum::http::StatusCode;
use common::git::*;
use common::*;
use fleetops_api::stacks::git::{SyncOutcome, SyncTrigger};
use fleetops_git::testing::SshOpts;
use serde_json::{json, Value};

const GIT: &str = "/api/v1/envs/local/stacks/shop/git";
const RSA: &str = include_str!("../../fleetops-git/tests/fixtures/keys/rsa");
const ED25519: &str = include_str!("../../fleetops-git/tests/fixtures/keys/ed25519");

fn ssh_url(g: &GitApp) -> String {
    g.ssh.as_ref().unwrap().url("repo.git")
}

fn body(url: &str, extra: Value) -> Value {
    let mut b = json!({"url": url, "branch": "main", "sync": "manual"});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

async fn put(g: &GitApp, b: Value) -> Resp {
    g.send(req("PUT", GIT, Some(b), Some(&g.admin))).await
}

async fn audit_details(g: &GitApp) -> String {
    let rows = g.store.list_audit(1000, None).await.unwrap();
    serde_json::to_string(&rows.iter().map(|r| r.detail.clone()).collect::<Vec<_>>()).unwrap()
}

#[tokio::test]
async fn generate_key_returns_only_the_public_key() {
    let g = ssh_git_app(None, authorised).await;
    let r = put(&g, body(&ssh_url(&g), json!({"generate_key": true}))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    let git = &r.json["status"]["git"];
    assert_eq!(git["has_key"], true);
    let public = git["ssh_public_key"].as_str().unwrap();
    assert!(public.starts_with("ssh-ed25519 "), "{public}");
    assert!(public.ends_with(" fleetops:shop"), "{public}");
    let got = g.send(req("GET", GIT, None, Some(&g.admin))).await;
    for text in [
        r.json.to_string(),
        got.json.to_string(),
        audit_details(&g).await,
    ] {
        assert!(!text.contains("PRIVATE KEY"), "{text}");
    }
    assert!(audit_details(&g).await.contains("generated"));
    let private = g.store.stack_git_key(g.stack.id).await.unwrap().unwrap();
    assert_eq!(fleetops_git::parse_key(&private).unwrap().public, public);
}

#[tokio::test]
async fn pasted_key_and_refusals() {
    let g = ssh_git_app(None, authorised).await;
    let url = ssh_url(&g);
    let r = put(&g, body(&url, json!({"ssh_key": ED25519}))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json);
    assert_eq!(r.json["status"]["git"]["has_key"], true);
    assert_eq!(
        r.json["status"]["git"]["ssh_public_key"].as_str(),
        Some(fleetops_git::parse_key(ED25519).unwrap().public.as_str())
    );
    assert!(audit_details(&g).await.contains("\"key\":\"set\""));
    let https = g.server.url("repo.git");
    let cases = [
        (
            body(&url, json!({"ssh_key": RSA})),
            "RSA keys are not supported: use ed25519",
        ),
        (
            body(&url, json!({"token": "t"})),
            "an access token is only for https URLs; use an SSH key",
        ),
        (
            body(&https, json!({"ssh_key": ED25519})),
            "an SSH key is only for ssh URLs",
        ),
        (
            body(&https, json!({"generate_key": true})),
            "an SSH key is only for ssh URLs",
        ),
        (
            body(&url, json!({"generate_key": true, "ssh_key": ED25519})),
            "send either generate_key or ssh_key, not both",
        ),
    ];
    for (b, msg) in cases {
        let r = put(&g, b.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{b}");
        let got = r.json["message"].as_str().unwrap();
        assert!(got.ends_with(msg), "{got} / {b}");
    }
}

#[tokio::test]
async fn switching_url_kind_forgets_the_other_credential() {
    let g = ssh_git_app(None, authorised).await;
    let https = g.server.url("repo.git");
    let r = put(&g, body(&https, json!({"token": "tok"}))).await;
    assert_eq!(r.json["status"]["git"]["has_token"], true, "{:?}", r.json);
    let r = put(&g, body(&ssh_url(&g), json!({"generate_key": true}))).await;
    assert_eq!(r.json["status"]["git"]["has_token"], false, "{:?}", r.json);
    assert_eq!(r.json["status"]["git"]["has_key"], true);
    let r = put(&g, body(&https, json!({}))).await;
    assert_eq!(r.json["status"]["git"]["has_key"], false, "{:?}", r.json);
    assert!(r.json["status"]["git"]["ssh_public_key"].is_null());
    assert_eq!(g.store.stack_git_key(g.stack.id).await.unwrap(), None);
}

#[tokio::test]
async fn ssh_stack_syncs_and_pins_the_host_key() {
    let g = ssh_git_app(Some(("manual", false)), authorised).await;
    let head = g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let out = g.sync(SyncTrigger::Manual(1)).await;
    assert!(
        matches!(out, SyncOutcome::Pending { ref commit } if *commit == head),
        "{out:?}"
    );
    assert_eq!(
        g.store.draft(g.stack.id).await.unwrap().unwrap().compose,
        WEB_A
    );
    assert_eq!(
        g.git().await.ssh_host_key,
        Some(g.ssh.as_ref().unwrap().host_key.clone())
    );
}

#[tokio::test]
async fn refused_key_pins_host_key() {
    let g = ssh_git_app(Some(("manual", false)), |_| SshOpts {
        accept_env: true,
        ..Default::default()
    })
    .await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    let out = g.sync(SyncTrigger::Manual(1)).await;
    assert!(
        matches!(out, SyncOutcome::Failed { ref error } if error == "the repository refused the SSH key"),
        "{out:?}"
    );
    assert_eq!(
        g.git().await.ssh_host_key,
        Some(g.ssh.as_ref().unwrap().host_key.clone())
    );
}

#[tokio::test]
async fn changed_host_key_blocks_until_reset() {
    let g = ssh_git_app(Some(("manual", false)), authorised).await;
    g.repo.commit(&[("compose.yaml", WEB_A)], "one");
    g.store
        .set_git_host_key(g.stack.id, Some("ssh-ed25519 SHA256:old"))
        .await
        .unwrap();
    let out = g.sync(SyncTrigger::Manual(1)).await;
    let SyncOutcome::Failed { error } = out else {
        panic!("{out:?}");
    };
    assert!(
        error.starts_with("the host key of 127.0.0.1 changed (now SHA256:"),
        "{error}"
    );
    assert_eq!(
        g.git().await.ssh_host_key.as_deref(),
        Some("ssh-ed25519 SHA256:old")
    );

    g.create_user(&g.admin, "vic", "vic-password", "user").await;
    g.grant("vic", "local", "viewer", None).await;
    let vic = session_from(&g.login("vic", "vic-password").await);
    let r = g
        .send(req("DELETE", &format!("{GIT}/host-key"), None, Some(&vic)))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = g
        .send(req(
            "DELETE",
            &format!("{GIT}/host-key"),
            None,
            Some(&g.admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.json);
    assert!(g
        .audit_actions()
        .await
        .contains(&"stack.git.host_key.reset".to_string()));
    assert_eq!(g.git().await.ssh_host_key, None);
    let out = g.sync(SyncTrigger::Manual(1)).await;
    assert!(matches!(out, SyncOutcome::Pending { .. }), "{out:?}");
    assert_eq!(
        g.git().await.ssh_host_key,
        Some(g.ssh.as_ref().unwrap().host_key.clone())
    );
}
