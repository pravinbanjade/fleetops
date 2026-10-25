//! A stack's git source (stacks spec §7–§8): attach, change and detach it,
//! sync now, resume after a rollback, and rotate the webhook secret.

use super::stacks::audit;
use super::{stack_ctx, StackCtx};
use crate::openapi::EnvParam;
use crate::stacks::git::{cache_dir, remove_cache, sync_stack, SyncOutcome, SyncTrigger};
use crate::{
    auth::extract::CurrentUser,
    authz::Need,
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use fleetops_core::perm::{Permission, Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::{GitRecord, GitSettings, KeyChange, StackRecord, TokenChange};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

pub const DEFAULT_PATH: &str = "compose.yaml";
pub const DEFAULT_POLL_SECONDS: i64 = 300;
pub const MIN_POLL_SECONDS: i64 = 60;

#[derive(Serialize, ToSchema)]
pub struct GitStatus {
    pub git: GitRecord,
    /// Where git hosts send push events. Relative when no public URL is set.
    pub webhook_url: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GitBody {
    /// An https or ssh URL (`ssh://…` or `[user@]host:path`), without
    /// credentials.
    pub url: String,
    pub branch: String,
    /// The Compose file in the repository; `compose.yaml` by default.
    pub path: Option<String>,
    /// manual | poll | webhook
    pub sync: String,
    /// At least 60; 300 by default.
    pub poll_seconds: Option<i64>,
    #[serde(default)]
    pub auto_deploy: bool,
    /// A new access token, for https URLs; omit it to keep the stored one.
    pub token: Option<String>,
    /// Forget the stored access token.
    #[serde(default)]
    pub clear_token: bool,
    /// For ssh URLs: generate a new ed25519 deploy key.
    #[serde(default)]
    pub generate_key: bool,
    /// For ssh URLs: an unencrypted OpenSSH ed25519 or ECDSA private key.
    pub ssh_key: Option<String>,
    /// Forget the stored SSH key.
    #[serde(default)]
    pub clear_key: bool,
}

#[derive(Serialize, ToSchema)]
pub struct GitSaved {
    pub status: GitStatus,
    /// The webhook secret, only when it was just created; it is never
    /// shown again.
    pub webhook_secret: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SecretRotated {
    pub webhook_secret: String,
}

fn new_secret() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

pub(crate) fn webhook_url(s: &AppState, stack_id: i64) -> String {
    format!(
        "{}/api/v1/stacks/hooks/{stack_id}",
        s.config.public_url.as_deref().unwrap_or("")
    )
}

async fn git_of(s: &AppState, stack: &StackRecord) -> Result<GitRecord> {
    s.store
        .stack_git(stack.id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("stack '{}' has no git source", stack.name)))
}

fn status(s: &AppState, git: GitRecord) -> GitStatus {
    GitStatus {
        webhook_url: webhook_url(s, git.stack_id),
        git,
    }
}

fn settings(s: &AppState, b: &GitBody) -> Result<GitSettings> {
    fleetops_git::validate_url(&b.url, s.config.git_allow_http).map_err(Error::Invalid)?;
    fleetops_git::validate_branch(&b.branch).map_err(Error::Invalid)?;
    let path = b
        .path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| DEFAULT_PATH.into());
    fleetops_git::validate_path(&path).map_err(Error::Invalid)?;
    if !["manual", "poll", "webhook"].contains(&b.sync.as_str()) {
        return Err(Error::Invalid(
            "sync must be manual, poll or webhook".into(),
        ));
    }
    let poll_seconds = b.poll_seconds.unwrap_or(DEFAULT_POLL_SECONDS);
    if poll_seconds < MIN_POLL_SECONDS {
        return Err(Error::Invalid(
            "the polling interval must be at least 60 seconds".into(),
        ));
    }
    Ok(GitSettings {
        url: b.url.clone(),
        branch: b.branch.clone(),
        path,
        sync: b.sync.clone(),
        poll_seconds,
        auto_deploy: b.auto_deploy,
    })
}

async fn stack_of(s: &AppState, c: &StackCtx, name: &str) -> Result<StackRecord> {
    s.store.stack_by_name(c.env.id(), name).await
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks/{stack}/git",
    operation_id = "getStackGit",
    tag = "stacks",
    summary = "The stack's git source and its last sync",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "The git source", body = GitStatus))
)]
pub async fn get(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<GitStatus>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    Ok(Json(status(&s, git_of(&s, &stack).await?)))
}

