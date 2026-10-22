//! Stack builds (stack build spec §2–§4): the context tree ids a plan's
//! build hashes come from, the build specs a deploy runs, and the cleanup
//! of images no deploy needs any more.

use super::git::cache_dir;
use crate::AppState;
use fleetops_core::compose::build::{
    context_dir, Trees, IMAGE_LABEL_PROJECT, IMAGE_LABEL_SERVICE, MAX_CONTEXT, MAX_ENTRIES,
};
use fleetops_core::compose::plan::Plan;
use fleetops_core::compose::{ComposeIssue, Project};
use fleetops_core::docker::DockerOps;
use fleetops_core::streams::{BuildContext, BuildSpec};
use fleetops_core::{Error, Result};
use fleetops_store::{Builds, BuiltImage, GitRecord, StackRecord};
use std::collections::{BTreeMap, BTreeSet};

/// The Dockerfile name an inline Dockerfile gets inside a repo context.
const INLINE_DOCKERFILE: &str = ".fleetops-inline.Dockerfile";

/// What a plan's builds come from.
#[derive(Debug, Clone, Default)]
pub(crate) struct BuildInputs {
    pub trees: Trees,
    /// Whether `trees` are authoritative (a git stack, or a revision's
    /// record); otherwise only `dockerfile_inline` can build.
    pub git: bool,
    /// The commit whose tree holds the contexts.
    pub commit: Option<String>,
    /// For a rollback: the revision, for its error messages.
    pub revision: Option<i64>,
}

/// A build a deploy runs, with notes for its log.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub spec: BuildSpec,
    pub notes: Vec<String>,
}

fn has_build(project: &Project) -> bool {
    project.services.values().any(|s| s.build.is_some())
}

fn git_source(git: &GitRecord, token: Option<String>) -> fleetops_git::Source {
    fleetops_git::Source {
        url: git.url.clone(),
        branch: git.branch.clone(),
        path: git.path.clone(),
        token,
    }
}

/// Makes sure the stack's cache holds `commit`, fetching it by id if not.
async fn ensure_commit(
    s: &AppState,
    stack: &StackRecord,
    git: &GitRecord,
    commit: &str,
) -> Result<()> {
    let cache = cache_dir(s, stack.id)?;
    if fleetops_git::has_commit(&cache, commit) {
        return Ok(());
    }
    let token = s.store.stack_git_token(stack.id).await?;
    fleetops_git::fetch_commit(&cache, &git_source(git, token), commit, s.config.git_limits).await
}

/// The inputs of a draft's builds: on a git stack, each build service's
/// context tree id at the draft's commit. Read errors are issues.
pub(crate) async fn draft_inputs(
    s: &AppState,
    stack: &StackRecord,
    commit: Option<&str>,
    project: &Project,
) -> Result<(BuildInputs, Vec<ComposeIssue>)> {
    let git = match stack.source.as_str() {
        "git" => s.store.stack_git(stack.id).await?,
        _ => None,
    };
    let (Some(git), Some(commit), true) = (git, commit, has_build(project)) else {
        return Ok((
            BuildInputs {
                git: stack.source == "git",
                commit: commit.map(String::from),
                ..Default::default()
            },
            vec![],
        ));
    };
    let mut issues = vec![];
    if let Err(e) = ensure_commit(s, stack, &git, commit).await {
        for name in project
            .services
            .iter()
            .filter(|(_, v)| v.build.is_some())
            .map(|(n, _)| n)
        {
            issues.push(ComposeIssue::new(
                format!("services.{name}.build.context"),
                e.detail(),
            ));
        }
        return Ok((BuildInputs::default(), issues));
    }
    let cache = cache_dir(s, stack.id)?;
    let mut trees = Trees::new();
    for (name, svc) in &project.services {
        let Some(b) = &svc.build else { continue };
        let path = format!("services.{name}.build.context");
        let dir = match context_dir(&git.path, &b.context) {
            Ok(d) => d,
            Err(e) => {
                issues.push(ComposeIssue::new(path, e));
                continue;
            }
        };
        let tree = match fleetops_git::tree_ids(&cache, commit, &[dir]).await {
            Ok(mut ids) => ids.remove(0),
            Err(e) => {
                issues.push(ComposeIssue::new(path, e.detail()));
                continue;
            }
        };
        if b.dockerfile_inline.is_none() {
            let file = b.dockerfile.clone().unwrap_or_else(|| "Dockerfile".into());
            if !fleetops_git::tree_has_file(&cache, &tree, &file).await? {
                issues.push(ComposeIssue::new(
                    format!("services.{name}.build.dockerfile"),
                    format!(
                        "{file} not found in {} at {}",
                        b.context,
                        &commit[..commit.len().min(7)]
                    ),
                ));
            }
        }
        trees.insert(name.clone(), tree);
    }
    Ok((
        BuildInputs {
            trees,
            git: true,
            commit: Some(commit.to_string()),
            revision: None,
        },
        issues,
    ))
}

