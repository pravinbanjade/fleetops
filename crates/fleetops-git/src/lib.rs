//! Git sources for stacks (stacks spec §8): a shallow fetch of one branch
//! into a per-stack cache, then a read of one file at its head. Everything
//! git is done here, with gitoxide, so no `git` binary is needed.

#[cfg(feature = "testing")]
pub mod testing;

mod dockerignore;
mod keys;

pub use keys::{generate_key, parse_key, KeyPair};

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

/// Packs a cache may hold. Each fetch adds one, so past this the cache is
/// dropped and the next fetch starts again at depth 1.
pub const MAX_PACKS: usize = 16;

/// `m` with the token taken out, raw, percent-encoded and base64-encoded
/// (alone or as basic-auth credentials).
fn scrub(m: String, token: Option<&str>) -> String {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};
    use base64::Engine;
    let Some(t) = token.filter(|t| !t.is_empty()) else {
        return m;
    };
    let pct: String = t
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let basic = format!("{TOKEN_USER}:{t}");
    let mut forms = vec![t.to_string(), pct.clone(), pct.to_lowercase()];
    for v in [t, basic.as_str()] {
        for e in [STANDARD.encode(v), URL_SAFE.encode(v)] {
            forms.push(e.trim_end_matches('=').to_string());
            forms.push(e);
        }
    }
    // Longest first, so a shorter form never leaves part of a longer one.
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.into_iter().fold(m, |m, f| m.replace(&f, "***"))
}

fn packs(cache: &Path) -> usize {
    std::fs::read_dir(cache.join("objects/pack")).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "pack"))
            .count()
    })
}

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

/// One lock per cache directory: a fetch waits until an earlier one,
/// interrupted or not, has stopped writing there and cleaned up.
fn dir_lock(cache: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let mut m = LOCKS.get_or_init(Default::default).lock().unwrap();
    // Forget directories nobody is using.
    m.retain(|_, l| Arc::strong_count(l) > 1);
    m.entry(cache.to_path_buf()).or_default().clone()
}

/// Fetches `src.branch` into `cache` (depth 1) and reads `src.path` at its
/// head.
pub async fn fetch_file(cache: &Path, src: &Source, limits: Limits) -> Result<Fetched> {
    // gix's HTTP client uses rustls without a built-in provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let guard = dir_lock(cache).lock_owned().await;
    // Each fetch adds a pack: past the cap the cache starts again. Done
    // before the fetch, so the cache always holds the commit it returns,
    // for [`read_at`].
    if packs(cache) >= MAX_PACKS {
        let _ = std::fs::remove_dir_all(cache);
    }
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
            // its partial cache goes once it has stopped, and the next fetch
            // into this directory waits for that.
            let dir = cache.to_path_buf();
            tokio::spawn(async move {
                let _ = job.await;
                let _ = std::fs::remove_dir_all(&dir);
                drop(guard);
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
    let scrub = |m: String| scrub(m, src.token.as_deref());
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
        Err(Fail::Network(m)) => {
            // A cache a failed fetch may have left half-written starts again.
            let _ = std::fs::remove_dir_all(cache);
            Err(Error::Upstream {
                status: 502,
                message: scrub(format!("git fetch: {m}")),
            })
        }
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
    fetch_with(&repo, src, &spec, Some(branch), interrupt)?;
    read_file(&repo, &tracking, &src.path, max_file)
}

/// Fetches `spec` at depth 1 with the source's credentials. With
/// `branch`, a branch the remote does not advertise is `BranchMissing`.
fn fetch_with(
    repo: &gix::Repository,
    src: &Source,
    spec: &str,
    branch: Option<&str>,
    interrupt: &AtomicBool,
) -> std::result::Result<(), Fail> {
    let net = |e: gix::Error| Fail::Network(e.to_string());
    let remote = repo
        .remote_at(src.url.as_str())
        .map_err(net)?
        .with_refspecs([spec], gix::remote::Direction::Fetch)
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
    if let Some(branch) = branch {
        let wanted = format!("refs/heads/{branch}");
        let advertised = prepare
            .ref_map()
            .mappings
            .iter()
            .any(|m| m.remote.as_name().is_some_and(|n| n == wanted.as_bytes()));
        if !advertised {
            return Err(Fail::BranchMissing(branch.to_string()));
        }
    }
    prepare
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            std::num::NonZeroU32::MIN,
        ))
        .receive(gix::progress::Discard, interrupt)
        .map_err(auth_fail)?;
    Ok(())
}

/// Opens a cache `fetch_file` filled and reads the tree of `commit`.
fn commit_tree<'r>(
    repo: &'r gix::Repository,
    commit: &str,
) -> std::result::Result<gix::Tree<'r>, Fail> {
    let id = gix::ObjectId::from_hex(commit.as_bytes()).map_err(|e| read_fail(&e))?;
    repo.find_commit(id)
        .map_err(|e| read_fail(&e))?
        .tree()
        .map_err(|e| read_fail(&e))
}