#[utoipa::path(
    put,
    path = "/envs/{env}/stacks/{stack}/git",
    operation_id = "setStackGit",
    tag = "stacks",
    summary = "Take the stack's Compose file from a git repository",
    description = "Saves the source and starts a sync in the background. The webhook secret is returned only when it is created.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    request_body = GitBody,
    responses((status = 200, description = "Saved", body = GitSaved))
)]
pub async fn set(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<GitBody>,
) -> ApiResult<Json<GitSaved>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    // Create authors and Operate ships: a source that deploys itself is both.
    if body.auto_deploy
        && !c
            .grants
            .allows(Permission::new(Row::Stacks, Verb::Operate), None)
    {
        return Err(Error::Forbidden(
            "deploying new commits automatically also needs stacks.operate".into(),
        )
        .into());
    }
    let stack = stack_of(&s, &c, &name).await?;
    let settings = settings(&s, &body)?;
    cache_dir(&s, stack.id)?;
    let before = s.store.stack_git(stack.id).await?;
    let secret = before.is_none().then(new_secret);
    let ssh = fleetops_git::is_ssh_url(&settings.url);
    let new_token = body.token.as_deref().filter(|t| !t.is_empty());
    let pasted = body.ssh_key.as_deref().filter(|k| !k.trim().is_empty());
    if body.generate_key && pasted.is_some() {
        return Err(Error::Invalid("send either generate_key or ssh_key, not both".into()).into());
    }
    if ssh && new_token.is_some() {
        return Err(Error::Invalid(
            "an access token is only for https URLs; use an SSH key".into(),
        )
        .into());
    }
    if !ssh && (body.generate_key || pasted.is_some()) {
        return Err(Error::Invalid("an SSH key is only for ssh URLs".into()).into());
    }
    let pair = match pasted {
        Some(k) => Some(fleetops_git::parse_key(k).map_err(Error::Invalid)?),
        None if body.generate_key => Some(fleetops_git::generate_key(&format!(
            "fleetops:{}",
            stack.name
        ))),
        None => None,
    };
    let had = |f: fn(&GitRecord) -> bool| before.as_ref().is_some_and(f);
    // A URL of the other kind forgets the old kind's credential.
    let token = match new_token {
        Some(t) => TokenChange::Set(t),
        None if body.clear_token || (ssh && had(|b| b.has_token)) => TokenChange::Clear,
        None => TokenChange::Keep,
    };
    let key = match &pair {
        Some(p) => KeyChange::Set {
            private: &p.private,
            public: &p.public,
        },
        None if body.clear_key || (!ssh && had(|b| b.has_key)) => KeyChange::Clear,
        None => KeyChange::Keep,
    };
    let key_note = match key {
        KeyChange::Set { .. } if body.generate_key => "generated",
        KeyChange::Set { .. } => "set",
        KeyChange::Clear => "cleared",
        KeyChange::Keep => "kept",
    };
    let token_note = match token {
        TokenChange::Keep => "kept",
        TokenChange::Clear => "cleared",
        TokenChange::Set(_) => "set",
    };
    if before.as_ref().is_some_and(|b| b.url != settings.url) {
        remove_cache(&s, stack.id);
    }
    let user = c.user.user.id;
    let res = s
        .store
        .set_stack_git_with(
            stack.id,
            &settings,
            token,
            key,
            secret.as_deref(),
            Some(user),
            now_unix(),
        )
        .await;
    let detail = json!({
        "url": settings.url, "branch": settings.branch, "path": settings.path,
        "sync": settings.sync, "auto_deploy": settings.auto_deploy, "token": token_note,
        "key": key_note,
    });
    audit(&s, &c, "stack.git.update", &name, &res, Some(detail)).await;
    let git = res?;
    tokio::spawn({
        let (s, stack) = (s.clone(), stack.clone());
        async move {
            if let Err(e) = sync_stack(&s, &stack, SyncTrigger::Settings(user)).await {
                tracing::warn!(error = %e, stack = %stack.name, "git sync failed");
            }
        }
    });
    Ok(Json(GitSaved {
        status: status(&s, git),
        webhook_secret: secret,
    }))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/stacks/{stack}/git",
    operation_id = "detachStackGit",
    tag = "stacks",
    summary = "Stop taking the stack's Compose file from git",
    description = "The stack keeps its draft and revisions and becomes an inline stack.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 204, description = "Detached"))
)]
pub async fn detach(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    git_of(&s, &stack).await?;
    let res = s.store.detach_stack_git(stack.id).await;
    audit(
        &s,
        &c,
        "stack.git.update",
        &name,
        &res,
        Some(json!({ "detached": true })),
    )
    .await;
    res?;
    remove_cache(&s, stack.id);
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/git/sync",
    operation_id = "syncStackGit",
    tag = "stacks",
    summary = "Fetch the branch now",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "What the sync did", body = SyncOutcome))
)]
pub async fn sync(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<SyncOutcome>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Operate)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    git_of(&s, &stack).await?;
    Ok(Json(
        sync_stack(&s, &stack, SyncTrigger::Manual(c.user.user.id)).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/git/resume",
    operation_id = "resumeStackGit",
    tag = "stacks",
    summary = "Follow the branch again after a rollback",
    description = "Turns auto-sync back on and syncs; with auto-deploy on, the branch head is deployed.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "What the sync did", body = SyncOutcome))
)]
pub async fn resume(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<SyncOutcome>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Operate)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    git_of(&s, &stack).await?;
    let res = s.store.set_git_paused(stack.id, false).await;
    audit(
        &s,
        &c,
        "stack.git.update",
        &name,
        &res,
        Some(json!({ "paused": false })),
    )
    .await;
    res?;
    Ok(Json(
        sync_stack(&s, &stack, SyncTrigger::Resume(c.user.user.id)).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/envs/{env}/stacks/{stack}/git/secret",
    operation_id = "rotateStackGitSecret",
    tag = "stacks",
    summary = "Replace the webhook secret",
    description = "The new secret is returned once; the old one stops working.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "The new secret", body = SecretRotated))
)]
pub async fn rotate_secret(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<SecretRotated>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    git_of(&s, &stack).await?;
    let secret = new_secret();
    let res = s.store.rotate_stack_git_secret(stack.id, &secret).await;
    audit(
        &s,
        &c,
        "stack.git.update",
        &name,
        &res,
        Some(json!({ "secret": "rotated" })),
    )
    .await;
    res?;
    Ok(Json(SecretRotated {
        webhook_secret: secret,
    }))
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/stacks/{stack}/git/host-key",
    operation_id = "resetStackGitHostKey",
    tag = "stacks",
    summary = "Accept a new SSH host key",
    description = "Forgets the pinned host key; the next sync pins the key the server then shows.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 204, description = "Forgotten"))
)]
pub async fn reset_host_key(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<StatusCode> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let stack = stack_of(&s, &c, &name).await?;
    let git = git_of(&s, &stack).await?;
    let res = s.store.set_git_host_key(stack.id, None).await;
    audit(
        &s,
        &c,
        "stack.git.host_key.reset",
        &name,
        &res,
        Some(json!({ "was": git.ssh_host_key })),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}
