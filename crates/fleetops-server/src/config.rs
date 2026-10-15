use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser, Debug, Clone)]
#[command(name = "fleetops", version, about = "Lightweight container management")]
pub struct Config {
    /// Directory for the SQLite database and TLS material.
    #[arg(long, env = "FLEETOPS_DATA", default_value = "/data")]
    pub data: PathBuf,
    /// Plain HTTP listen address.
    #[arg(long, env = "FLEETOPS_BIND", default_value = "0.0.0.0:9000")]
    pub bind: SocketAddr,
    /// HTTPS listen address; HTTPS is disabled when unset.
    #[arg(long, env = "FLEETOPS_TLS_BIND")]
    pub tls_bind: Option<SocketAddr>,
    /// 32 bytes, base64: seals stored kubeconfigs. Unset: <data>/secret.key,
    /// created on first start. Back it up with the database.
    #[arg(long, env = "FLEETOPS_SECRET_KEY", hide_env_values = true)]
    pub secret_key: Option<String>,
    /// PEM certificate; a self-signed one is generated when unset.
    #[arg(long, env = "FLEETOPS_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key matching --tls-cert.
    #[arg(long, env = "FLEETOPS_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
    /// Send HSTS on HTTPS responses. Only enable when clients never use the
    /// plain-HTTP listener: browsers apply HSTS per host, not per port.
    #[arg(long, env = "FLEETOPS_HSTS")]
    pub hsts: bool,
    /// Docker endpoint for the local environment (used from plan P2).
    #[arg(
        long,
        env = "DOCKER_HOST",
        default_value = "unix:///var/run/docker.sock"
    )]
    pub docker_host: String,
    /// tracing filter, e.g. `info` or `fleetops_api=debug,info`.
    #[arg(long, env = "FLEETOPS_LOG", default_value = "info")]
    pub log_level: String,
    /// Exec plugins kubeconfig users may run: `default` (aws,
    /// aws-iam-authenticator, gke-gcloud-auth-plugin, kubelogin) and extra
    /// bare names, comma-separated. Off when unset.
    #[arg(long, env = "FLEETOPS_KUBE_EXEC_PLUGINS")]
    pub kube_exec_plugins: Option<String>,
    /// The helm used to render charts for install and upgrade: `helm` on
    /// PATH or an absolute path. Off when unset.
    #[arg(long, env = "FLEETOPS_HELM")]
    pub helm_binary: Option<String>,
    /// Allow http:// chart repositories (for local testing).
    #[arg(long, env = "FLEETOPS_HELM_INSECURE_REPOS")]
    pub helm_insecure_repos: bool,
    /// Enterprise license file. A license uploaded in Admin → License
    /// takes precedence.
    #[cfg(feature = "enterprise")]
    #[arg(long, env = "FLEETOPS_LICENSE_FILE")]
    pub license_file: Option<PathBuf>,
    /// Print the OpenAPI document to stdout and exit without starting.
    #[arg(long)]
    pub openapi: bool,
}
