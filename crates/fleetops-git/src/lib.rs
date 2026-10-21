//! Git sources for stacks (stacks spec §8): a shallow fetch of one branch
//! into a per-stack cache, then a read of one file at its head. Everything
//! git is done here, with gitoxide, so no `git` binary is needed.

#[cfg(feature = "testing")]
pub mod testing;

use fleetops_core::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a stack's Compose file comes from.
#[derive(Clone)]
pub struct Source {
    pub url: String,
    pub branch: String,
    pub path: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("url", &self.url)
            .field("branch", &self.branch)
            .field("path", &self.path)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .finish()
    }
}

/// Caps on one fetch.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    /// The cache directory's largest size during a fetch.
    pub max_bytes: u64,
    /// The largest file that is read.
    pub max_file: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            max_bytes: 200 * 1024 * 1024,
            max_file: 1024 * 1024,
        }
    }
}

/// The file at the branch head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Full hex commit id.
    pub commit: String,
    pub content: String,
}

/// The username tokens are sent with; GitHub, GitLab and Gitea accept it.
pub const TOKEN_USER: &str = "x-access-token";

/// The file in the cache that records which URL it holds.
const URL_FILE: &str = "fleetops-url";

pub fn validate_url(url: &str, allow_http: bool) -> std::result::Result<(), String> {
    const HTTPS_ONLY: &str = "only https git URLs are supported";
    let u = url::Url::parse(url).map_err(|_| HTTPS_ONLY.to_string())?;
    match u.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => return Err(HTTPS_ONLY.into()),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("put the access token in the token field, not in the URL".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".into());
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err("the URL must not have a query or fragment".into());
    }
    Ok(())
}

pub fn validate_branch(b: &str) -> std::result::Result<(), String> {
    let ok = (1..=255).contains(&b.len())
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && !b.contains("..")
        && !b.starts_with('-')
        && !b.starts_with('/')
        && !b.ends_with('/');
    if ok {
        Ok(())
    } else {
        Err(format!("'{b}' is not a valid branch name"))
    }
}

pub fn validate_path(p: &str) -> std::result::Result<(), String> {
    let ok = (1..=512).contains(&p.len())
        && !p.starts_with('/')
        && !p.contains('\\')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    if ok {
        Ok(())
    } else {
        Err(format!(
            "'{p}' is not a valid path; use a relative path inside the repository"
        ))
    }
}

enum Fail {
    NeedsToken,
    RefusedToken,
    BranchMissing(String),
    Invalid(String),
    Network(String),
}

enum Cut {
    Timeout,
    TooBig,
}

/// Fetches `src.branch` into `cache` (depth 1) and reads `src.path` at its
/// head.
pub async fn fetch_file(cache: &Path, src: &Source, limits: Limits) -> Result<Fetched> {
    // gix's HTTP client uses rustls without a built-in provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let interrupt = Arc::new(AtomicBool::new(false));
    let mut job = tokio::task::spawn_blocking({
        let (cache, src, interrupt) = (cache.to_path_buf(), src.clone(), interrupt.clone());
        move || fetch_blocking(&cache, &src, limits.max_file, &interrupt)
    });
    let start = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let outcome = loop {
        tokio::select! {
            r = &mut job => break Ok(r),
            _ = tick.tick() => {
                if start.elapsed() >= limits.timeout {
                    break Err(Cut::Timeout);
                }
                if dir_size(cache) > limits.max_bytes {
                    break Err(Cut::TooBig);
                }
            }
        }
    };
    let joined = match outcome {
        Ok(j) => j,
        Err(cut) => {
            interrupt.store(true, Ordering::Relaxed);
            // The blocking fetch notices the interrupt on its next read;
            // its partial cache goes once it has stopped.
            let dir = cache.to_path_buf();
            tokio::spawn(async move {
                let _ = job.await;
                let _ = std::fs::remove_dir_all(&dir);
            });
            return Err(Error::Invalid(
                match cut {
                    Cut::Timeout => "fetching the repository took longer than 2 minutes",
                    Cut::TooBig => "the repository is larger than 200 MB at depth 1",
                }
                .into(),
            ));
        }
    };
    let scrub = |m: String| match &src.token {
        Some(t) if !t.is_empty() => m.replace(t.as_str(), "***"),
        _ => m,
    };
    match joined.map_err(|e| Error::Internal(format!("git fetch stopped: {e}")))? {
        Ok(f) => Ok(f),
        Err(Fail::NeedsToken) => Err(Error::Invalid(
            "the repository needs an access token".into(),
        )),
        Err(Fail::RefusedToken) => Err(Error::Invalid(
            "the repository refused the credentials".into(),
        )),
        Err(Fail::BranchMissing(b)) => Err(Error::Invalid(format!("branch '{b}' not found"))),
        Err(Fail::Invalid(m)) => Err(Error::Invalid(scrub(m))),
        Err(Fail::Network(m)) => Err(Error::Upstream {
            status: 502,
            message: scrub(format!("git fetch: {m}")),
        }),
    }
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Opens the cache, or starts it again when it is missing, broken or holds
/// another URL.
fn open_cache(cache: &Path, url: &str) -> std::result::Result<gix::Repository, Fail> {
    let local = |e: &dyn std::fmt::Display| Fail::Network(format!("the repository cache: {e}"));
    let same_url = std::fs::read_to_string(cache.join(URL_FILE)).is_ok_and(|u| u == url);
    if same_url {
        if let Ok(repo) = gix::open_opts(cache, gix::open::Options::isolated()) {
            return Ok(repo);
        }
    }
    if cache.exists() {
        std::fs::remove_dir_all(cache).map_err(|e| local(&e))?;
    }
    std::fs::create_dir_all(cache).map_err(|e| local(&e))?;
    gix::ThreadSafeRepository::init_opts(
        cache,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )
    .map_err(|e| local(&e))?;
    std::fs::write(cache.join(URL_FILE), url).map_err(|e| local(&e))?;
    gix::open_opts(cache, gix::open::Options::isolated()).map_err(|e| local(&e))
}

fn fetch_blocking(
    cache: &Path,
    src: &Source,
    max_file: u64,
    interrupt: &AtomicBool,
) -> std::result::Result<Fetched, Fail> {
    let repo = open_cache(cache, &src.url)?;
    let branch = &src.branch;
    let tracking = format!("refs/remotes/origin/{branch}");
    let spec = format!("+refs/heads/{branch}:{tracking}");
    let net = |e: gix::Error| Fail::Network(e.to_string());
    let remote = repo
        .remote_at(src.url.as_str())
        .map_err(net)?
        .with_refspecs([spec.as_str()], gix::remote::Direction::Fetch)
        .map_err(net)?;
    let asked = Arc::new(AtomicBool::new(false));
    let token = src.token.clone().filter(|t| !t.is_empty());
    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(net)?
        .with_credentials({
            let (asked, token) = (asked.clone(), token.clone());
            move |action| {
                if let gix::credentials::helper::Action::Get(ctx) = action {
                    asked.store(true, Ordering::Relaxed);
                    return Ok(token.as_ref().map(|t| gix::credentials::protocol::Outcome {
                        identity: gix::sec::identity::Account {
                            username: TOKEN_USER.into(),
                            password: t.clone(),
                            oauth_refresh_token: None,
                        },
                        next: ctx.into(),
                    }));
                }
                Ok(None)
            }
        });
    let auth_fail = |e: gix::Error| {
        if asked.load(Ordering::Relaxed) {
            if token.is_some() {
                Fail::RefusedToken
            } else {
                Fail::NeedsToken
            }
        } else {
            Fail::Network(e.to_string())
        }
    };
    let prepare = connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(auth_fail)?;
    let wanted = format!("refs/heads/{branch}");
    let advertised = prepare
        .ref_map()
        .mappings
        .iter()
        .any(|m| m.remote.as_name().is_some_and(|n| n == wanted.as_bytes()));
    if !advertised {
        return Err(Fail::BranchMissing(branch.clone()));
    }
    prepare
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            std::num::NonZeroU32::MIN,
        ))
        .receive(gix::progress::Discard, interrupt)
        .map_err(auth_fail)?;
    read_file(&repo, &tracking, &src.path, max_file)
}

