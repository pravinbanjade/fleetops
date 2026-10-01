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
    #[arg(long, env = "FLEETOPS_AGENT_LOG", default_value = "info")]
    pub log: String,
    /// How long the server may stay silent (it pings every 15 s) before the
    /// agent treats the connection as dead and dials again.
    #[arg(skip = Duration::from_secs(45))]
    pub server_silence: Duration,
}
