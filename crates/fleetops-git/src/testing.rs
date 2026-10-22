//! Test helpers: fixture repositories built with the `git` CLI, and an HTTP
//! server that serves them through `git http-backend`.

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use base64::Engine;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("the git CLI is needed for git tests");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A bare repository `repo.git` under [`TestRepo::root`], fed by a work tree.
pub struct TestRepo {
    dir: tempfile::TempDir,
    branch: String,
}

impl TestRepo {
    pub fn new(branch: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("srv")).unwrap();
        git(
            &root.join("srv"),
            &["init", "--bare", "-q", "-b", branch, "repo.git"],
        );
        git(root, &["init", "-q", "-b", branch, "work"]);
        git(
            &root.join("work"),
            &[
                "remote",
                "add",
                "origin",
                root.join("srv/repo.git").to_str().unwrap(),
            ],
        );
        Self {
            dir,
            branch: branch.into(),
        }
    }

    /// The directory to serve: it holds `repo.git`.
    pub fn root(&self) -> PathBuf {
        self.dir.path().join("srv")
    }

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn write(&self, files: &[(&str, &str)]) {
        for (path, content) in files {
            let p = self.work().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }

    /// Commits `files` on the branch and pushes; returns the commit id.
    pub fn commit(&self, files: &[(&str, &str)], msg: &str) -> String {
        self.write(files);
        let w = self.work();
        git(&w, &["add", "-A"]);
        git(&w, &["commit", "-q", "--allow-empty", "-m", msg]);
        git(
            &w,
            &[
                "push",
                "-q",
                "-f",
                "origin",
                &format!("HEAD:{}", self.branch),
            ],
        );
        git(&w, &["rev-parse", "HEAD"])
    }

    /// Marks `path` (already written) executable for the next commit.
    pub fn set_executable(&self, path: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = self.work().join(path);
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Adds a symlink `path` → `target` for the next commit.
    pub fn add_symlink(&self, path: &str, target: &str) {
        let p = self.work().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, p).unwrap();
    }

    /// Commits `files` on another branch, leaving the main one alone.
    pub fn commit_on(&self, branch: &str, files: &[(&str, &str)], msg: &str) -> String {
        let w = self.work();
        git(&w, &["checkout", "-q", "-B", branch]);
        let id = {
            self.write(files);
            git(&w, &["add", "-A"]);
            git(&w, &["commit", "-q", "--allow-empty", "-m", msg]);
            git(
                &w,
                &["push", "-q", "-f", "origin", &format!("HEAD:{branch}")],
            );
            git(&w, &["rev-parse", "HEAD"])
        };
        git(&w, &["checkout", "-q", &self.branch]);
        id
    }

    /// Replaces the branch's history with one new commit holding only
    /// `files` (a force-push).
    pub fn force_reset(&self, files: &[(&str, &str)], msg: &str) -> String {
        let w = self.work();
        git(&w, &["checkout", "-q", "--orphan", "fleetops-reset"]);
        git(&w, &["rm", "-rfq", "--ignore-unmatch", "."]);
        self.write(files);
        git(&w, &["add", "-A"]);
        git(&w, &["commit", "-q", "-m", msg]);
        git(&w, &["branch", "-q", "-D", &self.branch]);
        git(&w, &["branch", "-q", "-m", &self.branch]);
        git(
            &w,
            &[
                "push",
                "-q",
                "-f",
                "origin",
                &format!("HEAD:{}", self.branch),
            ],
        );
        git(&w, &["rev-parse", "HEAD"])
    }
}

#[derive(Default, Clone)]
pub struct ServerOpts {
    /// Requires basic auth `x-access-token:<token>`.
    pub token: Option<String>,
    /// Added to every request.
    pub delay: Duration,
    /// Each fetch takes a permit before it is served.
    pub hold: Option<Arc<tokio::sync::Semaphore>>,
    /// Speaks protocol v0 only, where a commit that no ref points at
    /// cannot be fetched by id.
    pub v0_only: bool,
}

struct Shared {
    root: PathBuf,
    opts: ServerOpts,
    fetches: Arc<AtomicUsize>,
    in_flight: AtomicUsize,
    in_flight_max: Arc<AtomicUsize>,
}

pub struct GitServer {
    pub addr: SocketAddr,
    /// Fetches started (ref advertisements served or held).
    pub fetches: Arc<AtomicUsize>,
    /// The most requests served at once.
    pub in_flight_max: Arc<AtomicUsize>,
}

