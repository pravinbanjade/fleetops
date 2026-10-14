//! Runs an exec plugin and keeps its token until shortly before it expires
//! (spec 3k §2).

use super::Plugin;
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// The most a plugin may print.
const STDOUT_CAP: usize = 1 << 20;
/// How much of a failing plugin's stderr is shown.
const STDERR_SHOWN: usize = 300;
/// How long a token without an expiry is kept.
const NO_EXPIRY_KEEP: Duration = Duration::from_secs(600);
/// A token is replaced this long before it expires.
const EARLY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    NotFound(String),
    Failed {
        command: String,
        code: Option<i32>,
        stderr: String,
    },
    TimedOut {
        command: String,
        secs: u64,
    },
    NotACredential(String),
    NoToken(String),
    Io(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::NotFound(c) => {
                write!(f, "exec plugin '{c}' was not found on the server's PATH")
            }
            ExecError::Failed {
                command,
                code,
                stderr,
            } => {
                let code = code.map_or("signal".to_string(), |c| c.to_string());
                write!(f, "exec plugin '{command}' failed (exit {code}): {stderr}")
            }
            ExecError::TimedOut { command, secs } => {
                write!(f, "exec plugin '{command}' did not finish within {secs}s")
            }
            ExecError::NotACredential(c) => {
                write!(
                    f,
                    "exec plugin '{c}' printed something that is not an ExecCredential"
                )
            }
            ExecError::NoToken(c) => write!(f, "exec plugin '{c}' printed no token"),
            ExecError::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ExecError {}

pub struct Credential {
    pub token: String,
    pub expires: Option<jiff::Timestamp>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"<redacted>")
            .field("expires", &self.expires)
            .finish()
    }
}

#[derive(serde::Deserialize)]
struct ExecCredential {
    status: Option<Status>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    token: Option<String>,
    expiration_timestamp: Option<String>,
}

/// An `ExecCredential` (JSON, or YAML as client-go also accepts).
pub fn parse_credential(command: &str, out: &[u8]) -> Result<Credential, ExecError> {
    let c: ExecCredential =
        serde_saphyr::from_slice(out).map_err(|_| ExecError::NotACredential(command.into()))?;
    let s = c.status.ok_or_else(|| ExecError::NoToken(command.into()))?;
    let token = s
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ExecError::NoToken(command.into()))?;
    let expires = match s.expiration_timestamp {
        Some(t) => Some(
            t.parse()
                .map_err(|_| ExecError::NotACredential(command.into()))?,
        ),
        None => None,
    };
    Ok(Credential { token, expires })
}

#[async_trait]
pub trait Runner: Send + Sync {
    /// The plugin's stdout, or why it gave none.
    async fn run(&self, p: &Plugin) -> Result<Vec<u8>, ExecError>;
}

/// Runs the plugin as a child: no stdin, killed when the wait is abandoned.
pub struct ProcessRunner {
    pub timeout: Duration,
}

#[async_trait]
impl Runner for ProcessRunner {
    async fn run(&self, p: &Plugin) -> Result<Vec<u8>, ExecError> {
        let mut cmd = tokio::process::Command::new(&p.command);
        // Its own process group, so a timeout ends whatever it started too.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .args(&p.args)
            .envs(p.env.iter().map(|(k, v)| (k, v)))
            .env("KUBERNETES_EXEC_INFO", &p.exec_info)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ExecError::NotFound(p.command.clone()),
                _ => ExecError::Io(format!("exec plugin '{}' could not start: {e}", p.command)),
            })?;
        let group = child.id();
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let work = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut out_cap = (&mut stdout).take(STDOUT_CAP as u64 + 1);
            let mut err_cap = (&mut stderr).take(64 * 1024);
            // Over the cap, stop at once: a plugin blocked on a full stdout
            // never closes stderr.
            let read_out = async {
                out_cap
                    .read_to_end(&mut out)
                    .await
                    .map_err(|e| ExecError::Io(e.to_string()))?;
                if out.len() > STDOUT_CAP {
                    return Err(ExecError::NotACredential(p.command.clone()));
                }
                Ok(())
            };
            let read_err = async {
                err_cap
                    .read_to_end(&mut err)
                    .await
                    .map_err(|e| ExecError::Io(e.to_string()))
            };
            tokio::try_join!(read_out, read_err)?;
            let status = child
                .wait()
                .await
                .map_err(|e| ExecError::Io(e.to_string()))?;
            if !status.success() {
                let text = String::from_utf8_lossy(&err);
                let shown: String = text.trim().chars().take(STDERR_SHOWN).collect();
                return Err(ExecError::Failed {
                    command: p.command.clone(),
                    code: status.code(),
                    stderr: shown,
                });
            }
            Ok(out)
        };
        match tokio::time::timeout(self.timeout, work).await {
            Ok(r) => r,
            // Dropping `child` (kill_on_drop) ends the plugin; the group
            // kill ends what it started.
            Err(_) => {
                kill_group(group);
                Err(ExecError::TimedOut {
                    command: p.command.clone(),
                    secs: self.timeout.as_secs(),
                })
            }
        }
    }
}

