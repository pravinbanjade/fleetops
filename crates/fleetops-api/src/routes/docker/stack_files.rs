//! A stack's Secrets tab (stack secrets spec §6): the secrets and configs
//! its draft declares, and the values stored for them. Secret values are
//! write-only: no response ever carries one.

use super::stack_ctx;
use super::stacks::audit;
use crate::openapi::EnvParam;
use crate::{auth::extract::CurrentUser, authz::Need, extract::ApiPath, ApiResult, AppState};
use axum::{body::Bytes, extract::State, http::StatusCode, Json};
use fleetops_core::compose::files::{repo_path, MAX_VALUE};
use fleetops_core::compose::{parse, parse_variables, FileKind, FileSource};
use fleetops_core::perm::{Row, Verb};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::StoredFile;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct StackFile {
    pub kind: FileKind,
    /// The top-level Compose key.
    pub name: String,
    /// `fleetops` (a Secrets-tab value), `variable`, `inline` or `repo`.
    pub source: &'static str,
    /// The stack variable, for a `variable` source.
    pub variable: Option<String>,
    /// The `file:` path as written, or for a `repo` source the path in the
    /// repository.
    pub path: Option<String>,
    /// Services that mount it.
    pub used_by: Vec<String>,
    /// Whether it has a value.
    pub set: bool,
    /// False for a stored value the draft no longer uses.
    pub used: bool,
    /// The text of a Secrets-tab or repo config; never a secret's.
    pub content: Option<String>,
    pub updated_at: Option<i64>,
    pub updated_by: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StackFileBody {
    /// UTF-8 text, at most 500 KiB.
    pub value: String,
}

fn kind_of(kind: &str) -> Result<FileKind> {
    match kind {
        "secret" => Ok(FileKind::Secret),
        "config" => Ok(FileKind::Config),
        _ => Err(Error::Invalid("kind must be secret or config".into())),
    }
}

fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "a name is 1 to 64 letters, digits, '.', '_' or '-'".into(),
        ))
    }
}

fn stored_item(f: &StoredFile, kind: FileKind, used: bool) -> StackFile {
    StackFile {
        kind,
        name: f.name.clone(),
        source: if f.source == "repo" {
            "repo"
        } else {
            "fleetops"
        },
        variable: None,
        path: None,
        used_by: vec![],
        set: true,
        used,
        content: (kind == FileKind::Config).then(|| f.value.clone()),
        updated_at: Some(f.updated_at),
        updated_by: f.updated_by.clone(),
    }
}

#[utoipa::path(
    get,
    path = "/envs/{env}/stacks/{stack}/files",
    operation_id = "listStackFiles",
    tag = "stacks",
    summary = "The stack's secrets and configs",
    description = "Every secret and config the draft declares, then stored values it no longer uses. Secret values are never returned.",
    params(EnvParam, ("stack" = String, Path, description = "Stack name")),
    responses((status = 200, description = "Secrets and configs", body = Vec<StackFile>))
)]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, name)): ApiPath<(String, String)>,
) -> ApiResult<Json<Vec<StackFile>>> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::View)).await?;
    let stack = s.store.stack_by_name(c.env.id(), &name).await?;
    let stored = s.store.stack_files(stack.id).await?;
    let git = match stack.source.as_str() {
        "git" => s.store.stack_git(stack.id).await?,
        _ => None,
    };
    let draft = s.store.draft(stack.id).await?;
    let parsed = draft
        .as_ref()
        .and_then(|d| parse(&d.compose, &d.variables, &stack.name).ok());
    let mut out = vec![];
    let mut shown: Vec<(&str, &str, &str)> = vec![];
    if let (Some(d), Some(parsed)) = (&draft, parsed) {
        let p = parsed.project;
        let vars = parse_variables(&d.variables).unwrap_or_default();
        for (kind, decls) in [
            (FileKind::Secret, &p.secrets),
            (FileKind::Config, &p.configs),
        ] {
            for (key, src) in decls {
                let used_by: Vec<String> = p
                    .services
                    .values()
                    .filter(|svc| svc.files.iter().any(|f| f.kind == kind && &f.source == key))
                    .map(|svc| svc.name.clone())
                    .collect();
                let mut item = StackFile {
                    kind,
                    name: key.clone(),
                    source: "fleetops",
                    variable: None,
                    path: None,
                    used: !used_by.is_empty(),
                    used_by,
                    set: false,
                    content: None,
                    updated_at: None,
                    updated_by: None,
                };
                match src {
                    FileSource::File { path } => {
                        let source = match (&git, kind) {
                            (Some(g), FileKind::Config) => {
                                item.source = "repo";
                                item.path = Some(repo_path(&g.path, path).unwrap_or(path.clone()));
                                "repo"
                            }
                            _ => {
                                item.path = Some(path.clone());
                                "fleetops"
                            }
                        };
                        if let Some(f) = stored.iter().find(|f| {
                            f.kind == kind.as_str() && f.source == source && &f.name == key
                        }) {
                            shown.push((kind.as_str(), source, f.name.as_str()));
                            item.set = true;
                            item.updated_at = Some(f.updated_at);
                            item.updated_by = f.updated_by.clone();
                            if kind == FileKind::Config {
                                item.content = Some(f.value.clone());
                            }
                        }
                    }
                    FileSource::Environment { var } => {
                        item.source = "variable";
                        item.set = vars.contains_key(var);
                        item.variable = Some(var.clone());
                    }
                    FileSource::Content { .. } => {
                        item.source = "inline";
                        item.set = true;
                    }
                }
                out.push(item);
            }
        }
    }
    for f in &stored {
        if shown.contains(&(f.kind.as_str(), f.source.as_str(), f.name.as_str())) {
            continue;
        }
        let kind = kind_of(&f.kind)?;
        // Stale repo rows are replaced by the next sync; only Secrets-tab
        // values are worth listing.
        if f.source == "fleetops" {
            out.push(stored_item(f, kind, false));
        }
    }
    Ok(Json(out))
}