/// The tree of directory `dir` (`""` = the root) in `tree`.
fn dir_tree<'r>(
    tree: gix::Tree<'r>,
    dir: &str,
    short: &str,
) -> std::result::Result<gix::Tree<'r>, Fail> {
    if dir.is_empty() {
        return Ok(tree);
    }
    let entry = tree
        .lookup_entry_by_path(dir)
        .map_err(|e| read_fail(&e))?
        .ok_or_else(|| Fail::Invalid(format!("{dir} not found at {short}")))?;
    if !entry.mode().is_tree() {
        return Err(Fail::Invalid(format!("{dir} is not a directory")));
    }
    entry
        .object()
        .map_err(|e| read_fail(&e))?
        .try_into_tree()
        .map_err(|e| read_fail(&e))
}

fn invalid(f: Fail) -> Error {
    match f {
        Fail::Invalid(m) => Error::Invalid(m),
        Fail::Network(m) => Error::Internal(m),
        _ => Error::Internal("reading the fetched commit failed".into()),
    }
}

/// Runs a blocking read of the cache under its lock.
async fn with_cache<T: Send + 'static>(
    cache: &Path,
    f: impl FnOnce(&gix::Repository) -> std::result::Result<T, Fail> + Send + 'static,
) -> Result<T> {
    let _guard = dir_lock(cache).lock_owned().await;
    let cache = cache.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let repo =
            gix::open_opts(&cache, gix::open::Options::isolated()).map_err(|e| read_fail(&e))?;
        f(&repo)
    })
    .await
    .map_err(|e| Error::Internal(format!("git read stopped: {e}")))?
    .map_err(invalid)
}

/// Git's content id of each directory of `dirs` (`""` = the root) at
/// `commit`: it changes exactly when something under the directory does.
pub async fn tree_ids(cache: &Path, commit: &str, dirs: &[String]) -> Result<Vec<String>> {
    let (commit, dirs) = (commit.to_string(), dirs.to_vec());
    with_cache(cache, move |repo| {
        let short = &commit[..commit.len().min(7)];
        dirs.iter()
            .map(|d| {
                Ok(dir_tree(commit_tree(repo, &commit)?, d, short)?
                    .id
                    .to_string())
            })
            .collect()
    })
    .await
}

/// Caps on a build context.
#[derive(Debug, Clone, Copy)]
pub struct TarLimits {
    pub max_bytes: usize,
    pub max_entries: usize,
}

/// A directory of a commit as a build context.
#[derive(Debug, Clone)]
pub struct DirTar {
    pub tar: Vec<u8>,
    /// Submodules, which are left out.
    pub skipped: Vec<String>,
}

/// A tar of directory `dir` at `commit`, with its `.dockerignore` applied:
/// paths relative to `dir`, in tree order, mtime 0, owner 0, mode 644 or
/// 755, symlinks kept. The same tree always gives the same bytes.
pub async fn tar_dir(cache: &Path, commit: &str, dir: &str, limits: TarLimits) -> Result<DirTar> {
    let tree = tree_ids(cache, commit, &[dir.to_string()]).await?.remove(0);
    let label = if dir.is_empty() { "." } else { dir };
    tar_tree(cache, &tree, label, &[], limits).await
}

fn find_tree<'r>(
    repo: &'r gix::Repository,
    tree: &str,
) -> std::result::Result<gix::Tree<'r>, Fail> {
    let id = gix::ObjectId::from_hex(tree.as_bytes()).map_err(|e| read_fail(&e))?;
    repo.find_object(id)
        .map_err(|e| read_fail(&e))?
        .try_into_tree()
        .map_err(|e| read_fail(&e))
}

/// Whether tree `tree` holds a file at `path`.
pub async fn tree_has_file(cache: &Path, tree: &str, path: &str) -> Result<bool> {
    let (tree, path) = (tree.to_string(), path.to_string());
    with_cache(cache, move |repo| {
        let t = find_tree(repo, &tree)?;
        Ok(t.lookup_entry_by_path(&path)
            .map_err(|e| read_fail(&e))?
            .is_some_and(|e| e.mode().is_blob() || e.mode().is_link()))
    })
    .await
}

