//! Which exec plugins a kubeconfig may name, and with what (spec 3k §1).

use fleetops_core::{Error, Result};
use kube::config::{Cluster, ExecConfig, ExecInteractiveMode};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::time::Duration;

/// The plugins `default` turns on.
pub const BUILT_IN: [&str; 4] = [
    "aws",
    "aws-iam-authenticator",
    "gke-gcloud-auth-plugin",
    "kubelogin",
];
/// How long a plugin may take.
pub const PLUGIN_TIMEOUT: Duration = Duration::from_secs(30);
pub const OFF: &str =
    "exec plugins are off on this server; start it with --kube-exec-plugins=default";
/// kubelogin's login modes that need no person.
const KUBELOGIN_LOGINS: [&str; 5] = ["azurecli", "spn", "msi", "workloadidentity", "azd"];

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// Which exec plugins this server runs (spec 3k §1); off when empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecPolicy {
    allowed: BTreeSet<String>,
    timeout: Option<Duration>,
}

impl ExecPolicy {
    /// The flag's value: `default`, extra bare names, comma-separated.
    pub fn parse(v: &str) -> Result<Self> {
        let mut allowed = BTreeSet::new();
        for n in v.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            if n == "default" {
                allowed.extend(BUILT_IN.iter().map(|s| s.to_string()));
            } else if n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                allowed.insert(n.to_string());
            } else {
                return Err(invalid(format!(
                    "'{n}' is not a plugin name: give bare command names"
                )));
            }
        }
        Ok(Self {
            allowed,
            timeout: None,
        })
    }

    /// A shorter limit, for tests.
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout.unwrap_or(PLUGIN_TIMEOUT)
    }

    pub fn is_on(&self) -> bool {
        !self.allowed.is_empty()
    }

    /// The plugin `exec` would run, or why it may not.
    pub fn check(&self, exec: &ExecConfig, cluster: &Cluster) -> Result<Plugin> {
        if !self.is_on() {
            return Err(invalid(OFF));
        }
        let cmd = exec
            .command
            .as_deref()
            .filter(|c| !c.is_empty())
            .ok_or_else(|| invalid("the exec user has no command"))?;
        if cmd.contains('/') || cmd.contains('\\') {
            return Err(invalid(
                "exec plugin command must be a bare name on the server's PATH",
            ));
        }
        if !self.allowed.contains(cmd) {
            return Err(invalid(format!(
                "exec plugin '{cmd}' is not enabled on this server (--kube-exec-plugins)"
            )));
        }
        let args = exec.args.clone().unwrap_or_default();
        check_args(cmd, &args)?;
        let mut env = Vec::new();
        for e in exec.env.iter().flatten() {
            let (Some(name), Some(value)) = (e.get("name"), e.get("value")) else {
                return Err(invalid("each exec env entry needs a name and a value"));
            };
            check_env_name(name)?;
            env.push((name.clone(), value.clone()));
        }
        if exec.interactive_mode == Some(ExecInteractiveMode::Always) {
            return Err(invalid(format!(
                "exec plugin '{cmd}' needs a person at a terminal (interactiveMode: Always)"
            )));
        }
        let api = exec
            .api_version
            .clone()
            .unwrap_or_else(|| "client.authentication.k8s.io/v1beta1".into());
        let mut spec = json!({"interactive": false});
        if exec.provide_cluster_info {
            spec["cluster"] = cluster_info(cluster);
        }
        let exec_info =
            json!({"apiVersion": api, "kind": "ExecCredential", "spec": spec}).to_string();
        Ok(Plugin {
            command: cmd.to_string(),
            args,
            env,
            exec_info,
        })
    }
}