#[utoipa::path(
    put,
    path = "/envs/{env}/stacks/{stack}/files/{kind}/{name}",
    operation_id = "setStackFile",
    tag = "stacks",
    summary = "Set a secret or config value",
    description = "Stores the value in the Secrets tab, replacing one of the same name. The stack shows changes not deployed until it is deployed again. A secret's value is never returned.",
    params(
        EnvParam,
        ("stack" = String, Path, description = "Stack name"),
        ("kind" = String, Path, description = "secret or config"),
        ("name" = String, Path, description = "The top-level Compose key"),
    ),
    request_body = StackFileBody,
    responses((status = 204, description = "Saved"))
)]
pub async fn set(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, stack_name, kind, name)): ApiPath<(String, String, String, String)>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let kind = kind_of(&kind)?;
    check_name(&name)?;
    let text = std::str::from_utf8(&body)
        .map_err(|_| Error::Invalid("the value must be UTF-8 text".into()))?;
    let body: StackFileBody = serde_json::from_str(text).map_err(|e| {
        if e.to_string().contains("surrogate") {
            Error::Invalid("the value must be UTF-8 text".into())
        } else {
            // serde's message can quote the value, so it is not passed on.
            Error::Invalid(r#"the body must be {"value": "<text>"}"#.into())
        }
    })?;
    if body.value.len() > MAX_VALUE {
        return Err(Error::Invalid("the value is larger than 500 KiB".into()).into());
    }
    let stack = s.store.stack_by_name(c.env.id(), &stack_name).await?;
    if kind == FileKind::Config && stack.source == "git" {
        return Err(
            Error::Invalid("configs of a git stack come from the repository".into()).into(),
        );
    }
    let res = s
        .store
        .set_stack_file(
            stack.id,
            kind.as_str(),
            &name,
            &body.value,
            Some(c.user.user.id),
            now_unix(),
        )
        .await;
    let action = format!("stack.{}.set", kind.as_str());
    audit(
        &s,
        &c,
        &action,
        &stack_name,
        &res,
        Some(json!({ "name": name })),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/envs/{env}/stacks/{stack}/files/{kind}/{name}",
    operation_id = "removeStackFile",
    tag = "stacks",
    summary = "Remove a stored secret or config value",
    params(
        EnvParam,
        ("stack" = String, Path, description = "Stack name"),
        ("kind" = String, Path, description = "secret or config"),
        ("name" = String, Path, description = "The top-level Compose key"),
    ),
    responses((status = 204, description = "Removed"))
)]
pub async fn remove(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath((env, stack_name, kind, name)): ApiPath<(String, String, String, String)>,
) -> ApiResult<StatusCode> {
    let c = stack_ctx(&s, cu, &env, Need::of(Row::Stacks, Verb::Create)).await?;
    let kind = kind_of(&kind)?;
    let stack = s.store.stack_by_name(c.env.id(), &stack_name).await?;
    let res = match s
        .store
        .remove_stack_file(stack.id, kind.as_str(), &name)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::NotFound(format!(
            "no stored {} named '{name}'",
            kind.as_str()
        ))),
        Err(e) => Err(e),
    };
    let action = format!("stack.{}.remove", kind.as_str());
    audit(
        &s,
        &c,
        &action,
        &stack_name,
        &res,
        Some(json!({ "name": name })),
    )
    .await;
    res?;
    Ok(StatusCode::NO_CONTENT)
}