/// A tar of tree `tree` (by its git id, so the content is exactly what was
/// hashed), as `tar_dir` makes it. `label` names it in errors. `keep` are
/// paths `.dockerignore` never leaves out: like the Docker CLI, the
/// Dockerfile and the ignore file itself are always sent.
pub async fn tar_tree(
    cache: &Path,
    tree: &str,
    label: &str,
    keep: &[String],
    limits: TarLimits,
) -> Result<DirTar> {
    let (tree_id, dir, keep) = (tree.to_string(), label.to_string(), keep.to_vec());
    with_cache(cache, move |repo| {
        let tree = find_tree(repo, &tree_id)?;
        let mut text = match tree
            .lookup_entry_by_path(".dockerignore")
            .map_err(|e| read_fail(&e))?
        {
            Some(e) if e.mode().is_blob() => {
                let blob = e.object().map_err(|e| read_fail(&e))?;
                String::from_utf8_lossy(&blob.data).into_owned()
            }
            _ => String::new(),
        };
        if !text.is_empty() {
            text.push_str("\n!.dockerignore\n");
            for k in &keep {
                text.push_str(&format!("!{k}\n"));
            }
        }
        let mut w = TarWriter {
            b: tar::Builder::new(Vec::new()),
            ignore: dockerignore::Matcher::new(&text),
            limits,
            dir,
            bytes: 0,
            entries: 0,
            skipped: vec![],
        };
        w.walk(&tree, "")?;
        let tar = w.b.into_inner().map_err(|e| read_fail(&e))?;
        Ok(DirTar {
            tar,
            skipped: w.skipped,
        })
    })
    .await
}

struct TarWriter {
    b: tar::Builder<Vec<u8>>,
    ignore: dockerignore::Matcher,
    limits: TarLimits,
    dir: String,
    bytes: usize,
    entries: usize,
    skipped: Vec<String>,
}

impl TarWriter {
    fn count(&mut self, size: usize) -> std::result::Result<(), Fail> {
        self.entries += 1;
        self.bytes += size;
        if self.entries > self.limits.max_entries {
            return Err(Fail::Invalid(format!(
                "the build context {} has more than {} entries",
                self.dir, self.limits.max_entries
            )));
        }
        if self.bytes > self.limits.max_bytes {
            return Err(Fail::Invalid(format!(
                "the build context {} is larger than {}",
                self.dir,
                human_size(self.limits.max_bytes as u64)
            )));
        }
        Ok(())
    }

    fn header(kind: tar::EntryType, mode: u32, size: u64) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_mode(mode);
        h.set_size(size);
        h.set_mtime(0);
        h.set_uid(0);
        h.set_gid(0);
        h
    }

    fn walk(&mut self, tree: &gix::Tree<'_>, prefix: &str) -> std::result::Result<(), Fail> {
        for entry in tree.iter() {
            let entry = entry.map_err(|e| read_fail(&e))?;
            let path = format!("{prefix}{}", entry.filename());
            let mode = entry.mode();
            let ignored = self.ignore.ignored(&path);
            if mode.is_tree() {
                if ignored && !self.ignore.has_exceptions() {
                    continue;
                }
                let sub = entry
                    .object()
                    .map_err(|e| read_fail(&e))?
                    .try_into_tree()
                    .map_err(|e| read_fail(&e))?;
                if !ignored {
                    self.count(0)?;
                    let mut h = Self::header(tar::EntryType::Directory, 0o755, 0);
                    self.b
                        .append_data(&mut h, format!("{path}/"), std::io::empty())
                        .map_err(|e| read_fail(&e))?;
                }
                self.walk(&sub, &format!("{path}/"))?;
            } else if mode.is_commit() {
                self.skipped.push(path);
            } else if !ignored {
                let blob = entry.object().map_err(|e| read_fail(&e))?;
                self.count(blob.data.len())?;
                if mode.is_link() {
                    let target = String::from_utf8_lossy(&blob.data).into_owned();
                    let mut h = Self::header(tar::EntryType::Symlink, 0o777, 0);
                    self.b
                        .append_link(&mut h, &path, target)
                        .map_err(|e| read_fail(&e))?;
                } else {
                    let exec = mode.is_executable();
                    let mut h = Self::header(
                        tar::EntryType::Regular,
                        if exec { 0o755 } else { 0o644 },
                        blob.data.len() as u64,
                    );
                    self.b
                        .append_data(&mut h, &path, blob.data.as_slice())
                        .map_err(|e| read_fail(&e))?;
                }
            }
        }
        Ok(())
    }
}

/// Whether the cache holds `commit`.
pub fn has_commit(cache: &Path, commit: &str) -> bool {
    let Ok(repo) = gix::open_opts(cache, gix::open::Options::isolated()) else {
        return false;
    };
    gix::ObjectId::from_hex(commit.as_bytes())
        .ok()
        .is_some_and(|id| repo.find_commit(id).is_ok())
}

