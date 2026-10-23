# Docker Swarm

FleetOps manages a Docker Swarm through an agent on every node. This page covers adding a swarm, the two ways to deploy the agents, key rotation and what FleetOps checks before it trusts a node.

## Add a swarm

In **Admin → Environments → Add environment**, choose **Docker Swarm** and name it. You get, once:

1. **A key.** On a manager, run `docker secret create fleetops-agent-key -`, paste the key and press Ctrl-D. The key never appears in a command line, a file or the stack file. FleetOps keeps only its hash.
2. **A stack file**, `fleetops-agent.yml`. Deploy it from a manager with `docker stack deploy -c fleetops-agent.yml fleetops-agent`. It runs `fleetops-agent --swarm` as a global service, one task per node, now and on every node that joins later.

The environment's **Nodes** page lists every node of the swarm and whether its agent is connected, with the reason when one was refused. The environment is up while at least one manager's agent is connected.

The agent runs as root (`user: "0"`). It needs the node's Docker socket, and it reads the node's Swarm certificate from `/var/lib/docker/swarm/certificates`, mounted read-only, which only root can read.

## FleetOps outside or inside the swarm

The dialog offers two variants of the stack file:

- **FleetOps runs outside the swarm.** The agents dial the address your browser used to reach FleetOps. It must be HTTPS (`--tls-bind`); for a self-signed certificate, add the agent's `--ca-pem` (`FLEETOPS_AGENT_CA_PEM`) with the certificate.
- **FleetOps runs inside the swarm.** Deploy FleetOps itself with [`deploy/swarm/fleetops.yml`](../deploy/swarm/fleetops.yml): label one manager first (`docker node update --label-add fleetops=true <node>`), then `docker stack deploy -c fleetops.yml fleetops`. FleetOps runs as one replica on that manager, keeps its database in the `fleetops-data` volume there, and creates the encrypted, attachable overlay network `fleetops`. The agents join that network and dial `https://fleetops:9443`, trusting the certificate FleetOps generated, which the stack file carries. This variant is offered only while FleetOps serves its own generated certificate (`--tls-bind` without `--tls-cert`).

## How a node is trusted

Every agent of a swarm shares the key, so the key alone does not say which node an agent runs on. Before it accepts an agent, FleetOps:

1. Sends a random challenge, which the agent signs with its node's Swarm key.
2. Checks the node's certificate against the swarm's root CA, and that its subject names the node the agent claims (common name), this swarm (organization) and the node's role (`swarm-manager` or `swarm-worker`).
3. Asks a manager whether the node is still a member, with that role. A removed node's certificate stays valid until it expires; membership is what refuses it.

The first manager agent to connect sets which swarm the environment belongs to and pins its root CA. Worker agents wait until a manager's agent is connected and retry on their own.

After rotating the swarm's root CA (`docker swarm ca --rotate`), agents are refused until you click **Re-trust swarm** on the Nodes page. Every agent then reconnects, and the first manager pins the new root CA.

**Autolock is not supported.** With `docker swarm update --autolock=true`, a manager's node key is encrypted on disk, so its agent cannot prove which node it runs on. It stops with a message saying so.

## Rotating the key

**Rotate key** on the environment gives a new key and a stack file that reads it from a new secret, `fleetops-agent-key-2` (then `-3`, and so on): Docker secrets cannot change. Create the new secret and redeploy the stack. The old key keeps working for 24 hours, so running agents stay connected meanwhile. Click **Finish rotation** once every node runs the new stack: agents still on the old key are disconnected and the old key stops working. You can then remove the old secret.

## Agent flags in Swarm mode

`--swarm` (`FLEETOPS_AGENT_SWARM`), `--key-file` (`FLEETOPS_AGENT_KEY_FILE`, the secret's path), `--swarm-certs` (`FLEETOPS_AGENT_SWARM_CERTS`, default `/swarm-certs`) and `--ca-pem` (`FLEETOPS_AGENT_CA_PEM`), next to `--server`, `--docker` and `--log`. An agent in Swarm mode never writes a key; if the server rejects it, the agent exits with a message naming the secret to update.