fn check_args(cmd: &str, args: &[String]) -> Result<()> {
    let starts =
        |want: &[&str]| args.len() >= want.len() && args.iter().zip(want).all(|(a, w)| a == w);
    let refuse = |why: &str| Err(invalid(format!("exec plugin '{cmd}' may only {why}")));
    match cmd {
        "aws" if !starts(&["eks", "get-token"]) => refuse("run 'eks get-token'"),
        "aws-iam-authenticator" if !starts(&["token"]) => refuse("run 'token'"),
        "kubelogin" if !starts(&["get-token"]) => refuse("run 'get-token'"),
        "kubelogin" => match login_mode(args) {
            Some(m) if KUBELOGIN_LOGINS.contains(&m) => Ok(()),
            _ => refuse("log in without a person: kubelogin needs --login azurecli, spn, msi, workloadidentity or azd"),
        },
        _ => Ok(()),
    }
}

/// kubelogin's `--login`/`-l` value, in either `--login x` or `--login=x` form.
fn login_mode(args: &[String]) -> Option<&str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        for flag in ["--login", "-l"] {
            if a == flag {
                return it.next().map(String::as_str);
            }
            if let Some(v) = a.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
                return Some(v);
            }
        }
    }
    None
}

/// Names that make an interpreter load code, or a cloud CLI read its
/// config (which can name commands to run), from a path the kubeconfig picks.
const RESERVED_ENV: [&str; 23] = [
    "PATH",
    "KUBERNETES_EXEC_INFO",
    "HOME",
    "XDG_CONFIG_HOME",
    "BASH_ENV",
    "ENV",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PERL5LIB",
    "PERL5OPT",
    "PERLLIB",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "GCONV_PATH",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "CLOUDSDK_CONFIG",
    "AZURE_CONFIG_DIR",
    "BOTO_CONFIG",
    "KUBECONFIG",
];
const RESERVED_ENV_PREFIXES: [&str; 3] = ["LD_", "DYLD_", "PYTHON"];

fn check_env_name(name: &str) -> Result<()> {
    let shaped = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let reserved =
        RESERVED_ENV.contains(&name) || RESERVED_ENV_PREFIXES.iter().any(|p| name.starts_with(p));
    if shaped && !reserved {
        Ok(())
    } else {
        Err(invalid(format!("exec env '{name}' is not allowed")))
    }
}

/// `spec.cluster` of `KUBERNETES_EXEC_INFO`, in client-go's field names.
fn cluster_info(c: &Cluster) -> Value {
    let mut m = Map::new();
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            m.insert(k.into(), v);
        }
    };
    put("server", c.server.clone().map(Value::from));
    put(
        "certificate-authority-data",
        c.certificate_authority_data.clone().map(Value::from),
    );
    put(
        "tls-server-name",
        c.tls_server_name.clone().map(Value::from),
    );
    put("proxy-url", c.proxy_url.clone().map(Value::from));
    put(
        "insecure-skip-tls-verify",
        c.insecure_skip_tls_verify.filter(|b| *b).map(Value::from),
    );
    put(
        "config",
        c.extensions
            .iter()
            .flatten()
            .find(|e| e.name == "client.authentication.k8s.io/exec")
            .map(|e| e.extension.clone()),
    );
    Value::Object(m)
}

