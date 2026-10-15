//! Running a helper program (slices 3k, 3j): no stdin, its own process
//! group killed on timeout, stdout capped and stderr kept for messages.

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// How much stderr is kept.
const STDERR_KEPT: u64 = 64 * 1024;

pub struct Spec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub env: &'a [(String, String)],
    pub timeout: Duration,
    pub stdout_cap: usize,
    /// Where the child runs; `None` is the server's own directory.
    pub cwd: Option<&'a std::path::Path>,
}

#[derive(Debug)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    NotFound,
    TimedOut,
    TooLarge,
    Io(String),
}

pub async fn run(s: Spec<'_>) -> Result<Output, RunError> {
    let mut cmd = tokio::process::Command::new(s.program);
    if let Some(dir) = s.cwd {
        cmd.current_dir(dir);
    }
    // Its own process group, so a timeout ends whatever it started too.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd
        .args(s.args)
        .envs(s.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound,
            _ => RunError::Io(e.to_string()),
        })?;
    let group = child.id();
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let cap = s.stdout_cap;
    let work = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_cap = (&mut stdout).take(cap as u64 + 1);
        let mut err_cap = (&mut stderr).take(STDERR_KEPT);
        // Over the cap, stop at once: a child blocked on a full stdout
        // never closes stderr.
        let read_out = async {
            out_cap
                .read_to_end(&mut out)
                .await
                .map_err(|e| RunError::Io(e.to_string()))?;
            if out.len() > cap {
                return Err(RunError::TooLarge);
            }
            Ok(())
        };
        let read_err = async {
            err_cap
                .read_to_end(&mut err)
                .await
                .map_err(|e| RunError::Io(e.to_string()))
        };
        tokio::try_join!(read_out, read_err)?;
        let status = child
            .wait()
            .await
            .map_err(|e| RunError::Io(e.to_string()))?;
        Ok(Output {
            stdout: out,
            stderr: String::from_utf8_lossy(&err).into_owned(),
            code: status.code(),
        })
    };
    match tokio::time::timeout(s.timeout, work).await {
        Ok(r) => {
            if r.is_err() {
                kill_group(group);
            }
            r
        }
        Err(_) => {
            kill_group(group);
            Err(RunError::TimedOut)
        }
    }
}

/// SIGKILL to the child's process group (its pid, from `process_group(0)`).
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec<'a>(args: &'a [String], env: &'a [(String, String)]) -> Spec<'a> {
        Spec {
            program: "sh",
            args,
            env,
            timeout: Duration::from_secs(5),
            stdout_cap: 1024,
            cwd: None,
        }
    }
    fn sh(script: &str) -> Vec<String> {
        vec!["-c".into(), script.into()]
    }

    #[tokio::test]
    async fn output_code_and_stderr_come_back_and_stdin_is_empty() {
        let args = sh("read x || echo \"out $A\"; echo err >&2; exit 4");
        let env = [("A".to_string(), "a".to_string())];
        let o = run(spec(&args, &env)).await.unwrap();
        assert_eq!(String::from_utf8(o.stdout.clone()).unwrap(), "out a\n");
        assert_eq!(o.stderr.trim(), "err");
        assert_eq!(o.code, Some(4));
        assert!(!o.success());
    }

    #[tokio::test]
    async fn missing_too_large_and_timed_out() {
        let none: [(String, String); 0] = [];
        let args: Vec<String> = vec![];
        let missing = Spec {
            program: "fleetops-no-such-program",
            ..spec(&args, &none)
        };
        assert_eq!(run(missing).await.unwrap_err(), RunError::NotFound);
        let big = sh("head -c 5000 /dev/zero");
        assert_eq!(
            run(spec(&big, &none)).await.unwrap_err(),
            RunError::TooLarge
        );
        let slow = sh("sleep 5");
        let s = Spec {
            timeout: Duration::from_millis(200),
            ..spec(&slow, &none)
        };
        assert_eq!(run(s).await.unwrap_err(), RunError::TimedOut);
    }

    #[tokio::test]
    async fn the_child_runs_in_the_directory_given() {
        let dir = tempfile::tempdir().unwrap();
        let args = sh("pwd -P");
        let none: [(String, String); 0] = [];
        let s = Spec {
            cwd: Some(dir.path()),
            ..spec(&args, &none)
        };
        let o = run(s).await.unwrap();
        let want = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            String::from_utf8(o.stdout.clone()).unwrap().trim(),
            want.to_string_lossy()
        );
    }
}