/// The inputs of a rollback to revision `number`: what it recorded.
pub(crate) async fn revision_inputs(
    s: &AppState,
    stack: &StackRecord,
    number: i64,
) -> Result<BuildInputs> {
    let builds = s.store.revision_builds(stack.id, number).await?;
    let commit = s.store.revision(stack.id, number).await?.git_commit;
    Ok(BuildInputs {
        git: !builds.is_empty(),
        trees: builds.into_iter().map(|(n, b)| (n, b.tree)).collect(),
        commit,
        revision: Some(number),
    })
}

/// What `project` builds, to record with its revision.
pub(crate) fn builds_of(project: &Project, inputs: &BuildInputs) -> Builds {
    project
        .services
        .iter()
        .filter(|(_, s)| s.build.is_some())
        .map(|(n, s)| {
            (
                n.clone(),
                BuiltImage {
                    tree: inputs.trees.get(n).cloned().unwrap_or_default(),
                    image: s.image.clone(),
                },
            )
        })
        .collect()
}

/// For a rollback whose image is gone, checks the context can still be
/// read: the stack must still have its git source, and the commit must be
/// fetchable.
pub(crate) async fn check_rebuilds(
    s: &AppState,
    stack: &StackRecord,
    project: &Project,
    plan: &Plan,
    inputs: &BuildInputs,
) -> Result<Vec<ComposeIssue>> {
    let Some(n) = inputs.revision else {
        return Ok(vec![]);
    };
    let mut issues = vec![];
    for img in plan.images.iter().filter(|i| i.build) {
        let needs_repo = project.services.iter().any(|(name, svc)| {
            svc.image == img.image && inputs.trees.get(name).is_some_and(|t| !t.is_empty())
        });
        if !needs_repo {
            continue;
        }
        let git = match stack.source.as_str() {
            "git" => s.store.stack_git(stack.id).await?,
            _ => None,
        };
        let path = "services".to_string();
        match (git, &inputs.commit) {
            (Some(git), Some(commit)) => {
                if ensure_commit(s, stack, &git, commit).await.is_err() {
                    issues.push(ComposeIssue::new(
                        path,
                        format!(
                            "the image built for revision {n} is gone and commit {} cannot be fetched; deploy from the branch instead",
                            &commit[..commit.len().min(7)]
                        ),
                    ));
                }
            }
            _ => issues.push(ComposeIssue::new(
                path,
                format!("the image built for revision {n} is gone and this stack no longer has a git source"),
            )),
        }
        break;
    }
    Ok(issues)
}