impl GitServer {
    pub async fn start(root: &Path, opts: ServerOpts) -> Self {
        let fetches = Arc::new(AtomicUsize::new(0));
        let in_flight_max = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            root: root.to_path_buf(),
            opts,
            fetches: fetches.clone(),
            in_flight: AtomicUsize::new(0),
            in_flight_max: in_flight_max.clone(),
        });
        let app = axum::Router::new().fallback(serve).with_state(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            fetches,
            in_flight_max,
        }
    }

    pub fn url(&self, repo: &str) -> String {
        format!("http://{}/{repo}", self.addr)
    }
}

struct InFlight<'a>(&'a Shared);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn serve(State(st): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let query = parts.uri.query().unwrap_or("").to_string();
    if let Some(token) = &st.opts.token {
        let want = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{token}", crate::TOKEN_USER))
        );
        let sent = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if sent != Some(want.as_str()) {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", "Basic realm=\"git\"")
                .body(Body::empty())
                .unwrap();
        }
    }
    let advert = parts.uri.path().ends_with("/info/refs");
    if advert {
        st.fetches.fetch_add(1, Ordering::SeqCst);
        if let Some(h) = &st.opts.hold {
            let _permit = h.acquire().await.unwrap();
        }
    }
    let now = st.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    st.in_flight_max.fetch_max(now, Ordering::SeqCst);
    let _guard = InFlight(&st);
    if !st.opts.delay.is_zero() {
        tokio::time::sleep(st.opts.delay).await;
    }
    let body = axum::body::to_bytes(body, 64 * 1024 * 1024).await.unwrap();
    cgi(
        st.opts.v0_only,
        &st.root,
        parts.method.as_ref(),
        parts.uri.path(),
        &query,
        &parts.headers,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn cgi(
    v0_only: bool,
    root: &Path,
    method: &str,
    path: &str,
    query: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let header = |n: &str| {
        headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let mut child = tokio::process::Command::new("git")
        .arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("REQUEST_METHOD", method)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", header("content-type"))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env(
            "GIT_PROTOCOL",
            if v0_only {
                String::new()
            } else {
                header("git-protocol")
            },
        )
        .env("HTTP_CONTENT_ENCODING", header("content-encoding"))
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("git http-backend");
    let mut stdin = child.stdin.take().unwrap();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&body).await;
    });
    let out = child.wait_with_output().await.unwrap();
    let _ = writer.await;
    let raw = out.stdout;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)))
        .expect("CGI headers");
    let (head, rest) = (&raw[..split.0], &raw[split.0 + split.1..]);
    let mut resp = Response::builder().status(StatusCode::OK);
    for line in String::from_utf8_lossy(head).lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        if k.eq_ignore_ascii_case("status") {
            let code = v.split(' ').next().unwrap_or("200").parse().unwrap_or(200);
            resp = resp.status(code);
        } else if let Ok(v) = HeaderValue::from_str(v) {
            resp = resp.header(k, v);
        }
    }
    resp.body(Body::from(rest.to_vec())).unwrap()
}

/// How [`SshServer`] behaves.
#[derive(Clone, Default)]
pub struct SshOpts {
    /// Public key lines (`ssh-ed25519 AAAA… comment`) allowed in.
    pub authorized: Vec<String>,
    /// Passes the client's `GIT_PROTOCOL` on, so it speaks protocol v2;
    /// otherwise v0, as OpenSSH without `AcceptEnv`.
    pub accept_env: bool,
    /// Each `git-upload-pack` takes a permit before it starts, and keeps it
    /// unless its client has disconnected by then.
    pub hold: Option<Arc<tokio::sync::Semaphore>>,
}

/// An SSH server that runs `git upload-pack` on repositories under a root,
/// with a fixed host key per server.
pub struct SshServer {
    pub addr: SocketAddr,
    /// `<algorithm> SHA256:…`, as [`crate::host_key_text`] gives it.
    pub host_key: String,
    /// SSH sessions open now.
    pub sessions: Arc<AtomicUsize>,
}