/// Fetches one commit by id (depth 1) into the cache, for a rollback to a
/// commit the cache no longer holds. A git host that refuses to serve an
/// unadvertised commit gives `commit <short> cannot be fetched`.
pub async fn fetch_commit(cache: &Path, src: &Source, commit: &str, limits: Limits) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _guard = dir_lock(cache).lock_owned().await;
    let short = commit[..commit.len().min(7)].to_string();
    let interrupt = Arc::new(AtomicBool::new(false));
    let job = tokio::task::spawn_blocking({
        let (cache, src, commit, interrupt) = (
            cache.to_path_buf(),
            src.clone(),
            commit.to_string(),
            interrupt.clone(),
        );
        move || {
            let repo = open_cache(&cache, &src.url)?;
            let spec = format!("{commit}:refs/fleetops/commits/{commit}");
            fetch_with(&repo, &src, &spec, None, &interrupt)
        }
    });
    let joined = match tokio::time::timeout(limits.timeout, job).await {
        Ok(j) => j.map_err(|e| Error::Internal(format!("git fetch stopped: {e}")))?,
        Err(_) => {
            interrupt.store(true, Ordering::Relaxed);
            return Err(Error::Invalid(format!("commit {short} cannot be fetched")));
        }
    };
    match joined {
        Ok(()) => Ok(()),
        Err(Fail::NeedsToken) => Err(Error::Invalid(
            "the repository needs an access token".into(),
        )),
        Err(Fail::RefusedToken) => Err(Error::Invalid(
            "the repository refused the credentials".into(),
        )),
        Err(_) => Err(Error::Invalid(format!("commit {short} cannot be fetched"))),
    }
}

fn read_file(
    repo: &gix::Repository,
    tracking: &str,
    path: &str,
    max_file: u64,
) -> std::result::Result<Fetched, Fail> {
    let mut head = repo.find_reference(tracking).map_err(|e| read_fail(&e))?;
    let commit = head.peel_to_commit().map_err(|e| read_fail(&e))?;
    let id = commit.id().to_string();
    let tree = commit.tree().map_err(|e| read_fail(&e))?;
    let content = read_blob(repo, &tree, path, &id[..7], max_file)?;
    Ok(Fetched {
        commit: id,
        content,
    })
}

fn read_fail(e: &dyn std::fmt::Display) -> Fail {
    Fail::Network(format!("reading the fetched commit: {e}"))
}

/// The UTF-8 text of the file at `path` in `tree`.
fn read_blob(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &str,
    short: &str,
    max_file: u64,
) -> std::result::Result<String, Fail> {
    let internal = |e: &dyn std::fmt::Display| read_fail(e);
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
    String::from_utf8(blob.data.clone()).map_err(|_| Fail::Invalid(format!("{path} is not UTF-8")))
}

/// Reads `paths` at `commit` from a cache [`fetch_file`] just filled (a git
/// stack's config files, read at the Compose file's commit). Errors match
/// `fetch_file`'s: `<path> not found at <short>`, and so on.
pub async fn read_at(
    cache: &Path,
    commit: &str,
    paths: &[String],
    max_file: u64,
) -> Result<Vec<String>> {
    let _guard = dir_lock(cache).lock_owned().await;
    let (cache, commit, paths) = (cache.to_path_buf(), commit.to_string(), paths.to_vec());
    let read = tokio::task::spawn_blocking(move || -> std::result::Result<Vec<String>, Fail> {
        let repo = gix::open(&cache).map_err(|e| read_fail(&e))?;
        let id = gix::ObjectId::from_hex(commit.as_bytes()).map_err(|e| read_fail(&e))?;
        let tree = repo
            .find_commit(id)
            .map_err(|e| read_fail(&e))?
            .tree()
            .map_err(|e| read_fail(&e))?;
        let short = &commit[..commit.len().min(7)];
        paths
            .iter()
            .map(|p| read_blob(&repo, &tree, p, short, max_file))
            .collect()
    })
    .await
    .map_err(|e| Error::Internal(format!("git read stopped: {e}")))?;
    read.map_err(|f| match f {
        Fail::Invalid(m) => Error::Invalid(m),
        Fail::Network(m) => Error::Internal(m),
        _ => Error::Internal("reading the fetched commit failed".into()),
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
    fn scrub_hides_encoded_tokens() {
        use base64::Engine;
        let t = "tok/en+1=";
        let b64 = |v: &str| base64::engine::general_purpose::STANDARD.encode(v);
        let basic = b64(&format!("{TOKEN_USER}:{t}"));
        let m = format!(
            "a {t} b tok%2Fen%2B1%3D c tok%2fen%2b1%3d d {} e Basic {basic}",
            b64(t)
        );
        let out = scrub(m, Some(t));
        assert!(!out.contains(t), "{out}");
        assert!(!out.to_lowercase().contains("tok%2fen"), "{out}");
        assert!(!out.contains(&b64(t)), "{out}");
        assert!(!out.contains(&basic), "{out}");
        assert_eq!(scrub("plain".into(), None), "plain");
        assert_eq!(scrub("plain".into(), Some("")), "plain");
    }

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