/// A plugin the policy allows, ready to run.
#[derive(Clone, PartialEq, Eq)]
pub struct Plugin {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `KUBERNETES_EXEC_INFO`'s JSON.
    pub exec_info: String,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.env.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Plugin")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &names)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn exec(command: &str, args: &[&str]) -> ExecConfig {
        serde_json::from_value(json!({
            "apiVersion": "client.authentication.k8s.io/v1",
            "command": command,
            "args": args,
        }))
        .unwrap()
    }
    fn cluster() -> Cluster {
        serde_json::from_value(json!({"server": "https://k.example:443",
            "certificate-authority-data": "Q0E=", "tls-server-name": "k"}))
        .unwrap()
    }
    fn on() -> ExecPolicy {
        ExecPolicy::parse("default").unwrap()
    }
    fn err(p: &ExecPolicy, e: &ExecConfig) -> String {
        p.check(e, &cluster()).unwrap_err().to_string()
    }

    #[test]
    fn off_by_default_and_when_empty() {
        for p in [
            ExecPolicy::default(),
            ExecPolicy::parse("").unwrap(),
            ExecPolicy::parse(" , ").unwrap(),
        ] {
            assert!(!p.is_on());
            assert!(err(&p, &exec("aws", &["eks", "get-token"])).contains(OFF));
        }
    }

    #[test]
    fn default_enables_the_built_ins_and_extras_add_names() {
        let p = ExecPolicy::parse("default, my-auth").unwrap();
        for c in BUILT_IN {
            let refused = p
                .check(&exec(c, &[]), &cluster())
                .err()
                .map(|e| e.to_string());
            assert!(!refused.is_some_and(|e| e.contains("not enabled")), "{c}");
        }
        assert!(p
            .check(&exec("my-auth", &["anything", "goes"]), &cluster())
            .is_ok());
        let only = ExecPolicy::parse("my-auth").unwrap();
        assert!(err(&only, &exec("aws", &["eks", "get-token"]))
            .contains("exec plugin 'aws' is not enabled on this server (--kube-exec-plugins)"));
        assert!(ExecPolicy::parse("my auth").is_err());
        assert!(ExecPolicy::parse("/bin/sh").is_err());
    }

    #[test]
    fn a_path_is_never_a_plugin() {
        let p = ExecPolicy::parse("default,aws").unwrap();
        for c in ["/usr/bin/aws", "./aws", "bin/aws", "..\\aws"] {
            assert!(
                err(&p, &exec(c, &["eks", "get-token"]))
                    .contains("must be a bare name on the server's PATH"),
                "{c}"
            );
        }
    }

    #[test]
    fn aws_only_gets_tokens() {
        let p = on();
        assert!(p
            .check(
                &exec(
                    "aws",
                    &[
                        "eks",
                        "get-token",
                        "--cluster-name",
                        "c",
                        "--region",
                        "eu-west-1"
                    ]
                ),
                &cluster()
            )
            .is_ok());
        assert!(
            err(&p, &exec("aws", &["s3", "rm", "s3://b", "--recursive"]))
                .contains("may only run 'eks get-token'")
        );
        assert!(err(&p, &exec("aws", &[])).contains("may only run 'eks get-token'"));
        assert!(p
            .check(
                &exec("aws-iam-authenticator", &["token", "-i", "c"]),
                &cluster()
            )
            .is_ok());
        assert!(err(&p, &exec("aws-iam-authenticator", &["init"])).contains("may only run 'token'"));
        assert!(p
            .check(&exec("gke-gcloud-auth-plugin", &[]), &cluster())
            .is_ok());
    }

    #[test]
    fn kubelogin_needs_a_login_mode_without_a_person() {
        let p = on();
        for ok in [
            vec!["get-token", "--login", "azurecli", "--server-id", "x"],
            vec!["get-token", "-l", "msi"],
            vec!["get-token", "--login=workloadidentity"],
            vec!["get-token", "-l=spn"],
            vec!["get-token", "--login", "azd"],
        ] {
            assert!(
                p.check(&exec("kubelogin", &ok), &cluster()).is_ok(),
                "{ok:?}"
            );
        }
        for bad in [
            vec!["get-token"],
            vec!["get-token", "--login", "devicecode"],
            vec!["get-token", "-l", "interactive"],
            vec!["convert-kubeconfig", "-l", "azurecli"],
        ] {
            assert!(
                err(&p, &exec("kubelogin", &bad)).contains("kubelogin"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn env_names_are_checked_and_values_kept() {
        let p = on();
        let mut e = exec("aws", &["eks", "get-token"]);
        e.env = Some(vec![HashMap::from([
            ("name".into(), "AWS_PROFILE".into()),
            ("value".into(), "prod".into()),
        ])]);
        assert_eq!(
            p.check(&e, &cluster()).unwrap().env,
            [("AWS_PROFILE".to_string(), "prod".to_string())]
        );
        for bad in [
            "PATH",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "KUBERNETES_EXEC_INFO",
            "1X",
            "A-B",
            "",
        ] {
            e.env = Some(vec![HashMap::from([
                ("name".into(), bad.into()),
                ("value".into(), "v".into()),
            ])]);
            assert!(err(&p, &e).contains("is not allowed"), "{bad}");
        }
    }

    #[test]
    fn env_that_makes_an_interpreter_load_code_or_config_from_a_path_is_refused() {
        let p = on();
        let mut e = exec("aws", &["eks", "get-token"]);
        for bad in [
            "PYTHONPATH",
            "PYTHONSTARTUP",
            "NODE_OPTIONS",
            "NODE_PATH",
            "BASH_ENV",
            "ENV",
            "PERL5LIB",
            "PERL5OPT",
            "RUBYOPT",
            "RUBYLIB",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
            "JDK_JAVA_OPTIONS",
            "GCONV_PATH",
            "HOME",
            "XDG_CONFIG_HOME",
            "AWS_CONFIG_FILE",
            "AWS_SHARED_CREDENTIALS_FILE",
            "CLOUDSDK_CONFIG",
            "AZURE_CONFIG_DIR",
            "BOTO_CONFIG",
            "KUBECONFIG",
        ] {
            e.env = Some(vec![HashMap::from([
                ("name".into(), bad.into()),
                ("value".into(), "/tmp/x".into()),
            ])]);
            assert!(err(&p, &e).contains("is not allowed"), "{bad}");
        }
        for ok in [
            "AWS_PROFILE",
            "AWS_REGION",
            "AZURE_CLIENT_ID",
            "CLOUDSDK_CORE_PROJECT",
        ] {
            e.env = Some(vec![HashMap::from([
                ("name".into(), ok.into()),
                ("value".into(), "v".into()),
            ])]);
            assert!(p.check(&e, &cluster()).is_ok(), "{ok}");
        }
    }

    #[test]
    fn always_interactive_is_refused_and_the_rest_run_without_a_person() {
        let p = on();
        let mut e = exec("aws", &["eks", "get-token"]);
        e.interactive_mode = Some(ExecInteractiveMode::Always);
        assert!(err(&p, &e).contains("interactiveMode: Always"));
        e.interactive_mode = Some(ExecInteractiveMode::IfAvailable);
        let info: Value =
            serde_json::from_str(&p.check(&e, &cluster()).unwrap().exec_info).unwrap();
        assert_eq!(info["spec"]["interactive"], false);
        assert_eq!(info["kind"], "ExecCredential");
        assert_eq!(info["apiVersion"], "client.authentication.k8s.io/v1");
        assert!(info["spec"].get("cluster").is_none());
    }

    #[test]
    fn provide_cluster_info_passes_the_cluster() {
        let mut e = exec("gke-gcloud-auth-plugin", &[]);
        e.provide_cluster_info = true;
        let info: Value =
            serde_json::from_str(&on().check(&e, &cluster()).unwrap().exec_info).unwrap();
        assert_eq!(info["spec"]["cluster"]["server"], "https://k.example:443");
        assert_eq!(
            info["spec"]["cluster"]["certificate-authority-data"],
            "Q0E="
        );
        assert_eq!(info["spec"]["cluster"]["tls-server-name"], "k");
    }

    #[test]
    fn a_missing_api_version_defaults_to_v1beta1() {
        let mut e = exec("aws", &["eks", "get-token"]);
        e.api_version = None;
        let info: Value =
            serde_json::from_str(&on().check(&e, &cluster()).unwrap().exec_info).unwrap();
        assert_eq!(info["apiVersion"], "client.authentication.k8s.io/v1beta1");
    }

    #[test]
    fn debug_never_shows_env_values() {
        let mut e = exec("aws", &["eks", "get-token"]);
        e.env = Some(vec![HashMap::from([
            ("name".into(), "AWS_SECRET".into()),
            ("value".into(), "s3cr3t".into()),
        ])]);
        let shown = format!("{:?}", on().check(&e, &cluster()).unwrap());
        assert!(
            !shown.contains("s3cr3t") && shown.contains("AWS_SECRET"),
            "{shown}"
        );
    }

    #[test]
    fn the_timeout_is_thirty_seconds_unless_set() {
        assert_eq!(on().timeout(), Duration::from_secs(30));
        assert_eq!(
            on().with_timeout(Duration::from_secs(1)).timeout(),
            Duration::from_secs(1)
        );
    }
}