impl SshServer {
    pub async fn start(root: &Path, opts: SshOpts) -> Self {
        use russh::server::Server as _;
        let key =
            russh::keys::PrivateKey::from_openssh(crate::generate_key("host").private).unwrap();
        let host_key = crate::host_key_text(key.public_key());
        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut server = SshShared {
            sessions: sessions.clone(),
            root: root.to_path_buf(),
            opts: Arc::new(opts),
            channels: Default::default(),
            protocol: None,
            session: None,
        };
        tokio::spawn(async move {
            let _ = server.run_on_socket(config, &listener).await;
        });
        Self {
            addr,
            host_key,
            sessions,
        }
    }

    pub fn url(&self, repo: &str) -> String {
        format!("ssh://git@127.0.0.1:{}/{repo}", self.addr.port())
    }
}

type SshChannels = Arc<
    std::sync::Mutex<
        std::collections::HashMap<russh::ChannelId, russh::Channel<russh::server::Msg>>,
    >,
>;

#[derive(Clone)]
struct SshShared {
    root: PathBuf,
    opts: Arc<SshOpts>,
    channels: SshChannels,
    /// The client's `GIT_PROTOCOL`, when passed on.
    protocol: Option<String>,
    sessions: Arc<AtomicUsize>,
    /// Dropped with the client's session, which tells held fetches it went.
    session: Option<Arc<SessionGuard>>,
}

struct SessionGuard {
    gone: tokio::sync::watch::Sender<()>,
    sessions: Arc<AtomicUsize>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

impl russh::server::Server for SshShared {
    type Handler = Self;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Self {
        Self {
            channels: Default::default(),
            protocol: None,
            session: Some(Arc::new(SessionGuard {
                gone: tokio::sync::watch::channel(()).0,
                sessions: {
                    self.sessions.fetch_add(1, Ordering::SeqCst);
                    self.sessions.clone()
                },
            })),
            ..self.clone()
        }
    }
}

impl russh::server::Handler for SshShared {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<russh::server::Auth, Self::Error> {
        let line = key.to_openssh().unwrap();
        let sent = line.split(' ').take(2).collect::<Vec<_>>();
        let ok = self
            .opts
            .authorized
            .iter()
            .any(|a| a.split(' ').take(2).collect::<Vec<_>>() == sent);
        Ok(if ok {
            russh::server::Auth::Accept
        } else {
            russh::server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().unwrap().insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: russh::ChannelId,
        name: &str,
        value: &str,
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if self.opts.accept_env && name == "GIT_PROTOCOL" {
            self.protocol = Some(value.to_string());
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: russh::ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        let path = command
            .strip_prefix("git-upload-pack '")
            .and_then(|p| p.strip_suffix('\''))
            .map(|p| p.replace(r"'\''", "'"));
        let channel = self.channels.lock().unwrap().remove(&id);
        let (Some(path), Some(channel)) = (path, channel) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        session.channel_success(id)?;
        let handle = session.handle();
        let dir = self.root.join(path.trim_start_matches('/'));
        let (hold, protocol) = (self.opts.hold.clone(), self.protocol.clone());
        let mut gone = self
            .session
            .as_ref()
            .expect("a client's handler")
            .gone
            .subscribe();
        tokio::spawn(async move {
            if let Some(h) = &hold {
                // A client that goes while held gives its permit up; one
                // still there keeps it for good.
                let permit = tokio::select! {
                    p = h.acquire() => p.unwrap(),
                    _ = gone.changed() => return,
                };
                permit.forget();
            }
            let mut cmd = tokio::process::Command::new("git");
            cmd.arg("upload-pack")
                .arg(&dir)
                .env_remove("GIT_PROTOCOL")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            if let Some(p) = protocol {
                cmd.env("GIT_PROTOCOL", p);
            }
            let mut child = cmd.spawn().expect("git upload-pack");
            let (mut stdin, mut stdout) =
                (child.stdin.take().unwrap(), child.stdout.take().unwrap());
            let (mut rx, mut tx) = tokio::io::split(channel.into_stream());
            let input = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut rx, &mut stdin).await;
            });
            let _ = tokio::io::copy(&mut stdout, &mut tx).await;
            let _ = tx.flush().await;
            let code = child
                .wait()
                .await
                .map(|s| s.code().unwrap_or(1))
                .unwrap_or(1);
            input.abort();
            let _ = handle.exit_status_request(id, code as u32).await;
            let _ = handle.eof(id).await;
            let _ = handle.close(id).await;
        });
        Ok(())
    }
}
