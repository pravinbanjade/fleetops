//! `POST /stacks/hooks/{id}`: push webhooks from git hosts (stacks spec §8).
//! No session and no CSRF; the stack's secret is the credential. Every
//! refusal looks the same, so the route never says whether a stack exists.

use crate::auth::extract::ClientMeta;
use crate::auth::limiter::limiter_key;
use crate::extract::{ApiPath, ApiQuery};
use crate::stacks::git::{sync_stack, SyncTrigger};
use crate::stacks::start::GIT_ACTOR;
use crate::stacks::webhook::{pushed_branch, verify, Provider};
use crate::{ApiResult, AppState};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use fleetops_core::now_unix;
use fleetops_store::NewAudit;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use utoipa::{IntoParams, ToSchema};

/// Largest webhook body accepted.
pub const HOOK_BODY_MAX: usize = 5 * 1024 * 1024;
/// Requests per stack and client address in [`HOOK_WINDOW`].
pub const HOOK_MAX_REQUESTS: u32 = 30;
pub const HOOK_WINDOW: Duration = Duration::from_secs(60);
/// Requests per client address in [`HOOK_WINDOW`], whatever the stack, so
/// walking ids cannot flood the audit log.
pub const HOOK_MAX_PER_ADDRESS: u32 = 120;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HookQuery {
    /// The webhook secret, for senders that cannot sign.
    pub secret: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct HookAccepted {
    /// accepted | ignored (a push to another branch)
    pub status: &'static str,
}

async fn audit(s: &AppState, id: i64, env: Option<i64>, result: &str, detail: Value) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: None,
        environment_id: env,
        action: "stack.git.webhook",
        target_type: Some("stack"),
        target_id: Some(&id.to_string()),
        result,
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

#[derive(Serialize, ToSchema)]
pub struct HookRefused {
    /// Always `unauthorized`.
    pub error: &'static str,
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(HookRefused {
            error: "unauthorized",
        }),
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/stacks/hooks/{id}",
    operation_id = "stackWebhook",
    tag = "stacks",
    summary = "Push webhook for a git-backed stack",
    description = "Signed with the stack's webhook secret: GitHub `X-Hub-Signature-256`, Gitea or Forgejo `X-Gitea-Signature`, GitLab `X-Gitlab-Token`, or `?secret=`. Starts a sync in the background.",
    security(()),
    params(("id" = i64, Path, description = "Stack id"), HookQuery),
    responses(
        (status = 202, description = "Accepted, or ignored for another branch", body = HookAccepted),
        (status = 401, description = "Not verified; the same for every reason", body = HookRefused),
    )
)]
pub async fn hook(
    State(s): State<AppState>,
    meta: ClientMeta,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(q): ApiQuery<HookQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let now = Instant::now();
    s.hook_address_limiter.try_acquire(&meta.ip, now)?;
    s.hook_limiter
        .try_acquire(&limiter_key(&id.to_string(), &meta.ip), now)?;
    let refuse = |reason: &'static str, env: Option<i64>, provider: Option<Provider>| {
        let s = s.clone();
        async move {
            let mut d = json!({ "reason": reason, "by": GIT_ACTOR });
            if let Some(p) = provider {
                d["provider"] = json!(p.name());
            }
            audit(&s, id, env, "failure", d).await;
            Ok(unauthorized())
        }
    };
    let Ok(stack) = s.store.stack_by_id(id).await else {
        return refuse("no such stack", None, None).await;
    };
    let env = Some(stack.env_id);
    let (Ok(Some(git)), Ok(Some(secret))) = (
        s.store.stack_git(id).await,
        s.store.stack_git_secret(id).await,
    ) else {
        return refuse("not a git stack", env, None).await;
    };
    if git.sync != "webhook" {
        return refuse("webhooks are off for this stack", env, None).await;
    }
    let provider = match verify(&headers, q.secret.as_deref(), &body, &secret) {
        Ok(p) => p,
        Err(reason) => return refuse(reason, env, None).await,
    };
    let branch = pushed_branch(&body);
    if branch.as_deref().is_some_and(|b| b != git.branch) {
        audit(
            &s,
            id,
            env,
            "success",
            json!({ "provider": provider.name(), "ignored": "another branch", "branch": branch, "by": GIT_ACTOR }),
        )
        .await;
        return Ok((
            StatusCode::ACCEPTED,
            Json(HookAccepted { status: "ignored" }),
        )
            .into_response());
    }
    audit(
        &s,
        id,
        env,
        "success",
        json!({ "provider": provider.name(), "branch": branch, "by": GIT_ACTOR }),
    )
    .await;
    tokio::spawn({
        let s = s.clone();
        async move {
            if let Err(e) = sync_stack(&s, &stack, SyncTrigger::Webhook).await {
                tracing::warn!(error = %e, stack = %stack.name, "git sync failed");
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(HookAccepted { status: "accepted" }),
    )
        .into_response())
}
