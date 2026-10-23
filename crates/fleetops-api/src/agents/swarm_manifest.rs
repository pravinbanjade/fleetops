//! The agent stack for a Swarm environment (swarm spec §3): a global
//! service with the node's Docker socket and Swarm certificates, its key
//! from a Docker secret. Two variants: agents dial FleetOps' public URL
//! (FleetOps outside the swarm), or FleetOps' service over the shared
//! `fleetops` overlay network, trusting its generated certificate (inside).

use super::AGENT_IMAGE;
use serde::Serialize;
use utoipa::ToSchema;

/// The secret holding generation 1 of the key; later ones add `-N`.
pub const KEY_SECRET: &str = "fleetops-agent-key";
/// Where agents inside the swarm reach FleetOps (`deploy/swarm/fleetops.yml`).
pub const INSIDE_URL: &str = "https://fleetops:9443";
/// The overlay network FleetOps' own stack creates.
pub const INSIDE_NETWORK: &str = "fleetops";
pub const DEPLOY_COMMAND: &str = "docker stack deploy -c fleetops-agent.yml fleetops-agent";

pub fn secret_name(generation: i64) -> String {
    if generation <= 1 {
        KEY_SECRET.to_string()
    } else {
        format!("{KEY_SECRET}-{generation}")
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SwarmStack {
    /// Save as fleetops-agent.yml.
    pub stack_yaml: String,
    pub deploy_command: String,
}

/// What an admin needs to start (or re-key) a swarm's agents.
#[derive(Debug, Serialize, ToSchema)]
pub struct SwarmEnrollment {
    /// Shown once. Paste it into `secret_command` on a manager.
    pub agent_key: String,
    pub key_generation: i64,
    pub secret_name: String,
    /// Run on a manager, paste the key, then press Ctrl-D.
    pub secret_command: String,
    /// FleetOps runs outside the swarm: agents dial its public URL.
    pub outside: SwarmStack,
    /// FleetOps runs inside the swarm; null when it is not offered.
    pub inside: Option<SwarmStack>,
    /// Why `inside` is not offered.
    pub inside_unavailable: Option<String>,
}

pub const INSIDE_UNAVAILABLE: &str = "FleetOps is not serving its generated certificate on an \
     HTTPS port (--tls-bind without --tls-cert), so agents inside the swarm cannot verify it; \
     use the outside variant";

/// The stack file. `ca_pem` is set for the inside variant only.
pub fn swarm_stack(server_url: &str, secret: &str, ca_pem: Option<&str>) -> String {
    let inside = ca_pem.is_some();
    let mut y = format!(
        "# FleetOps agent: one per node. Deploy from a manager with\n\
         #   {DEPLOY_COMMAND}\n\
         version: \"3.8\"\n\
         services:\n  \
           agent:\n    \
             image: {AGENT_IMAGE}:{version}\n    \
             # Swarm's node certificates are readable by root only.\n    \
             user: \"0\"\n    \
             command: [\"--server\", \"{server_url}\", \"--swarm\", \"--key-file\", \"/run/secrets/{KEY_SECRET}\"]\n",
        version = env!("CARGO_PKG_VERSION"),
    );
    if let Some(pem) = ca_pem {
        y.push_str("    environment:\n      FLEETOPS_AGENT_CA_PEM: |\n");
        for line in pem.trim_end().lines() {
            y.push_str("        ");
            y.push_str(line);
            y.push('\n');
        }
    }
    y.push_str(&format!(
        "    volumes:\n      \
           - /var/run/docker.sock:/var/run/docker.sock\n      \
           - /var/lib/docker/swarm/certificates:/swarm-certs:ro\n    \
         secrets:\n      \
           - source: {secret}\n        \
             target: {KEY_SECRET}\n"
    ));
    if inside {
        y.push_str(&format!("    networks:\n      - {INSIDE_NETWORK}\n"));
    }
    y.push_str(&format!(
        "    deploy:\n      \
           mode: global\n      \
           restart_policy:\n        \
             condition: any\n        \
             delay: 5s\n\
         secrets:\n  \
           {secret}:\n    \
             external: true\n"
    ));
    if inside {
        y.push_str(&format!(
            "networks:\n  {INSIDE_NETWORK}:\n    external: true\n"
        ));
    }
    y
}

/// The enrollment for `key` at `generation`. `public_url` is where the
/// admin's browser reached FleetOps.
pub fn enrollment(
    key: String,
    generation: i64,
    public_url: &str,
    ca_pem: Option<&str>,
) -> SwarmEnrollment {
    let secret = secret_name(generation);
    let stack = |url: &str, ca: Option<&str>| SwarmStack {
        stack_yaml: swarm_stack(url, &secret, ca),
        deploy_command: DEPLOY_COMMAND.into(),
    };
    SwarmEnrollment {
        agent_key: key,
        key_generation: generation,
        secret_command: format!("docker secret create {secret} -"),
        outside: stack(public_url, None),
        inside: ca_pem.map(|ca| stack(INSIDE_URL, Some(ca))),
        inside_unavailable: ca_pem.is_none().then(|| INSIDE_UNAVAILABLE.into()),
        secret_name: secret,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const CA: &str = "-----BEGIN CERTIFICATE-----\nMIIB\nAAAA\n-----END CERTIFICATE-----\n";

    fn parse(y: &str) -> Value {
        serde_saphyr::from_str(y).unwrap_or_else(|e| panic!("{e}\n{y}"))
    }

    #[test]
    fn the_outside_stack_is_a_global_service_with_the_certs_and_secret() {
        let v = parse(&swarm_stack("https://fleet.example.com", "fleetops-agent-key-3", None));
        let agent = &v["services"]["agent"];
        assert_eq!(agent["deploy"]["mode"], "global");
        assert_eq!(agent["user"], "0");
        assert_eq!(
            agent["command"],
            serde_json::json!(["--server", "https://fleet.example.com", "--swarm", "--key-file", "/run/secrets/fleetops-agent-key"])
        );
        assert_eq!(agent["volumes"][1], "/var/lib/docker/swarm/certificates:/swarm-certs:ro");
        assert_eq!(agent["secrets"][0]["source"], "fleetops-agent-key-3");
        assert_eq!(agent["secrets"][0]["target"], "fleetops-agent-key");
        assert_eq!(v["secrets"]["fleetops-agent-key-3"]["external"], true);
        assert!(agent.get("networks").is_none());
        assert!(v.get("networks").is_none());
    }

    #[test]
    fn the_inside_stack_joins_the_overlay_and_trusts_the_ca() {
        let v = parse(&swarm_stack(INSIDE_URL, KEY_SECRET, Some(CA)));
        let agent = &v["services"]["agent"];
        assert_eq!(agent["command"][1], "https://fleetops:9443");
        assert_eq!(agent["environment"]["FLEETOPS_AGENT_CA_PEM"], CA);
        assert_eq!(agent["networks"][0], "fleetops");
        assert_eq!(v["networks"]["fleetops"]["external"], true);
    }

    #[test]
    fn secrets_are_named_per_generation() {
        assert_eq!(secret_name(1), "fleetops-agent-key");
        assert_eq!(secret_name(2), "fleetops-agent-key-2");
    }
}
