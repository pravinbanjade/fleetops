# Docker Swarm

FleetOps manages a Docker Swarm through an agent on every node. This page covers adding a swarm, the two ways to deploy the agents, key rotation and what FleetOps checks before it trusts a node.

## Add a swarm

In **Admin → Environments → Add environment**, choose **Docker Swarm** and name it. You get, once:

1. **A key.** On a manager, run `docker secret create fleetops-agent-key -`, paste the key and press Ctrl-D. The key never appears in a command line, a file or the stack file. FleetOps keeps only its hash.
2. **A stack file**, `fleetops-agent.yml`. Deploy it from a manager with `docker stack deploy -c fleetops-agent.yml fleetops-agent`. It runs `fleetops-agent --swarm` as a global service, one task per node, now and on every node that joins later.

The first manager agent to connect proposes the swarm, and FleetOps waits for you to trust it (see [Trusting a swarm](#trusting-a-swarm)). The environment's **Nodes** page lists every node of the swarm and whether its agent is connected, with the reason when one was refused. The environment is up while at least one manager's agent is connected.

The agent runs as root (`user: "0"`). It needs the node's Docker socket, and it reads the node's Swarm certificate from `/var/lib/docker/swarm/certificates`, mounted read-only, which only root can read.

## FleetOps outside or inside the swarm

The dialog offers two variants of the stack file:

- **FleetOps runs outside the swarm.** The agents dial the address your browser used to reach FleetOps. It must be HTTPS (`--tls-bind`); for a self-signed certificate, add the agent's `--ca-pem` (`FLEETOPS_AGENT_CA_PEM`) with the certificate.
- **FleetOps runs inside the swarm.** Deploy FleetOps itself with [`deploy/swarm/fleetops.yml`](../deploy/swarm/fleetops.yml): label one manager first (`docker node update --label-add fleetops=true <node>`), then `docker stack deploy -c fleetops.yml fleetops`. FleetOps runs as one replica on that manager, keeps its database in the `fleetops-data` volume there, and creates the encrypted, attachable overlay network `fleetops`. The agents join that network and dial `https://fleetops:9443`, trusting the certificate FleetOps generated, which the stack file carries. This variant is offered only while FleetOps serves its own generated certificate (`--tls-bind` without `--tls-cert`).

## Trusting a swarm

FleetOps does not trust the first agent's word about which swarm it belongs to. The first manager agent that proves itself proposes its swarm. Until an admin trusts it, every agent is told to wait and retries on its own.

The environment's **Overview** then shows admins which manager proposed it, the swarm's cluster ID, and the SHA-256 fingerprint of its root CA. On a manager, run:

```sh
docker swarm ca | openssl x509 -noout -fingerprint -sha256
```

If the fingerprint matches, click **Trust this swarm**. If it does not, click **Reject**: FleetOps forgets the proposal, and the next manager agent proposes again. Trusting is audited as `swarm.trust`.

## How a node is trusted

Every agent of a swarm shares the key, so the key alone does not say which node an agent runs on. Before it accepts an agent, FleetOps:

1. Sends a random challenge, which the agent signs with its node's Swarm key.
2. Checks the node's certificate against the swarm's root CA, and that its subject names the node the agent claims (common name), this swarm (organization) and the node's role (`swarm-manager` or `swarm-worker`).
3. Asks a *known* manager whether the node is still a member, with that role. A removed node's certificate stays valid until it expires; membership is what refuses it.

FleetOps remembers the swarm's managers (its known managers) and refreshes the list whenever a known manager's agent connects. Only a known manager vouches for other nodes. A known manager whose agent is the only one connected, for example right after FleetOps restarts, checks itself. A newly promoted manager is let in once a known manager confirms it. A node that was removed, or demoted, drops off the list the next time a known manager connects, so its still-valid certificate no longer gets it in. Worker agents wait until a known manager's agent is connected, and retry on their own.

After rotating the swarm's root CA (`docker swarm ca --rotate`), agents are refused until you click **Re-trust swarm** on the Nodes page. Every agent then reconnects, and the first manager pins the new root CA.

**Autolock is not supported.** With `docker swarm update --autolock=true`, a manager's node key is encrypted on disk, so its agent cannot prove which node it runs on. It stops with a message saying so.

## Cluster pages

A Swarm environment's sidebar has:

- **Overview:** what needs attention first (nodes that are not ready, services short of tasks, tasks that failed or were rejected in the last hour), the swarm's latest events, and which manager serves FleetOps' cluster calls.
- **Nodes:** every node with its role, state and agent. A node's page shows its address, resources, labels and tasks.
- **Services:** every service with its stack, mode, running and desired tasks, image and ports. A service's page shows its tasks, its logs from every task (the same log viewer as containers), and its spec.
- **Tasks:** every task, with its node and, when it has one, a link to its container on that node.
- **Visualizer:** one column per node with the tasks running or starting there.
- **Secrets, Configs, Networks:** what exists and which services use each. Secret values are never shown: Swarm never returns them.

These pages update live. Services, nodes, secrets, configs and networks follow the swarm's events. Swarm sends no event for tasks, and a task that fails before it gets a container produces no event at all. So while a task page is open, the server reads the task list through the manager every 3 seconds and sends only what changed. The browser never polls, and the server stops reading as soon as nobody is watching.

## Changing the swarm

- **Nodes:** activate, pause or drain a node; promote a worker or demote a manager; edit its labels. A worker that is down can be removed. FleetOps refuses to remove a node that is still up, or a manager (demote it first). Draining a node stops every task on it, its FleetOps agent included, so that node's pages go offline until it is active again.
- **Services:** create a service, or edit one as a form (image, mode, replicas, ports, environment, placement) or as Docker's raw ServiceSpec. Fields the form does not show are kept. Scale, redeploy (restart every task with the same spec), roll back to the previous spec, or remove. If someone else changed the service since you opened it, saving fails and asks you to reload. Without the Reveal cell on containers, environment values show masked, and saving keeps the real values.
- **Secrets and configs:** create and remove. A secret's value is write-only: neither Docker nor FleetOps shows it again. A config's content can be viewed with the Reveal cell, and each view is audited. Values are at most 500 KiB.
- **Networks:** create overlay networks (attachable, internal, encrypted, with an optional subnet) and remove them. Only the swarm's own networks are listed here; a node's local networks are on its node pages.

Every change goes to the audit log with the node, service or object, and secret values never appear there.

## Permissions

Swarm has its own rows in the role grid, under **Docker Swarm**:

| Row | View | Create | Operate | Delete | Reveal |
|---|---|---|---|---|---|
| Nodes | list and inspect | | availability, role, labels | remove | |
| Services & tasks | services, tasks, service logs | create | edit, scale, redeploy, roll back | remove | |
| Secrets (swarm) | names and who uses them | create | | remove | |
| Configs | names and who uses them | create | | remove | content |

Swarm networks use the **Networks** row. A node's pages use the Docker rows (containers, images, volumes, networks). The built-in Viewer gets every View cell and config content; Operator gets every swarm cell. For now these cells must apply to the whole swarm: a grant limited to stacks sees no cluster pages yet.

## Self-protection

FleetOps asks you to type the name before a change that could stop FleetOps or cut it off from the swarm:

- any change to the **agent service** (scale, edit, redeploy, roll back, remove);
- any change to **FleetOps' own service**, when FleetOps runs inside the swarm;
- **draining, demoting, relabelling or removing the node** that runs FleetOps' own service: its stack places it by role and label.

Both services are known by the label `io.fleetops.role` (`agent` or `server`), which the generated agent stack and `deploy/swarm/fleetops.yml` set, or by their image (`fleets/fleetops-agent`, `fleets/fleetops`) for stacks deployed before the label existed. If you run FleetOps from another image, add `io.fleetops.role: server` under `deploy: labels:` in your stack file.

## Node pages

Every node also opens as a Docker host of its own, at `<environment>@<node id>` (follow a node's name on the Nodes page). It has the usual Docker pages for that node's containers, images, volumes and networks, with logs, stats, terminals and files. They work through that node's own agent. Roles on the swarm apply to its nodes too, and actions on a node are audited with the node's id. FleetOps stacks are not available on a single node: they belong to the swarm. A node's Compose projects are still listed.

## Rotating the key

**Rotate key** on the environment gives a new key and a stack file that reads it from a new secret, `fleetops-agent-key-2` (then `-3`, and so on): Docker secrets cannot change. Create the new secret and redeploy the stack. The old key keeps working for 24 hours, so running agents stay connected meanwhile. Click **Finish rotation** once every node runs the new stack: agents still on the old key are disconnected and the old key stops working. You can then remove the old secret.

## Agent flags in Swarm mode

`--swarm` (`FLEETOPS_AGENT_SWARM`), `--key-file` (`FLEETOPS_AGENT_KEY_FILE`, the secret's path), `--swarm-certs` (`FLEETOPS_AGENT_SWARM_CERTS`, default `/swarm-certs`) and `--ca-pem` (`FLEETOPS_AGENT_CA_PEM`), next to `--server`, `--docker` and `--log`. An agent in Swarm mode never writes a key; if the server rejects it, the agent exits with a message naming the secret to update.