fn read_file(
    repo: &gix::Repository,
    tracking: &str,
    path: &str,
    max_file: u64,
) -> std::result::Result<Fetched, Fail> {
    let internal =
        |e: &dyn std::fmt::Display| Fail::Network(format!("reading the fetched commit: {e}"));
    let mut head = repo.find_reference(tracking).map_err(|e| internal(&e))?;
    let commit = head.peel_to_commit().map_err(|e| internal(&e))?;
    let id = commit.id().to_string();
    let short = &id[..7];
    let tree = commit.tree().map_err(|e| internal(&e))?;
    let entry = tree
        .lookup_entry_by_path(path)
        .map_err(|e| internal(&e))?
        .ok_or_else(|| Fail::Invalid(format!("{path} not found at {short}")))?;
    if !entry.mode().is_blob() {
        return Err(Fail::Invalid(format!("{path} is not a file")));
    }
    let header = repo
        .find_header(entry.object_id())
        .map_err(|e| internal(&e))?;
    if header.size() > max_file {
        return Err(Fail::Invalid(format!(
            "{path} is larger than {}",
            human_size(max_file)
        )));
    }
    let blob = entry.object().map_err(|e| internal(&e))?;
    let content = String::from_utf8(blob.data.clone())
        .map_err(|_| Fail::Invalid(format!("{path} is not UTF-8")))?;
    Ok(Fetched {
        commit: id,
        content,
    })
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 && bytes.is_multiple_of(1024 * 1024) {
        format!("{} MiB", bytes / (1024 * 1024))
    } else {
        format!("{bytes} bytes")
    }
}

/// The cache directory of stack `id` under `root`.
pub fn cache_path(root: &Path, id: i64) -> PathBuf {
    root.join(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_rules() {
        let cases: &[(&str, bool, bool)] = &[
            ("https://h/r.git", false, true),
            ("http://h/r", false, false),
            ("http://h/r", true, true),
            ("ssh://h/r", true, false),
            ("git@h:r.git", true, false),
            ("file:///r", true, false),
            ("https://h/r?x=1", false, false),
        ];
        for (url, http, ok) in cases {
            assert_eq!(validate_url(url, *http).is_ok(), *ok, "{url}");
        }
        assert_eq!(
            validate_url("https://u:p@h/r", false).unwrap_err(),
            "put the access token in the token field, not in the URL"
        );
        assert_eq!(
            validate_url("ssh://h/r", false).unwrap_err(),
            "only https git URLs are supported"
        );
    }

    #[test]
    fn branch_rules() {
        for ok in ["main", "release/1.2", "feature_x-y"] {
            assert!(validate_branch(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "/x", "x/", "a..b", "a b", "a~1", &"b".repeat(256)] {
            assert!(validate_branch(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn path_rules() {
        for ok in ["compose.yaml", "ops/stack.yml"] {
            assert!(validate_path(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/etc/x", "../x", "a/../b", "a//b", "./a", "a\\b"] {
            assert!(validate_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn source_debug_hides_token() {
        let s = Source {
            url: "https://h/r".into(),
            branch: "main".into(),
            path: "compose.yaml".into(),
            token: Some("s3cret".into()),
        };
        assert!(!format!("{s:?}").contains("s3cret"));
    }
}
