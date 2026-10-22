//! Git syncs (stacks spec §8): fetch the stack's file, put it in the draft,
//! then mark it pending or auto-deploy it.

use super::start::{start_deploy, Actor, DeploySource, GIT_ACTOR};
use crate::AppState;
use fleetops_core::compose::parse;
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{GitRecord, NewAudit, StackRecord};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use utoipa::ToSchema;

/// What started a sync; the ids are the user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncTrigger {
    Manual(i64),
    Poll,
    Webhook,
    Resume(i64),
    Settings(i64),
}

impl SyncTrigger {
    pub fn name(self) -> &'static str {
        match self {
            SyncTrigger::Manual(_) => "manual",
            SyncTrigger::Poll => "poll",
            SyncTrigger::Webhook => "webhook",
            SyncTrigger::Resume(_) => "resume",
            SyncTrigger::Settings(_) => "settings",
        }
    }

    fn user(self) -> Option<i64> {
        match self {
            SyncTrigger::Manual(u) | SyncTrigger::Resume(u) | SyncTrigger::Settings(u) => Some(u),
            SyncTrigger::Poll | SyncTrigger::Webhook => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SyncOutcome {
    /// The deployed revision already has this file.
    UpToDate {
        commit: String,
    },
    /// The file differs from the deployed one and waits for a deploy.
    Pending {
        commit: String,
    },
    /// An auto-deploy of the commit started.
    Deploying {
        commit: String,
        task_id: String,
        revision: i64,
    },
    /// A sync of this stack is running; it fetches once more when done.
    Queued,
    Failed {
        error: String,
    },
}

/// Prefix of errors from an auto-deploy; they stay on the stack while the
/// same commit is pending.
const AUTO_DEPLOY_FAILED: &str = "auto-deploy of ";
pub const UNREACHABLE: &str = "the environment is not reachable; the update is pending";
pub const BUSY: &str = "a deploy of this stack is running; the update is pending";
const DETACHED: &str = "the stack was detached from git during the sync";

/// `<data_dir>/git/<stack id>`.
pub fn cache_dir(s: &AppState, stack_id: i64) -> Result<PathBuf> {
    let data =
        s.config.data_dir.as_ref().ok_or_else(|| {
            Error::Invalid("git stacks need a data directory (FLEETOPS_DATA)".into())
        })?;
    Ok(data.join("git").join(stack_id.to_string()))
}

/// Removes the stack's repository cache, if any.
pub fn remove_cache(s: &AppState, stack_id: i64) {
    if let Ok(dir) = cache_dir(s, stack_id) {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, stack_id, "removing a git cache failed");
            }
        }
    }
}

struct Gate<'a> {
    s: &'a AppState,
    id: i64,
}

impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.s.git_syncs.lock().unwrap().remove(&self.id);
    }
}

/// Syncs `stack`. A sync asked for while one runs returns `Queued`, and the
/// running one fetches again when it is done.
pub async fn sync_stack(
    s: &AppState,
    stack: &StackRecord,
    trigger: SyncTrigger,
) -> Result<SyncOutcome> {
    {
        let mut running = s.git_syncs.lock().unwrap();
        if let Some(again) = running.get_mut(&stack.id) {
            *again = true;
            return Ok(SyncOutcome::Queued);
        }
        running.insert(stack.id, false);
    }
    let _gate = Gate { s, id: stack.id };
    let mut trigger = trigger;
    loop {
        let outcome = sync_once(s, stack.id, trigger).await;
        let again = s
            .git_syncs
            .lock()
            .unwrap()
            .get_mut(&stack.id)
            .map(std::mem::take)
            .unwrap_or(false);
        if !again {
            // Detached or removed meanwhile: the route's cache removal may
            // have come before this fetch wrote it.
            if matches!(s.store.stack_git(stack.id).await, Ok(None)) {
                remove_cache(s, stack.id);
            }
            return outcome;
        }
        // A rerun is for whatever was pushed meanwhile; a resume already
        // had its deploy.
        if let SyncTrigger::Resume(u) = trigger {
            trigger = SyncTrigger::Manual(u);
        }
    }
}

async fn sync_once(s: &AppState, stack_id: i64, trigger: SyncTrigger) -> Result<SyncOutcome> {
    let stack = s.store.stack_by_id(stack_id).await?;
    let git = s
        .store
        .stack_git(stack_id)
        .await?
        .ok_or_else(|| Error::Invalid(format!("stack '{}' has no git source", stack.name)))?;
    let outcome = sync_fetched(s, &stack, &git, trigger).await?;
    audit_sync(s, &stack, trigger, &outcome).await;
    Ok(outcome)
}

