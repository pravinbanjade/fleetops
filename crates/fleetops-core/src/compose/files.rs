//! Secrets and configs (stack secrets spec §1–§4): the values a stack's
//! services use, a keyed digest of each in the service's config hash, and
//! the tar archive copied into a container before it starts. Pure; the
//! caller picks the values (Secrets tab, repo, or a revision's snapshot).

use super::interp::parse_variables;
use super::model::*;
use super::ComposeIssue;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;

pub const MAX_VALUE: usize = 500 * 1024;
pub const MAX_TOTAL: usize = 4 * 1024 * 1024;
pub const MAX_FILES: usize = 64;
/// A JSON list of a container's secret targets (never contents), so the
/// Files tab can refuse them without a database lookup.
pub const LABEL_SECRETS: &str = "io.fleetops.stack.secrets";

/// Every secret and config of a container, with its keyed digest (never
/// its content): the Files tab refuses a download whose content matches a
/// secret's, whatever path reached it, and stack containers are not
/// recreated outside a deploy, which would drop the files.
pub const LABEL_FILES: &str = "io.fleetops.stack.files";

/// One entry of [`LABEL_FILES`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LabelledFile {
    pub kind: FileKind,
    pub name: String,
    pub digest: String,
}

/// The entries of a container's [`LABEL_FILES`]; empty without one.
pub fn labelled_files(labels: &BTreeMap<String, String>) -> Vec<LabelledFile> {
    labels
        .get(LABEL_FILES)
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_default()
}

/// Values by (kind, name).
pub type FileValues = BTreeMap<(FileKind, String), String>;

fn top_key(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Secret => "secrets",
        FileKind::Config => "configs",
    }
}

/// Resolves the content of every secret and config a service uses, from
/// `values` (for `file:` sources), the stack variables or inline content,
/// and fills each mount's digest. Declarations no service uses are left
/// alone.
pub fn resolve(
    project: &mut Project,
    variables: &[String],
    values: &FileValues,
    key: &[u8; 32],
) -> Result<FileValues, Vec<ComposeIssue>> {
    let vars = parse_variables(variables).map_err(|e| vec![e])?;
    let used: std::collections::BTreeSet<(FileKind, String)> = project
        .services
        .values()
        .flat_map(|s| s.files.iter().map(|f| (f.kind, f.source.clone())))
        .collect();
    let mut out = FileValues::new();
    let mut issues = vec![];
    for (kind, name) in &used {
        let decls = match kind {
            FileKind::Secret => &project.secrets,
            FileKind::Config => &project.configs,
        };
        let path = format!("{}.{name}", top_key(*kind));
        let what = kind.as_str();
        let value = match decls.get(name) {
            Some(FileSource::File { .. }) => match values.get(&(*kind, name.clone())) {
                Some(v) => v.clone(),
                None => {
                    issues.push(ComposeIssue::new(
                        &path,
                        format!("{what} {name} has no value: set it in the Secrets tab"),
                    ));
                    continue;
                }
            },
            Some(FileSource::Environment { var }) => match vars.get(var) {
                Some(v) => v.clone(),
                None => {
                    issues.push(ComposeIssue::new(
                        &path,
                        format!("{what} {name}: variable {var} is not set"),
                    ));
                    continue;
                }
            },
            Some(FileSource::Content { text }) => text.clone(),
            // Swarm holds an external object's content; it keeps no digest.
            Some(FileSource::External { .. }) => continue,
            // The parser refuses a mount of an undeclared source.
            None => continue,
        };
        if value.len() > MAX_VALUE {
            issues.push(ComposeIssue::new(
                &path,
                format!("{what} {name} is larger than 500 KiB"),
            ));
            continue;
        }
        out.insert((*kind, name.clone()), value);
    }
    if used.len() > MAX_FILES {
        issues.push(ComposeIssue::new(
            "services",
            format!(
                "a stack can use at most 64 files; this one uses {}",
                used.len()
            ),
        ));
    }
    if out.values().map(String::len).sum::<usize>() > MAX_TOTAL {
        issues.push(ComposeIssue::new(
            "services",
            "secrets and configs add up to more than 4 MiB",
        ));
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    for svc in project.services.values_mut() {
        for f in &mut svc.files {
            if let Some(content) = out.get(&(f.kind, f.source.clone())) {
                f.digest = digest(key, f.kind, &f.source, content);
            }
        }
    }
    Ok(out)
}

/// Hex HMAC-SHA256 of `kind \0 name \0 content`. Keyed, so the config-hash
/// label cannot be used to guess a weak value offline.
pub fn digest(key: &[u8; 32], kind: FileKind, name: &str, content: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(kind.as_str().as_bytes());
    mac.update(b"\0");
    mac.update(name.as_bytes());
    mac.update(b"\0");
    mac.update(content.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A tar of the service's files, to upload at `/`. File entries only: a
/// directory entry would reset an existing directory's owner and mode, and
/// Docker creates missing parents itself.
pub fn archive(svc: &Service, contents: &FileValues) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for f in &svc.files {
        let Some(content) = contents.get(&(f.kind, f.source.clone())) else {
            continue;
        };
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(content.len() as u64);
        h.set_mode(f.mode);
        h.set_uid(f.uid.parse().unwrap_or(0));
        h.set_gid(f.gid.parse().unwrap_or(0));
        h.set_mtime(0);
        b.append_data(&mut h, f.target.trim_start_matches('/'), content.as_bytes())
            .expect("writing to memory");
    }
    b.into_inner().expect("writing to memory")
}

/// The secret targets recorded on a container by [`LABEL_SECRETS`].
pub fn secret_paths(labels: &BTreeMap<String, String>) -> Vec<String> {
    labels
        .get(LABEL_SECRETS)
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_default()
}

/// The repo path of a config `file:`, relative to the Compose file's
/// directory. Absolute paths and paths leaving the repository are refused.
pub fn repo_path(compose_path: &str, file: &str) -> Result<String, String> {
    if file.starts_with('/') {
        return Err(format!(
            "{file} must be a path relative to the Compose file"
        ));
    }
    let mut parts: Vec<&str> = compose_path.split('/').collect();
    parts.pop();
    parts.retain(|p| !p.is_empty() && *p != ".");
    for p in file.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("{file} is outside the repository"));
                }
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() || file.ends_with('/') {
        return Err(format!("{file} is not a file path"));
    }
    Ok(parts.join("/"))
}