/// Adds `name` with `content` to a tar.
fn with_file(tar: &[u8], name: &str, content: &[u8]) -> Result<Vec<u8>> {
    let io = |e: std::io::Error| Error::Internal(format!("building the context: {e}"));
    let mut out = tar::Builder::new(Vec::new());
    let mut a = tar::Archive::new(tar);
    for e in a.entries().map_err(io)? {
        let mut e = e.map_err(io)?;
        let mut h = e.header().clone();
        let path = e.path().map_err(io)?.into_owned();
        if h.entry_type().is_symlink() {
            let target = e.link_name().map_err(io)?.unwrap_or_default().into_owned();
            out.append_link(&mut h, &path, target).map_err(io)?;
        } else {
            out.append_data(&mut h, &path, &mut e).map_err(io)?;
        }
    }
    let mut h = tar::Header::new_gnu();
    h.set_size(content.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(0);
    out.append_data(&mut h, name, content).map_err(io)?;
    out.into_inner().map_err(io)
}

/// A build spec for every image the plan builds, one per tag.
pub(crate) async fn prepare(
    s: &AppState,
    stack: &StackRecord,
    project: &Project,
    plan: &Plan,
    inputs: &BuildInputs,
) -> Result<BTreeMap<String, Prepared>> {
    let mut out = BTreeMap::new();
    for img in plan.images.iter().filter(|i| i.build) {
        let Some((name, svc)) = project
            .services
            .iter()
            .find(|(_, s)| s.image == img.image && s.build.is_some())
        else {
            continue;
        };
        let b = svc.build.as_ref().expect("filtered");
        let tree = inputs.trees.get(name).cloned().unwrap_or_default();
        let labels = BTreeMap::from([
            (IMAGE_LABEL_PROJECT.to_string(), stack.name.clone()),
            (IMAGE_LABEL_SERVICE.to_string(), name.clone()),
        ]);
        let mut notes = vec![];
        let (context, dockerfile) = if tree.is_empty() {
            let text = b.dockerfile_inline.clone().unwrap_or_default();
            (BuildContext::Dockerfile(text.into_bytes()), None)
        } else {
            let git = s.store.stack_git(stack.id).await?.ok_or_else(|| {
                Error::Invalid(format!("stack '{}' has no git source", stack.name))
            })?;
            let commit = inputs
                .commit
                .clone()
                .ok_or_else(|| Error::Invalid("the build has no commit".into()))?;
            ensure_commit(s, stack, &git, &commit).await?;
            let limits = fleetops_git::TarLimits {
                max_bytes: MAX_CONTEXT,
                max_entries: MAX_ENTRIES,
            };
            // By tree id, not by path: the content is exactly what was
            // hashed, whatever the stack's path setting says now.
            let keep = vec![b.dockerfile.clone().unwrap_or_else(|| "Dockerfile".into())];
            let out =
                fleetops_git::tar_tree(&cache_dir(s, stack.id)?, &tree, &b.context, &keep, limits)
                    .await?;
            notes.extend(
                out.skipped
                    .iter()
                    .map(|p| format!("left out submodule {p}")),
            );
            match &b.dockerfile_inline {
                Some(text) => (
                    BuildContext::Tar(with_file(&out.tar, INLINE_DOCKERFILE, text.as_bytes())?),
                    Some(INLINE_DOCKERFILE.to_string()),
                ),
                None => (BuildContext::Tar(out.tar), b.dockerfile.clone()),
            }
        };
        out.insert(
            img.image.clone(),
            Prepared {
                spec: BuildSpec {
                    context,
                    tag: Some(img.image.clone()),
                    dockerfile,
                    args: b.args.clone(),
                    target: b.target.clone(),
                    labels,
                },
                notes,
            },
        );
    }
    Ok(out)
}

fn repo_of(tag: &str) -> &str {
    match tag.rfind(':') {
        Some(i) if !tag[i..].contains('/') => &tag[..i],
        _ => tag,
    }
}

/// Removes this stack's built images that no tag in `keep` names: images
/// in the same repositories, labelled with the stack. Never fails; returns
/// lines for the deploy log.
pub(crate) async fn cleanup(
    ops: &dyn DockerOps,
    stack: &str,
    keep: &BTreeSet<String>,
) -> Vec<String> {
    let repos: BTreeSet<&str> = keep.iter().map(|t| repo_of(t)).collect();
    if repos.is_empty() {
        return vec![];
    }
    let images = match ops.list_images().await {
        Ok(i) => i,
        Err(e) => return vec![format!("could not list images: {e}")],
    };
    let mut lines = vec![];
    for tag in images.into_iter().flat_map(|i| i.repo_tags) {
        if keep.contains(&tag) || !repos.contains(repo_of(&tag)) {
            continue;
        }
        let ours = ops
            .inspect_image(&tag)
            .await
            .ok()
            .and_then(|v| {
                v["Config"]["Labels"][IMAGE_LABEL_PROJECT]
                    .as_str()
                    .map(String::from)
            })
            .is_some_and(|p| p == stack);
        if !ours {
            continue;
        }
        match ops.remove_image(&tag, false).await {
            Ok(_) => lines.push(format!("removed image {tag}")),
            Err(e) => lines.push(format!("could not remove image {tag}: {e}")),
        }
    }
    lines
}