async fn sync_fetched(
    s: &AppState,
    stack: &StackRecord,
    git: &GitRecord,
    trigger: SyncTrigger,
) -> Result<SyncOutcome> {
    let source = fleetops_git::Source {
        url: git.url.clone(),
        branch: git.branch.clone(),
        path: git.path.clone(),
        token: s.store.stack_git_token(stack.id).await?,
    };
    let fetched = match fleetops_git::fetch_file(
        &cache_dir(s, stack.id)?,
        &source,
        s.config.git_limits,
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            let error = e.detail();
            s.store
                .record_git_error(stack.id, &error, now_unix())
                .await?;
            return Ok(SyncOutcome::Failed { error });
        }
    };
    let commit = fetched.commit.clone();
    let short = &commit[..7];
    // A new file, not a new commit, is what auto-deploys: a commit that
    // leaves the file alone never retries a failed auto-deploy. One that
    // could not start yet (environment down, another deploy running) is
    // tried again on the next sync.
    let draft = s.store.draft(stack.id).await?;
    let deferred = matches!(git.last_sync_error.as_deref(), Some(UNREACHABLE | BUSY));
    let fresh = git.last_fetched_commit.is_none()
        || draft.as_ref().map(|d| d.compose.as_str()) != Some(fetched.content.as_str())
        || matches!(trigger, SyncTrigger::Resume(_))
        || deferred;
    if !s
        .store
        .record_git_fetch(stack.id, &commit, &fetched.content, &[], now_unix())
        .await?
    {
        return Ok(SyncOutcome::Failed {
            error: DETACHED.into(),
        });
    }
    let deployed = match stack.deployed_revision {
        Some(n) => Some(s.store.revision(stack.id, n).await?.compose),
        None => None,
    };
    if deployed.as_deref() == Some(fetched.content.as_str()) {
        s.store.set_git_pending(stack.id, None).await?;
        return Ok(SyncOutcome::UpToDate { commit });
    }
    s.store.set_git_pending(stack.id, Some(&commit)).await?;
    let fail = |error: String| async move {
        s.store
            .record_git_error(stack.id, &error, now_unix())
            .await?;
        Ok::<_, Error>(SyncOutcome::Failed { error })
    };
    // The file is checked even when nothing deploys it, so a broken commit
    // shows on the stack straight away.
    let variables = draft.map(|d| d.variables).unwrap_or_default();
    if let Err(issues) = parse(&fetched.content, &variables, &stack.name) {
        let msg = super::start::issues_message(&issues);
        return fail(format!("{} at {short} has problems: {msg}", git.path)).await;
    }
    if !(git.auto_deploy && !git.paused && fresh) {
        if !fresh {
            // Keep a failed auto-deploy of this same commit in view.
            if let Some(e) = git
                .last_sync_error
                .as_deref()
                .filter(|e| e.starts_with(AUTO_DEPLOY_FAILED))
            {
                s.store.record_git_error(stack.id, e, now_unix()).await?;
            }
        }
        return Ok(SyncOutcome::Pending { commit });
    }
    let env = s.store.get_environment(stack.env_id).await?;
    let handle = s.envs.get(&env.name)?;
    let Some(ops) = handle.docker() else {
        s.store
            .record_git_error(stack.id, UNREACHABLE, now_unix())
            .await?;
        return Ok(SyncOutcome::Pending { commit });
    };
    match start_deploy(s, ops, handle, stack, DeploySource::Draft, None, Actor::Git).await {
        Ok(d) => Ok(SyncOutcome::Deploying {
            commit,
            task_id: d.task_id,
            revision: d.revision,
        }),
        Err(Error::EnvUnavailable(_)) => {
            s.store
                .record_git_error(stack.id, UNREACHABLE, now_unix())
                .await?;
            Ok(SyncOutcome::Pending { commit })
        }
        Err(Error::Conflict(_)) if s.stack_locks.lock().unwrap().contains(&stack.id) => {
            s.store.record_git_error(stack.id, BUSY, now_unix()).await?;
            Ok(SyncOutcome::Pending { commit })
        }
        Err(e) => {
            fail(format!(
                "{AUTO_DEPLOY_FAILED}{short} failed: {}",
                e.detail()
            ))
            .await
        }
    }
}

async fn audit_sync(
    s: &AppState,
    stack: &StackRecord,
    trigger: SyncTrigger,
    outcome: &SyncOutcome,
) {
    let mut detail = serde_json::to_value(outcome).unwrap_or_else(|_| json!({}));
    detail["trigger"] = json!(trigger.name());
    if trigger.user().is_none() {
        detail["by"] = json!(GIT_ACTOR);
    }
    let entry = NewAudit {
        ts: now_unix(),
        user_id: trigger.user(),
        environment_id: Some(stack.env_id),
        action: "stack.git.sync",
        target_type: Some("stack"),
        target_id: Some(&stack.name),
        result: if matches!(outcome, SyncOutcome::Failed { .. }) {
            "failure"
        } else {
            "success"
        },
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

/// Syncs every polled stack whose interval has passed, a few at a time.
pub async fn poll_once(s: &AppState) {
    use futures_util::StreamExt;
    let due = match s.store.git_stacks_due(now_unix()).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "listing git stacks to poll failed");
            return;
        }
    };
    futures_util::stream::iter(due)
        .for_each_concurrent(POLL_CONCURRENCY, |stack| async move {
            if let Err(e) = sync_stack(s, &stack, SyncTrigger::Poll).await {
                tracing::warn!(error = %e, stack = %stack.name, "git sync failed");
            }
        })
        .await;
}

/// Syncs that run at once when polling.
pub const POLL_CONCURRENCY: usize = 4;
pub const POLL_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Polls git stacks every 30 seconds.
pub async fn run_git_polling(s: AppState) {
    let mut tick = tokio::time::interval(POLL_TICK);
    loop {
        tick.tick().await;
        poll_once(&s).await;
    }
}