/// SIGKILL to the plugin's process group (its pid, from `process_group(0)`).
fn kill_group(group: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = group.and_then(|g| i32::try_from(g).ok()) {
        // SAFETY: kill(2) with a negative pid signals that process group;
        // it touches no memory of ours.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = group;
}

/// One plugin's token, refreshed by at most one run at a time.
pub struct TokenSource {
    plugin: Plugin,
    runner: Arc<dyn Runner>,
    cache: Mutex<Option<(String, Instant)>>,
}

impl TokenSource {
    pub fn new(plugin: Plugin, runner: Arc<dyn Runner>) -> Self {
        Self {
            plugin,
            runner,
            cache: Mutex::new(None),
        }
    }

    pub fn command(&self) -> &str {
        &self.plugin.command
    }

    pub async fn token(&self) -> Result<String, ExecError> {
        let mut cache = self.cache.lock().await;
        if let Some((t, until)) = cache.as_ref() {
            if Instant::now() < *until {
                return Ok(t.clone());
            }
        }
        let out = self.runner.run(&self.plugin).await?;
        let c = parse_credential(&self.plugin.command, &out)?;
        let keep = match c.expires {
            Some(at) => {
                let left = at.duration_since(jiff::Timestamp::now());
                Duration::try_from(left)
                    .unwrap_or_default()
                    .saturating_sub(EARLY)
            }
            None => NO_EXPIRY_KEEP,
        };
        *cache = Some((c.token.clone(), Instant::now() + keep));
        Ok(c.token)
    }

    /// Forgets the token: the cluster refused it.
    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn plugin(command: &str) -> Plugin {
        Plugin {
            command: command.into(),
            args: vec![],
            env: vec![],
            exec_info: "{}".into(),
        }
    }

    struct Fake {
        runs: AtomicUsize,
        out: std::sync::Mutex<Result<Vec<u8>, ExecError>>,
        delay: Duration,
    }
    impl Fake {
        fn new(out: Result<&str, ExecError>) -> Arc<Self> {
            Arc::new(Self {
                runs: AtomicUsize::new(0),
                out: std::sync::Mutex::new(out.map(|s| s.as_bytes().to_vec())),
                delay: Duration::ZERO,
            })
        }
        fn runs(&self) -> usize {
            self.runs.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl Runner for Fake {
        async fn run(&self, _: &Plugin) -> Result<Vec<u8>, ExecError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.out.lock().unwrap().clone()
        }
    }

    fn cred(token: &str, expires: Option<&str>) -> String {
        let exp = expires
            .map(|e| format!(",\"expirationTimestamp\":\"{e}\""))
            .unwrap_or_default();
        format!("{{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{{\"token\":\"{token}\"{exp}}}}}")
    }

    #[test]
    fn json_and_yaml_credentials_parse() {
        let c =
            parse_credential("aws", cred("t1", Some("2030-01-01T00:00:00Z")).as_bytes()).unwrap();
        assert_eq!(c.token, "t1");
        assert_eq!(c.expires.unwrap().to_string(), "2030-01-01T00:00:00Z");
        let y = "apiVersion: client.authentication.k8s.io/v1\nkind: ExecCredential\nstatus:\n  token: t2\n";
        assert_eq!(parse_credential("aws", y.as_bytes()).unwrap().token, "t2");
        assert_eq!(
            parse_credential("aws", b"not: [yaml").unwrap_err(),
            ExecError::NotACredential("aws".into())
        );
        let no_token = "{\"kind\":\"ExecCredential\",\"status\":{\"clientCertificateData\":\"x\"}}";
        assert_eq!(
            parse_credential("aws", no_token.as_bytes()).unwrap_err(),
            ExecError::NoToken("aws".into())
        );
    }

    #[test]
    fn errors_read_as_the_spec_says() {
        assert_eq!(
            ExecError::NotFound("aws".into()).to_string(),
            "exec plugin 'aws' was not found on the server's PATH"
        );
        assert_eq!(
            ExecError::Failed {
                command: "aws".into(),
                code: Some(255),
                stderr: "boom".into()
            }
            .to_string(),
            "exec plugin 'aws' failed (exit 255): boom"
        );
        assert_eq!(
            ExecError::TimedOut {
                command: "aws".into(),
                secs: 30
            }
            .to_string(),
            "exec plugin 'aws' did not finish within 30s"
        );
        assert_eq!(
            ExecError::NoToken("aws".into()).to_string(),
            "exec plugin 'aws' printed no token"
        );
        assert_eq!(
            ExecError::NotACredential("aws".into()).to_string(),
            "exec plugin 'aws' printed something that is not an ExecCredential"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_is_kept_until_a_minute_before_it_expires() {
        let soon = (jiff::Timestamp::now() + jiff::SignedDuration::from_secs(300)).to_string();
        let f = Fake::new(Ok(&cred("t", Some(&soon))));
        let s = TokenSource::new(plugin("aws"), f.clone());
        assert_eq!(s.token().await.unwrap(), "t");
        tokio::time::advance(Duration::from_secs(200)).await;
        s.token().await.unwrap();
        assert_eq!(f.runs(), 1);
        tokio::time::advance(Duration::from_secs(50)).await;
        s.token().await.unwrap();
        assert_eq!(f.runs(), 2, "inside the last minute it runs again");
    }

    #[tokio::test(start_paused = true)]
    async fn without_an_expiry_a_token_is_kept_ten_minutes() {
        let f = Fake::new(Ok(&cred("t", None)));
        let s = TokenSource::new(plugin("aws"), f.clone());
        s.token().await.unwrap();
        tokio::time::advance(Duration::from_secs(599)).await;
        s.token().await.unwrap();
        assert_eq!(f.runs(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        s.token().await.unwrap();
        assert_eq!(f.runs(), 2);
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_run() {
        let f = Arc::new(Fake {
            runs: AtomicUsize::new(0),
            out: std::sync::Mutex::new(Ok(cred("t", None).into_bytes())),
            delay: Duration::from_millis(50),
        });
        let s = Arc::new(TokenSource::new(plugin("aws"), f.clone()));
        let all = futures_util::future::join_all((0..20).map(|_| {
            let s = s.clone();
            async move { s.token().await }
        }))
        .await;
        assert!(all.iter().all(|t| t.as_deref() == Ok("t")));
        assert_eq!(f.runs(), 1);
    }

    #[tokio::test]
    async fn a_failure_is_not_cached_and_invalidate_forces_a_run() {
        let f = Fake::new(Err(ExecError::NotFound("aws".into())));
        let s = TokenSource::new(plugin("aws"), f.clone());
        assert!(s.token().await.is_err());
        assert!(s.token().await.is_err());
        assert_eq!(f.runs(), 2);
        *f.out.lock().unwrap() = Ok(cred("t", None).into_bytes());
        s.token().await.unwrap();
        s.invalidate().await;
        s.token().await.unwrap();
        assert_eq!(f.runs(), 4);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_process_runner_passes_args_env_and_exec_info_without_stdin() {
        let p = Plugin {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "read x || echo \"no-stdin $A $KUBERNETES_EXEC_INFO\"".into(),
            ],
            env: vec![("A".into(), "a".into())],
            exec_info: "{\"k\":1}".into(),
        };
        let out = ProcessRunner {
            timeout: Duration::from_secs(5),
        }
        .run(&p)
        .await
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap().trim(),
            "no-stdin a {\"k\":1}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_command_a_failing_one_and_a_long_stdout() {
        let r = ProcessRunner {
            timeout: Duration::from_secs(5),
        };
        assert_eq!(
            r.run(&plugin("fleetops-no-such-plugin")).await.unwrap_err(),
            ExecError::NotFound("fleetops-no-such-plugin".into())
        );
        let fail = Plugin {
            args: vec!["-c".into(), "echo nope >&2; exit 3".into()],
            ..plugin("sh")
        };
        assert_eq!(
            r.run(&fail).await.unwrap_err(),
            ExecError::Failed {
                command: "sh".into(),
                code: Some(3),
                stderr: "nope".into()
            }
        );
        let big = Plugin {
            args: vec!["-c".into(), "head -c 2000000 /dev/zero".into()],
            ..plugin("sh")
        };
        assert_eq!(
            r.run(&big).await.unwrap_err(),
            ExecError::NotACredential("sh".into())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hung_plugin_times_out_and_is_killed() {
        let dir = std::env::temp_dir().join(format!("fleetops-exec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("still-running");
        let p = Plugin {
            args: vec![
                "-c".into(),
                format!("(sleep 2; touch {}) & wait", mark.display()),
            ],
            ..plugin("sh")
        };
        let started = std::time::Instant::now();
        let e = ProcessRunner {
            timeout: Duration::from_millis(300),
        }
        .run(&p)
        .await
        .unwrap_err();
        assert_eq!(
            e,
            ExecError::TimedOut {
                command: "sh".into(),
                secs: 0
            }
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !mark.exists(),
            "a process the plugin started was left running"
        );
    }
}
