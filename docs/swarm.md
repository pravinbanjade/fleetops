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
- **Stacks:** FleetOps stacks deployed as Swarm services (see [Stacks](#stacks)), then the stacks deployed with `docker stack deploy`.
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

Swarm networks use the **Networks** row. A node's pages use the Docker rows (containers, images, volumes, networks). The built-in Viewer gets every View cell and config content; Operator gets every swarm cell.

## Stack scoping

A grant on a swarm can be limited to stacks, the way a Kubernetes grant is limited to namespaces. In the access dialog, untick **Entire swarm** and list the stacks. An object belongs to a stack by its `com.docker.stack.namespace` label. An object without that label belongs to no stack, and only a grant on the whole swarm reaches it.

**What a grant limited to stacks reaches:**
- **Cluster objects:** the services, tasks, secrets, configs and networks of its stacks. Lists show only those, and a service's logs need its stack.
- **Changes:** scaling, redeploying, rolling back, editing and removing its stacks' services, and removing their secrets, configs and networks. An edit cannot move a service out of its stack, by relabelling it or dropping the label. It cannot attach a secret, config or network that belongs to another stack.
- **Stacks:** its own stacks, and only those can be created or deployed. A deploy cannot name an external secret, config or network of another stack. A Git deploy stays within the stacks of whoever last saved the stack's Git settings, and of whoever started the sync. It stops if either no longer has the stack. It also stops if the person who saved the settings has been deleted; saving them again resumes it.
- **Nodes:** the nodes list, read-only, so the visualizer works. A node's own page, and every node change, needs the whole swarm.
- **Node pages:** only containers, and only its stacks' containers there: their logs, terminals, files, stats and actions. Images, volumes, networks, disk usage and Compose projects reach the whole node, so they are hidden. So is creating, duplicating or pruning containers.
- **Live updates:** events, logs and the task feed carry only objects in its stacks. Node events carry no stack, so they reach anyone who may list nodes.

**What it cannot do:** create services, secrets, configs or networks directly. It creates them by deploying one of its stacks. A stack's network that already exists outside it must be declared `external`, so it is checked as one.

**What it cannot learn.** An object outside its stacks reads exactly as one that does not exist, in lists, errors and plans alike.

**Several grants** add up as for Kubernetes: an action is allowed when one grant both holds the permission and covers the object. An env-wide Viewer plus an Operator limited to `blog` sees everything and changes only `blog`.

**Scoping is not a sandbox.** It limits what a person sees and manages through FleetOps. Someone who can deploy a stack can still run what Docker allows on the nodes, such as a container that mounts a host path. Give deploy rights on a swarm only to people you would trust on its nodes.

## Self-protection

FleetOps asks you to type the name before a change that could stop FleetOps or cut it off from the swarm:

- any change to the **agent service** (scale, edit, redeploy, roll back, remove);
- any change to **FleetOps' own service**, when FleetOps runs inside the swarm;
- **draining, demoting, relabelling or removing the node** that runs FleetOps' own service: its stack places it by role and label.

Both services are known by the label `io.fleetops.role` (`agent` or `server`), which the generated agent stack and `deploy/swarm/fleetops.yml` set, or by their image (`fleets/fleetops-agent`, `fleets/fleetops`) for stacks deployed before the label existed. If you run FleetOps from another image, add `io.fleetops.role: server` under `deploy: labels:` in your stack file.

## Stacks

A stack on a Swarm environment deploys its Compose file as Swarm services. Everything else is as for stacks on a Docker host: the editor, Git with polling and webhooks, variables, the Secrets tab, revisions, rollback, and a reviewed plan before every deploy.

**What the file may say.**
- **Honoured:**
  - the whole `deploy:` block: `mode` (replicated, global, replicated-job, global-job), `replicas`, `placement` (constraints, spread preferences, `max_replicas_per_node`), `resources` (limits and reservations), `restart_policy`, `update_config`, `rollback_config`, `labels` and `endpoint_mode`;
  - a port's `mode: host`;
  - secrets and configs marked `external: true`, optionally with a `name:`.
- **Refused:** `build:`. Swarm cannot build, so push the image to a registry and name it in `image:`. Nodes must already be able to pull it (`docker login` on each node).
- **Ignored, with a warning:** what `docker stack deploy` also ignores:
  - `container_name`, `depends_on`, `links`, `external_links`, `network_mode`, `devices`, `tmpfs`, `privileged`, `pull_policy`, `shm_size`, `cgroup_parent`, `security_opt` and `userns_mode`;
  - `restart`: use `deploy.restart_policy`;
  - `mem_limit` and `cpus`: use `deploy.resources.limits`;
  - a port's `host_ip`, and a network's `ipv4_address`.
- **Networks** use the overlay driver. A stack's `default` network is `<stack>_default`.

**Names and labels.**
- Objects are named as `docker stack deploy` names them: service `web` of stack `shop` is `shop_web`.
- Every service, network, secret and config carries `com.docker.stack.namespace=<stack>`.
- A secret or config gets the stack name, its key and a fingerprint of its value: `shop_db_3f9a1c07d2`. Swarm cannot change a secret, so a new value makes a new one. The deploy moves the service onto it, then removes the old one.
- Each service also carries `io.fleetops.stack.spec-hash`, the hash of the spec FleetOps deployed.

**Drift.**
- A stack's services can still be edited, scaled, redeployed and rolled back on the Services page. Their page says that the stack manages them, and that the next deploy overwrites such changes.
- A service whose spec no longer matches its hash has drifted. The stack's Services tab marks it and names the fields that changed. Environment values are never shown.
- The next deploy's plan lists what it will put back.

**Deploying.**
1. FleetOps creates networks, then secrets and configs, then creates or updates services.
2. It waits for each service to converge: Swarm's update completes and every task runs. The task log reports progress per service.
3. Only then does it remove the services that left the file, and the secrets, configs and networks nothing uses any more.

**When a deploy fails.**
- **What fails it:**
  - an update that Swarm pauses or rolls back;
  - a task the deploy started that is rejected (for example, an image that cannot be pulled);
  - a task that stays pending for 2 minutes (no node fits it);
  - a service that has not converged after 5 minutes.
- **What happens then:** FleetOps does not undo the deploy. Swarm already rolled each service out under its own update policy, and its `failure_action` applies. The revision is marked failed. To undo the deploy, roll back to an earlier revision.
- **How rejected and pending tasks are seen:** the deploy reads them from the same task feed the task pages use.

**Stacks deployed outside FleetOps.**
- Stacks deployed with `docker stack deploy` are listed under **Other stacks**. Their services can be managed one by one.
- To manage such a stack here, create a FleetOps stack with the same name. Its first deploy updates those services in place; the plan marks them "not deployed by FleetOps".

**Removing.** Removing a Swarm stack removes its services, then its secrets, configs and networks. Volumes live on each node and stay.

**Stopping and starting.** There is no start, stop or restart for a Swarm stack. Scale or redeploy its services on the Services page.

**What FleetOps will not change.** A plan that would update or remove FleetOps' own service or the agent service is refused, and so is removing a stack that holds one. Change those outside FleetOps: the change could stop FleetOps half way.

## Node pages

Every node also opens as a Docker host of its own, at `<environment>@<node id>` (follow a node's name on the Nodes page). It has the usual Docker pages for that node's containers, images, volumes and networks, with logs, stats, terminals and files. They work through that node's own agent. Roles on the swarm apply to its nodes too, and actions on a node are audited with the node's id. FleetOps stacks are not available on a single node: they belong to the swarm. A node's Compose projects are still listed.

## Rotating the key

**Rotate key** on the environment gives a new key and a stack file that reads it from a new secret, `fleetops-agent-key-2` (then `-3`, and so on): Docker secrets cannot change. Create the new secret and redeploy the stack. The old key keeps working for 24 hours, so running agents stay connected meanwhile. Click **Finish rotation** once every node runs the new stack: agents still on the old key are disconnected and the old key stops working. You can then remove the old secret.

## Agent flags in Swarm mode

`--swarm` (`FLEETOPS_AGENT_SWARM`), `--key-file` (`FLEETOPS_AGENT_KEY_FILE`, the secret's path), `--swarm-certs` (`FLEETOPS_AGENT_SWARM_CERTS`, default `/swarm-certs`) and `--ca-pem` (`FLEETOPS_AGENT_CA_PEM`), next to `--server`, `--docker` and `--log`. An agent in Swarm mode never writes a key; if the server rejects it, the agent exits with a message naming the secret to update.
