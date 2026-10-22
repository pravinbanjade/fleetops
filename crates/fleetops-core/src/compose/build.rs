//! Stack builds (stack build spec §1–§3): a keyed hash of what a service's
//! image is built from, and the image tag that carries it, so the config
//! hash follows the code. Pure; the caller reads the context tree ids.

use super::model::*;
use super::ComposeIssue;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;

pub const MAX_CONTEXT: usize = 100 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
pub const IMAGE_LABEL_PROJECT: &str = "io.fleetops.stack.project";
pub const IMAGE_LABEL_SERVICE: &str = "io.fleetops.stack.service";

/// Service name → git tree id of its build context ("" on an inline stack).
pub type Trees = BTreeMap<String, String>;

/// Hex HMAC-SHA256 of everything the image is built from. The stack and
/// service are in it, so two stacks never share a tag.
pub fn build_hash(key: &[u8; 32], stack: &str, service: &str, tree: &str, b: &Build) -> String {
    let input = serde_json::to_vec(&(
        stack,
        service,
        tree,
        &b.dockerfile,
        &b.dockerfile_inline,
        &b.args,
        &b.target,
    ))
    .expect("strings serialise");
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(&input);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `<repo>:<first 12 of hash>`: the repository of `image` (its tag or
/// digest dropped), or `<stack>-<service>`.
pub fn image_tag(stack: &str, service: &str, image: &str, hash: &str) -> String {
    let repo = if image.is_empty() {
        format!("{stack}-{service}").to_ascii_lowercase()
    } else {
        let no_digest = image.split('@').next().unwrap_or(image);
        match no_digest.rfind(':') {
            Some(i) if !no_digest[i..].contains('/') => no_digest[..i].to_string(),
            _ => no_digest.to_string(),
        }
    };
    format!("{repo}:{}", &hash[..hash.len().min(12)])
}

fn inline_ok(b: &Build) -> bool {
    matches!(b.context.as_str(), "." | "./" | "") && b.dockerfile_inline.is_some()
}

/// Fills each build service's hash and image tag. On an inline stack only
/// `dockerfile_inline` without a context can build.
pub fn apply(
    project: &mut Project,
    trees: &Trees,
    git: bool,
    key: &[u8; 32],
) -> Result<(), Vec<ComposeIssue>> {
    let stack = project.name.clone();
    let mut issues = vec![];
    for (name, svc) in project.services.iter_mut() {
        let Some(b) = svc.build.as_mut() else {
            continue;
        };
        let tree = if git {
            match trees.get(name) {
                Some(t) => t.clone(),
                None => {
                    issues.push(ComposeIssue::new(
                        format!("services.{name}.build.context"),
                        "the build context could not be read",
                    ));
                    continue;
                }
            }
        } else {
            if !inline_ok(b) {
                issues.push(ComposeIssue::new(
                    format!("services.{name}.build"),
                    format!("service {name}: a build context needs a git stack; use dockerfile_inline for a Dockerfile without files"),
                ));
                continue;
            }
            String::new()
        };
        b.hash = build_hash(key, &stack, name, &tree, b);
        svc.image = image_tag(&stack, name, &svc.image, &b.hash);
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// The repository directory of a build context, relative to the Compose
/// file's directory; `""` is the repository root.
pub fn context_dir(compose_path: &str, context: &str) -> Result<String, String> {
    if context.starts_with('/') || context.contains("://") {
        return Err(format!(
            "{context} must be a path relative to the Compose file"
        ));
    }
    let mut parts: Vec<&str> = compose_path.split('/').collect();
    parts.pop();
    parts.retain(|p| !p.is_empty() && *p != ".");
    for p in context.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("{context} is outside the repository"));
                }
            }
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}
