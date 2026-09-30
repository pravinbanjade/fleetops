//! Image pull and build: long-running, so they start a background task and
//! answer `202 {task_id}` at once (spec §5.1).

use super::ctx;
use crate::{
    auth::extract::CurrentUser,
    authz::Action,
    extract::{ApiJson, ApiPath},
    tasks::{NewTask, TaskInfo, TaskStatus},
    ApiResult, AppState,
};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use fleetops_core::docker::split_image_reference;
use fleetops_core::streams::{BoxStream, BuildContext, BuildSpec};
use fleetops_core::{now_unix, Error};
use fleetops_store::NewAudit;
use serde::Deserialize;
use serde_json::{json, Value};

/// Largest accepted build upload.
pub const BUILD_UPLOAD_MAX: usize = 100 * 1024 * 1024;

#[derive(Deserialize)]
pub struct PullBody {
    /// `nginx`, `nginx:1.27`, `ghcr.io/org/app:2` or `repo@sha256:...`.
    pub image: String,
    /// Overrides any tag in `image`.
    #[serde(default)]
    pub tag: Option<String>,
}

/// Starts `stream` as a task and audits its outcome when it finishes.
fn start_task(
    s: &AppState,
    user_id: i64,
    env: (&str, i64),
    kind: &'static str,
    target: String,
    stream: BoxStream<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let store = s.store.clone();
    let (env_name, env_id) = env;
    let task = s.tasks.spawn(
        NewTask {
            kind,
            env: env_name.to_string(),
            target: target.clone(),
            owner: user_id,
        },
        stream,
        move |info: TaskInfo| async move {
            let (result, detail) = match info.status {
                TaskStatus::Succeeded => ("success", Some(json!({ "task_id": info.id }))),
                _ => (
                    "failure",
                    Some(json!({ "task_id": info.id, "error": info.error })),
                ),
            };
            let entry = NewAudit {
                ts: now_unix(),
                user_id: Some(user_id),
                environment_id: Some(env_id),
                action: kind,
                target_type: Some("image"),
                target_id: Some(&target),
                result,
                detail,
            };
            if let Err(e) = store.insert_audit(&entry).await {
                tracing::error!(error = %e, "failed to write audit log");
            }
        },
    )?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "task_id": task.info().id })),
    ))
}

pub async fn pull(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    ApiJson(body): ApiJson<PullBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let (image, parsed_tag) = split_image_reference(&body.image)?;
    let tag = body.tag.filter(|t| !t.is_empty()).unwrap_or(parsed_tag);
    let target = if tag.is_empty() {
        image.clone()
    } else {
        format!("{image}:{tag}")
    };
    let stream = c.ops.pull_image(&image, &tag);
    start_task(
        &s,
        c.user.user.id,
        (c.env.name(), c.env.id()),
        "image.pull",
        target,
        stream,
    )
}

fn bad(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("bad upload: {e}"))
}

/// Multipart fields: `context` (a tar of the build context) or `dockerfile`
/// (a lone Dockerfile), plus optional `tag` and `dockerfile_path`.
pub async fn build(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(env): ApiPath<String>,
    mut form: Multipart,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let c = ctx(&s, cu, &env, Action::DockerWrite).await?;
    let (mut context, mut tag, mut dockerfile) = (None, None, None);
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(bad)?;
        match name.as_str() {
            "context" | "dockerfile" if context.is_some() => {
                return Err(Error::Invalid(
                    "send either `context` or `dockerfile`, not both".into(),
                )
                .into())
            }
            "context" => context = Some(BuildContext::Tar(bytes.to_vec())),
            "dockerfile" => context = Some(BuildContext::Dockerfile(bytes.to_vec())),
            "tag" => tag = Some(String::from_utf8(bytes.to_vec()).map_err(bad)?),
            "dockerfile_path" => dockerfile = Some(String::from_utf8(bytes.to_vec()).map_err(bad)?),
            other => return Err(Error::Invalid(format!("unexpected field `{other}`")).into()),
        }
    }
    let context = context
        .ok_or_else(|| Error::Invalid("a `context` tar or a `dockerfile` is required".into()))?;
    let tag = tag.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(t) = &tag {
        split_image_reference(t)?;
    }
    let target = tag.clone().unwrap_or_else(|| "(untagged)".into());
    let stream = c.ops.build_image(BuildSpec {
        context,
        tag,
        dockerfile: dockerfile.filter(|d| !d.is_empty()),
    });
    start_task(
        &s,
        c.user.user.id,
        (c.env.name(), c.env.id()),
        "image.build",
        target,
        stream,
    )
}
