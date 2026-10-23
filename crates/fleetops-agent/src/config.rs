use clap::Parser;
use std::{path::PathBuf, time::Duration};

/// fleetops-agent: connects this host's Docker to a FleetOps server.
#[derive(Debug, Clone, Parser)]
#[command(version)]
pub struct Config {
    /// The FleetOps server, e.g. https://fleet.example.com
    #[arg(long, env = "FLEETOPS_AGENT_SERVER")]
    pub server: String,
    /// One-time join token from the server's "Add agent" dialog. Used only
    /// until a key is stored in --data.
    #[arg(long, env = "FLEETOPS_AGENT_JOIN_TOKEN")]
    pub join_token: Option<String>,
    /// Holds agent.key.
    #[arg(long, env = "FLEETOPS_AGENT_DATA", default_value = "/data")]
    pub data: PathBuf,
    /// The Docker endpoint to manage.
    #[arg(
        long,
        env = "FLEETOPS_AGENT_DOCKER",
        default_value = "unix:///var/run/docker.sock"
    )]
    pub docker: String,
    /// Extra CA certificate (PEM) to trust for the server, e.g. its
    /// self-signed certificate.
    #[arg(long, env = "FLEETOPS_AGENT_CA")]
    pub ca: Option<PathBuf>,
    /// The same as --ca, given inline as PEM text.
    #[arg(long, env = "FLEETOPS_AGENT_CA_PEM")]
    pub ca_pem: Option<String>,
    #[arg(long, env = "FLEETOPS_AGENT_LOG", default_value = "info")]
    pub log: String,
    /// How long the server may stay silent (it pings every 15 s) before the
    /// agent treats the connection as dead and dials again.
    /// Serve the in-cluster Kubernetes API instead of Docker; run as a pod.
    #[arg(long, env = "FLEETOPS_AGENT_KUBE", conflicts_with = "docker")]
    pub kube: bool,
    /// Keep the agent key in this Secret (in $POD_NAMESPACE), not in --data.
    #[arg(long, env = "FLEETOPS_AGENT_KEY_SECRET", requires = "kube")]
    pub key_secret: Option<String>,
    /// Serve one Docker Swarm node; run as a global Swarm service.
    #[arg(long, env = "FLEETOPS_AGENT_SWARM", conflicts_with = "kube")]
    pub swarm: bool,
    /// Read the agent key from this file (a Docker secret). The agent never
    /// writes it.
    #[arg(long, env = "FLEETOPS_AGENT_KEY_FILE", requires = "swarm")]
    pub key_file: Option<PathBuf>,
    /// The node's Swarm certificates (/var/lib/docker/swarm/certificates).
    #[arg(
        long,
        env = "FLEETOPS_AGENT_SWARM_CERTS",
        default_value = "/swarm-certs"
    )]
    pub swarm_certs: PathBuf,
    /// Tests: the apiserver to use instead of the in-cluster one.
    #[arg(skip)]
    pub kube_upstream: Option<crate::kube_proxy::KubeUpstream>,
    #[arg(skip = Duration::from_secs(45))]
    pub server_silence: Duration,
}
