//! A FleetOps app with a data directory, a fixture repository and a git
//! server, for git-backed stack tests.

use super::*;
use fleetops_api::stacks::git::{sync_stack, SyncOutcome, SyncTrigger};
use fleetops_api::stacks::Timing;
use fleetops_git::testing::{GitServer, ServerOpts, SshOpts, SshServer, TestRepo};
use fleetops_git::KeyPair;
use fleetops_store::{GitSettings, KeyChange, StackRecord, TokenChange};
use std::time::Duration;

pub const STACKS: &str = "/api/v1/envs/local/stacks";
pub const WEB_A: &str = "services:\n  web:\n    image: nginx:1.27\n";
pub const WEB_B: &str = "services:\n  web:\n    image: nginx:1.28\n";

pub struct GitApp {
    pub backup: BackupApp,
    pub admin: Session,
    pub repo: TestRepo,
    pub server: GitServer,
    pub stack: StackRecord,
    /// With [`ssh_git_app`]: the SSH server and the key the stack holds.
    pub ssh: Option<SshServer>,
    pub key: Option<KeyPair>,
}

impl std::ops::Deref for GitApp {
    type Target = TestApp;
    fn deref(&self) -> &TestApp {
        &self.backup.app
    }
}

pub fn git_config() -> ApiConfig {
    ApiConfig {
        git_allow_http: true,
        stack_timing: Timing {
            exit_check: Duration::from_millis(5),
            poll: Duration::from_millis(1),
            health_cap: Duration::from_millis(50),
        },
        ..Default::default()
    }
}

pub fn settings(server: &GitServer, sync: &str, auto_deploy: bool) -> GitSettings {
    GitSettings {
        url: server.url("repo.git"),
        branch: "main".into(),
        path: "compose.yaml".into(),
        sync: sync.into(),
        poll_seconds: 300,
        auto_deploy,
    }
}

/// An app with stack `shop` (inline, empty) on `local`, and a repo served
/// with `opts`. The stack is attached to the repo unless `attach` is None.
pub async fn git_app(attach: Option<(&str, bool)>, opts: ServerOpts) -> GitApp {
    let backup = app_with_data(None, git_config()).await;
    let admin = backup.app.with_local().await;
    let r = backup
        .app
        .send(req(
            "POST",
            STACKS,
            Some(json!({"name": "shop", "compose": "", "variables": []})),
            Some(&admin),
        ))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
    let repo = TestRepo::new("main");
    let token = opts.token.clone();
    let server = GitServer::start(&repo.root(), opts).await;
    let env = backup.app.state.envs.get("local").unwrap().id();
    let mut stack = backup.app.store.stack_by_name(env, "shop").await.unwrap();
    if let Some((sync, auto)) = attach {
        let t = match &token {
            Some(t) => TokenChange::Set(t),
            None => TokenChange::Keep,
        };
        backup
            .app
            .store
            .set_stack_git(
                stack.id,
                &settings(&server, sync, auto),
                t,
                Some("hook-secret"),
                None,
                1,
            )
            .await
            .unwrap();
        stack = backup.app.store.stack_by_id(stack.id).await.unwrap();
    }
    GitApp {
        backup,
        admin,
        repo,
        server,
        stack,
        ssh: None,
        key: None,
    }
}

/// [`git_app`] over SSH: the repo is also served by an SSH server set up by
/// `opts` (given the key the stack holds), and the stack is attached to it
/// with that key unless `attach` is None.
pub async fn ssh_git_app(
    attach: Option<(&str, bool)>,
    opts: impl FnOnce(&KeyPair) -> SshOpts,
) -> GitApp {
    let mut g = git_app(None, ServerOpts::default()).await;
    let key = fleetops_git::generate_key("fleetops:shop");
    let ssh = SshServer::start(&g.repo.root(), opts(&key)).await;
    if let Some((sync, auto)) = attach {
        let settings = GitSettings {
            url: ssh.url("repo.git"),
            ..settings(&g.server, sync, auto)
        };
        g.store
            .set_stack_git_with(
                g.stack.id,
                &settings,
                TokenChange::Keep,
                KeyChange::Set {
                    private: &key.private,
                    public: &key.public,
                },
                Some("hook-secret"),
                None,
                1,
            )
            .await
            .unwrap();
        g.stack = g.store.stack_by_id(g.stack.id).await.unwrap();
    }
    g.ssh = Some(ssh);
    g.key = Some(key);
    g
}

/// SSH options that let `key` in, speaking protocol v2.
pub fn authorised(key: &KeyPair) -> SshOpts {
    SshOpts {
        authorized: vec![key.public.clone()],
        accept_env: true,
        hold: None,
    }
}

impl GitApp {
    pub async fn sync(&self, trigger: SyncTrigger) -> SyncOutcome {
        sync_stack(&self.state, &self.stack, trigger).await.unwrap()
    }

    pub async fn git(&self) -> fleetops_store::GitRecord {
        self.store.stack_git(self.stack.id).await.unwrap().unwrap()
    }

    pub async fn revisions(&self) -> Vec<fleetops_store::RevisionRecord> {
        self.store.list_revisions(self.stack.id).await.unwrap()
    }

    /// Waits for task `id` to finish and returns it.
    pub async fn wait_task(&self, id: &str) -> Value {
        for _ in 0..1000 {
            let r = self
                .send(req(
                    "GET",
                    &format!("/api/v1/tasks/{id}"),
                    None,
                    Some(&self.admin),
                ))
                .await;
            if r.json["status"] != "running" {
                // Let the completion hook record its outcome.
                tokio::time::sleep(Duration::from_millis(20)).await;
                return r.json;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("task {id} never finished");
    }

    /// Syncs, expects an auto-deploy, and waits for it.
    pub async fn sync_and_deploy(&self, trigger: SyncTrigger) -> (i64, Value) {
        match self.sync(trigger).await {
            SyncOutcome::Deploying {
                task_id, revision, ..
            } => (revision, self.wait_task(&task_id).await),
            other => panic!("expected a deploy, got {other:?}"),
        }
    }
}
