# FleetOps

FleetOps is a lightweight, open-source container management platform, built as an alternative to Portainer. It is a single Rust binary with the web UI and a SQLite database built in. Point it at a Docker or Podman socket and you can manage containers, images, volumes, networks and Compose projects from the browser.

What sets it apart:

- **One small binary.** It starts fast and uses little memory. The target is under 50 MB of RSS when idle, on a host with about 50 containers.
- **No paid tiers.** Features such as RBAC, SSO, GitOps and audit are free. They arrive across the roadmap below.
- **Live UI.** Lists and detail pages update from Docker events, with no polling or manual refresh.
- **Built for Compose and GitOps.** Stack authoring and git sync are planned (sub-project 4).

> **Status: early development.** Sub-project 1 (core and local Docker) is complete and released as v0.1.0. Sub-project 2 (an agent for remote hosts) is implemented: one server manages many Docker hosts. Sub-project 3 (Kubernetes) is implemented: clusters are managed next to Docker hosts, by kubeconfig or through an in-cluster agent, with live lists and YAML editing (slice 3b) and per-environment roles (slice 3c). Next is slice 3d (Helm), then sub-project 4, stacks and GitOps.

## Contents

- [Features](#features)
- [Quick start](#quick-start)
- [Access](#access)
- [Configuration](#configuration)
- [Development](#development)
- [Architecture](#architecture)
- [API](#api)
- [Security](#security)
- [Roadmap](#roadmap)

## Features

### Available now

**Access and administration**
- **First-run setup.** Setup creates the admin account. If no admin is created within 10 minutes of starting, setup locks until the server restarts.
- **Sign-in.** Passwords are hashed with argon2id. Sessions use a cookie, and only the token's hash is stored. Mutating requests need a CSRF token. Failed logins are rate-limited.
- **Account page.** Users can change their password and revoke their own sessions.
- **Admin pages.** Admins manage users (create, reset password, change role, delete), environments, and the audit log.
- **Audit log.** Every mutating Docker action and admin action is recorded. Rows are pruned after 90 days by default.

**Docker, per environment**
- **Dashboard.** Container counts by state, disk usage, and host information.
- **Containers.**
  - List: filter, bulk actions, clickable ports, and the Compose project.
  - List columns for live CPU and memory, with sparklines of the last minute. Only rows on screen subscribe.
  - Detail page tabs: Overview, Inspect, Env, Mounts, Networks, Stats, Logs, Terminal, and Files (read-only browsing with single-file download).
  - **Logs:** virtualized, keeping the newest 200k lines. Follow mode pauses when you scroll up. Text or regex search with highlighting and next/previous, stdout and stderr toggles, timestamps, wrapping, tail and time-range controls, ANSI colours, and download of the lines shown or the full log. A reconnect or a restart picks up where the log left off, without repeating lines.
  - **Terminal:** xterm.js with resize. The shell is auto-detected (bash, then sh), or you can choose a shell or command and a user. Ctrl+Shift+C copies. A dropped connection ends the session rather than silently starting a new one. Every session is audited.
  - **Stats:** live CPU, memory, network and block I/O charts for the last two minutes.
  - Create form, with an advanced JSON override.
  - **Recreate with changes:** if the new container fails to start, FleetOps rolls back to the old one.
  - **Duplicate:** host ports are cleared so the copy doesn't clash with the original.
- **Images.** List, pull with layer progress, build from a Dockerfile or tar context with a streamed log, tag, history, inspect, remove and prune.
- **Volumes.** List, create with driver options, inspect, remove and prune.
- **Networks.** List, create (subnet, gateway, internal, attachable), connect or disconnect containers, remove and prune.
- **Compose projects.** Containers grouped by project, with start, stop, restart and down.

**Remote hosts**
- **Dial-out agent.** `fleetops-agent` runs on each remote host and connects out to the server, so the host opens no port and never exposes its Docker socket.
- **One-time join command.** Admin → Environments → Add → Agent gives a `docker run` command with a 15-minute, single-use join token. The agent swaps it for its own key on first connect.
- **Live state.** Agent environments show as online, offline (with when they were last seen) or waiting, and switch live as agents connect and drop.
- **Key rotation.** Rotating an agent's key disconnects it and issues a new join command. Deleting the environment revokes the agent.
- **Everything works remotely.** Lists, live events, logs, terminal, stats, files and image builds work on agent environments exactly as on the local one.

**Interface**
- Command palette (⌘K / Ctrl-K) to jump to any resource or run an action on it.
- `/` focuses the list filter.
- Dark, light and system themes.
- Values that look like secrets are masked until clicked. This covers env, inspect JSON, image env, and volume driver options.

## Quick start

### Requirements

| Tool | Version |
|---|---|
| Rust | stable, 1.86 or newer (`rust-toolchain.toml` pins stable with rustfmt and clippy) |
| Node.js | 24 or newer |
| pnpm | 12 (`packageManager` in `web/package.json`) |
| Docker or Podman | A reachable Docker-compatible socket |

### Build and run

```sh
git clone git@gitlab.com:pravinbanjade/fleetops.git
cd fleetops
make build                      # builds web/dist, then the release binary with the UI embedded
./target/release/fleetops --data ./data --bind 127.0.0.1:9000
```

Open <http://127.0.0.1:9000> and create the admin account. You have 10 minutes from startup to do this.

When the socket isn't at `/var/run/docker.sock`, set `DOCKER_HOST`. This applies to Colima, rootless Docker and Podman:

```sh
DOCKER_HOST=unix://$HOME/.colima/default/docker.sock ./target/release/fleetops --data ./data
# or take it from the current docker context:
DOCKER_HOST=$(docker context inspect --format '{{.Endpoints.docker.Host}}') ./target/release/fleetops --data ./data
```

On first start, FleetOps registers a `local` environment when it can reach the socket.

> Access to the Docker socket is equivalent to root access on the host. Don't expose FleetOps to untrusted networks without TLS, and create the admin account right away.

### Run a release

Each release publishes `docker.io/pravinbanjade/fleetops:<version>` (and `latest`) for linux/amd64 and linux/arm64, and static linux binaries with SHA-256 checksums on the [releases page](https://gitlab.com/pravinbanjade/fleetops/-/releases). Run the image with the `docker run` below, using `docker.io/pravinbanjade/fleetops:latest` in place of `fleetops:dev`.

### Run the container image

`make image` builds `fleetops:dev` for linux/amd64 and linux/arm64: a static binary of about 17 MB on `distroless/static`, running as the non-root user `nonroot` (uid 65532). The build runs entirely inside Docker BuildKit, so it needs no Rust or Node toolchain on the host. Loading both platforms needs Docker's containerd image store; otherwise build one with `make image PLATFORMS=linux/amd64`.

```sh
make image
docker run -d --name fleetops -p 9000:9000 \
  -v fleetops-data:/data \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --group-add "$(stat -c %g /var/run/docker.sock)" \
  fleetops:dev
```

The container reaches the socket through the group that owns it, which `--group-add` grants. That group is root-equivalent on the host, like the socket itself. On Docker Desktop and Colima the socket is inside a VM, so read the group from a container instead: `docker run --rm -v /var/run/docker.sock:/s busybox stat -c %g /s`.

`make image-smoke` starts the image against the local daemon and checks that it serves the UI and can reach Docker.

### Add a remote host

1. In **Admin → Environments**, choose **Add environment → Agent on a remote host** and name it.
2. Run the command it shows on the remote host. It starts `docker.io/pravinbanjade/fleetops-agent` with the host's Docker socket and a `fleetops-agent` volume for its key.
3. The environment shows as online within a few seconds.

The agent must reach the server over HTTPS (start the server with `--tls-bind`). Plain HTTP is accepted only from the same machine. For a self-signed certificate, mount it into the agent and pass `--ca /ca.pem`. Agent flags: `--server`, `--join-token` (first start only), `--data` (default `/data`), `--docker` (default `unix:///var/run/docker.sock`), `--ca`, `--log`; each has a `FLEETOPS_AGENT_*` variable.

### Add a Kubernetes cluster

There are two ways in, both under **Admin → Environments → Add environment**:

- **Kubernetes cluster (kubeconfig).** Paste a kubeconfig and pick a context. Its user must authenticate with a bearer token (a ServiceAccount token works well) or a client certificate, and every certificate and key must be inline (`*-data`). `exec` plugins such as `aws eks get-token` and `auth-provider` users are refused: the server image has no CLIs. The server checks that the cluster answers before saving, unless you tick **Save without checking**. The kubeconfig is stored encrypted and never shown again.
- **Kubernetes cluster (in-cluster agent).** You get a `kubectl apply` command. Run it with a context for the target cluster. It installs `fleetops-agent --kube` in the `fleetops` namespace. The agent dials out to this server, so the cluster needs no inbound access, and the server never holds the cluster's credentials. The agent keeps its key in the Secret `fleetops-agent-key`. Rotating the key gives a new command; re-applying it restarts the agent with the new join token.

When you add an agent cluster you pick what its ClusterRole allows:

| Access level | Grants |
|---|---|
| **Read-only** | `get`, `list` and `watch` on namespaces, nodes, pods, services, configmaps, secrets, events, deployments, statefulsets, daemonsets, jobs, cronjobs and ingresses; `get` on `pods/log`. No changes and no terminal. |
| **Standard** (default) | Read-only, plus `create`, `update` and `patch` on those kinds, `delete` on the namespaced ones, the `scale` subresources, and `create` on `pods/exec`. |
| **Cluster admin** | Standard, plus a second binding, `fleetops-agent-cluster-admin`, to the built-in `cluster-admin` role. Needed to apply other kinds (CRDs, RBAC objects): full control of the cluster. |

The level is kept across key rotations. Raising it is a re-apply of the new command. A binding's role cannot change, so lowering it from cluster admin also needs `kubectl delete clusterrolebinding fleetops-agent-cluster-admin`; the lower manifests say so in a comment. When the cluster refuses a write, its 403 reason is shown with the level that would allow it.

What you can do: browse the 12 common kinds in one namespace or all of them. Open an object to see a summary, its YAML (`managedFields` stripped) and its events. Follow pod logs and open a pod terminal. Scale, restart or delete objects.

- **Live updates.** Lists and object pages follow the cluster through watch streams, shared between everyone viewing the same list and stopped 30 s after the last viewer leaves. When a watch cannot continue (for example the cluster forbids it), the page says *Live updates paused* with the reason and a Retry button, and never presents old data as live.
- **Editing.** *Edit YAML* on an object page, *Create* on a list (from a starter template), and *Apply YAML* (top bar or command palette) for any YAML, several documents at once and any kind the cluster serves. Every write is a dry run first, shown as a diff; nothing is written until you apply. Create and apply use server-side apply as field manager `fleetops`; a field owned by another manager is a conflict you can force. An edit is refused if the object changed since you opened it, and offers to reapply your edit on top of the new version.
- **Secrets.** Values are never shown, including the copy kubectl keeps in the `last-applied-configuration` annotation. Leave a masked value unchanged and the real one is kept; change it and the new value is written.

Helm comes later.

**The secret key.** Stored kubeconfigs are sealed with AES-256-GCM. The key is `FLEETOPS_SECRET_KEY` (32 bytes, base64), or `<data>/secret.key`, which is created on first start. Back it up with `fleetops.db`. Without it, stored kubeconfigs cannot be read; such environments show *credential cannot be decrypted; re-enter the kubeconfig*.

## Access

Admins manage users, environments and access, and can do everything everywhere. Every other account works only in the environments an admin grants it, from **Users → Access**:

- **Viewer** reads everything the grant covers: lists, details, logs, stats, events and YAML.
- **Operator** also changes it: start, stop, remove, pull, create, Compose actions, delete, scale, restart, edit and apply YAML, and terminals (a shell is a change).

An environment a user has no role on does not exist for them: it is missing from their list and its URLs answer *not found*.

**Namespaces.** A role on a Kubernetes environment can be limited to some namespaces. The user then sees and acts on objects in those namespaces only; *All namespaces* means all of theirs. Of the cluster-wide kinds they can read Nodes, and Namespaces in their list; they cannot change or apply anything cluster-wide. FleetOps enforces this boundary, not the cluster: the agent or kubeconfig keeps its own permissions. A namespace that is deleted stays in the grant, so recreating it restores access. An operator limited to some namespaces can still create pods there with any service account or host access the cluster allows, so give namespace-limited operators only namespaces whose cluster policies (Pod Security, RBAC) you trust.

**Upgrading.** Existing non-admin accounts keep operator access to every environment that existed before the upgrade. A new environment is visible only to admins until access is granted. Refused actions are recorded in the audit log as *denied*.

## Configuration

Configuration comes from CLI flags or environment variables. There is no config file.

| Flag | Environment variable | Default | Description |
|---|---|---|---|
| `--data` | `FLEETOPS_DATA` | `/data` | Directory for `fleetops.db` (SQLite, WAL mode) and TLS material |
| `--bind` | `FLEETOPS_BIND` | `0.0.0.0:9000` | Plain HTTP listen address |
| `--tls-bind` | `FLEETOPS_TLS_BIND` | unset (HTTPS off) | HTTPS listen address, for example `0.0.0.0:9443` |
| `--tls-cert` / `--tls-key` | `FLEETOPS_TLS_CERT` / `FLEETOPS_TLS_KEY` | unset | PEM certificate and key; pass both or neither. When unset, a self-signed certificate is generated and saved in the data directory |
| `--hsts` | `FLEETOPS_HSTS` | off | Send HSTS on HTTPS responses. Enable it only when no client uses the plain HTTP listener, because browsers apply HSTS per host, not per port |
| `--docker-host` | `DOCKER_HOST` | `unix:///var/run/docker.sock` | Docker endpoint for the `local` environment |
| `--secret-key` | `FLEETOPS_SECRET_KEY` | unset (`<data>/secret.key`) | 32 bytes, base64: seals stored kubeconfigs. Back it up with the database |
| `--log-level` | `FLEETOPS_LOG` | `info` | `tracing` filter, for example `fleetops_api=debug,info` |
| `--openapi` | — | — | Print the OpenAPI document to stdout and exit |

Run `fleetops --help` to see the full list.

## Development

```sh
make dev          # API on :9000 plus the Vite dev server on :5173 (proxies /api and the websocket)
```

Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`, which git ignores. It reads `DOCKER_HOST` from the environment, or from the current docker context when the variable is unset.

| Target | What it does |
|---|---|
| `make dev` | Runs `dev-server` and `dev-web` together |
| `make test` | `cargo test --workspace`, which uses mocks and needs no Docker |
| `make test-docker` | Integration tests against the current docker context's daemon (`docker-it` feature); the Docker suite runs a second time through an agent tunnel |
| `make test-web` | Web typecheck, lint, vitest, and a check that the generated API client matches the OpenAPI document |
| `make e2e` | `make build` and the agent, then the Playwright end-to-end tests in Chromium against the current docker context's daemon |
| `make rss-bench` | `make build`, then checks idle RSS stays under 50 MiB with 50 containers, under 50 MiB + 1 MiB per agent with 10 agents, and under 15 MiB per agent (`COUNT`, `LIMIT_MIB`, `AGENTS`, `AGENT_LIMIT_MIB` override). With `KUBE_IT_RSS=target/kube-it`, it also checks the server stays within that limit + 5 MiB with 5 kubeconfig clusters being listed |
| `make test-kube` | Creates or reuses the `kind` cluster `fleetops-test` (`brew install kind`), then runs the Kubernetes suite through a kubeconfig and again through the agent tunnel |
| `make e2e-kube` | The Playwright Kubernetes test against that cluster |
| `make agent-kube-smoke` | Loads `$(AGENT_IMAGE)` into the cluster, runs `$(IMAGE)` on kind's network, applies the generated manifest, and checks that the cluster environment comes up |
| `make lint` | `cargo fmt --check` and `clippy -D warnings` |
| `make fmt` | `cargo fmt --all` |
| `make web` | Builds the UI into `web/dist` |
| `make build` | `make web`, then a release build of the `fleetops` binary |
| `make image` | Builds the `fleetops:dev` container image for linux/amd64 and linux/arm64 (`IMAGE` and `PLATFORMS` override) |
| `make image-agent` | Builds the `fleetops-agent:dev` image (`AGENT_IMAGE` and `PLATFORMS` override) |
| `make image-smoke` | Runs `$(IMAGE)` against the local daemon and checks the UI, the non-root user and Docker access |
| `make agent-smoke` | Runs `$(IMAGE)` and `$(AGENT_IMAGE)` together and checks that the agent enrolls and its environment reaches Docker |
| `make openapi` | Regenerates `crates/fleetops-api/openapi.json` and the TypeScript client after an API change |

### Notes

- **Building without the UI.** `cargo build` works without `web/dist`. The server's `build.rs` writes a placeholder page, and the API still works under `/api/v1`.
- **After changing the API,** run `make openapi` and commit both generated files. `cargo test` and `make test-web` fail until the committed document and client match the code.
- **The integration tests** (`make test-docker`) create and remove their own labelled resources. Run them against a daemon where that's acceptable.
- **The end-to-end test** (`make e2e`) starts the server on `127.0.0.1:9123` with an empty data directory (`web/e2e-data`). It pulls `busybox:1.37`, and creates and then removes containers named `fleetops-e2e-*`; the second spec enrolls a real `fleetops-agent` against the same daemon. It leaves the image in place. On failure, `cd web && pnpm exec playwright show-trace` opens the trace saved under `web/test-results`.
- **CI** (`.gitlab-ci.yml`) runs on pushes to `main` and on merge requests: `make lint test`, `make test-docker`, `make test-web`, `make e2e`, `make rss-bench`, the kind suite (`make test-kube e2e-kube agent-kube-smoke`), and the server and agent image builds for both platforms followed by `make image-smoke` and `make agent-smoke`. Jobs run in `ci/Dockerfile`'s image and start their own Docker daemon (`ci/start-docker.sh`).
- **Releases** (the `publish` and `release` jobs in `.gitlab-ci.yml`) run on a `vX.Y.Z` tag that matches the workspace version in `Cargo.toml`. They push the server and agent images (`docker.io/pravinbanjade/fleetops` and `docker.io/pravinbanjade/fleetops-agent`) to Docker Hub with the `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` CI/CD variables, upload both binaries to the generic package registry and link them from a GitLab release.
- **TDD.** New behaviour starts with a failing test.

### Design documents

Design documents live in `docs/superpowers/`:

- `specs/2026-09-29-fleetops-core-docker-design.md` is the sub-project 1 design and the authority for behaviour.
- `specs/2026-10-01-fleetops-agent-multihost-design.md` is the sub-project 2 (agent) design.
- `specs/2026-10-01-fleetops-kubernetes-design.md` is the sub-project 3 (Kubernetes) design.
- `specs/2026-10-06-fleetops-kube-live-writable-design.md` is slice 3b (live and writable Kubernetes).
- `plans/` holds the implementation plans: one per phase of sub-project 1 (P1, P2a–c, P3a–d), one for sub-project 2, and one for sub-project 3.

## Architecture

```
fleetops/
├─ crates/
│  ├─ fleetops-server/   bin `fleetops`: CLI and config, startup, TLS, maintenance jobs, embeds web/dist
│  ├─ fleetops-api/      axum router, handlers, auth and CSRF, authz, audit, websocket hub, OpenAPI (utoipa)
│  ├─ fleetops-core/     domain types, the Environment and DockerOps traits, errors, recreate planning; no I/O
│  ├─ fleetops-docker/   bollard-backed implementation of the core traits (LocalDocker, AgentDockerEnv)
│  ├─ fleetops-tunnel/   yamux over a WebSocket, the agent control protocol, the live-session registry
│  ├─ fleetops-agent/    bin `fleetops-agent`: dial-out, key storage, byte pipe to the host's Docker socket
│  └─ fleetops-store/    sqlx + SQLite: migrations and repositories (users, sessions, environments, settings, audit)
├─ web/                  React 19 + TypeScript + Vite
│  └─ src/api/gen/       client generated from the OpenAPI document
└─ docs/superpowers/     specs and plans
```

- **Environments.** Every Docker route is scoped to an environment (`/envs/{env}/…`). Handlers resolve the environment through an `EnvRegistry` and call the `DockerOps` trait. They never call bollard directly. Local environments use `LocalDocker` on a socket; agent environments (`docker-agent`) use the same `LocalDocker` over a tunnel. Kubernetes environments (`kube-config`, `kube-agent`) implement `KubeOps` instead, on kube-rs, either from a stored kubeconfig or over the same tunnel to an in-cluster agent; their routes live under `/envs/{env}/k8s/…`, and pod logs and exec use the same websocket topics as containers.
- **Request pipeline.** Each request passes through: session auth → CSRF check (mutations only) → resolve the environment → `authorize(user, env, action)` → Docker operation → audit entry → response. `authorize` lives in `fleetops-api::authz`, so fine-grained RBAC (sub-project 5) can replace its body without touching handlers.
- **Agent tunnel.** Each agent opens one WebSocket to `/api/v1/agent/connect` and runs a yamux session over it. The agent's first stream carries a small JSON control protocol (hello, welcome, ping, bye). Every other stream is opened by the server and is a raw connection to the host's Docker socket: bollard runs over a custom transport whose connector opens a stream, so hijacked exec and streamed logs need nothing special. Each stream has its own 256 KiB flow-control window, so a slow reader stalls only its own stream.
- **Real time.** A per-environment hub shares one Docker events stream among all browser tabs. Stats streams are reference-counted per container.
- **Backpressure.** Each websocket subscription has a bounded queue. Events and stats drop the oldest items when the queue is full. Logs drop lines and send a `gap` message. Exec applies true backpressure.
- **Web stack.** TanStack Router (lazy routes), React Query, TanStack Table and Virtual, Tailwind with shadcn/ui, cmdk, xterm.js and uPlot. Docker events patch the query cache directly. xterm.js and uPlot load only when their tab opens.

## API

The REST API lives under `/api/v1` and uses JSON. The OpenAPI document is served at `/api/v1/openapi.json`, and a copy is committed at `crates/fleetops-api/openapi.json`.

| Area | Endpoints |
|---|---|
| Auth | `POST /auth/setup`, `/auth/login`, `/auth/logout`, `/auth/password` · `GET /auth/me`, `/auth/status` · `GET`/`DELETE /auth/sessions[/{id}]` |
| Admin | `/users`, `/envs` (mutations are admin-only), `POST /envs/{env}/agent/rotate`, `GET /audit` |
| Agents | `GET /agent/connect`: the agent tunnel, a WebSocket upgrade with `Authorization: Bearer fj_…` (join token) or `fk_…` (agent key); not for browsers |
| Docker | `/envs/{env}/{info,df,containers,images,volumes,networks,compose-projects}/…` |
| Tasks | `GET /tasks/{id}` for long-running image pulls and builds, which return `202 {task_id}` |
| Health | `GET /health` |

Errors use the shape `{code, message, details?, request_id}`. Every response carries an `X-Request-Id` header. Docker's own error messages are passed through.

### Websocket

The websocket is at `/api/v1/ws`. Each browser tab opens one connection, authenticated by the session cookie and checked against the Origin header.

- **Subscribing.** Clients send `{op:"sub", sub, topic, env, id?, opts?}`. The topics are `events`, `stats`, `logs`, `exec`, `task` and `kube-watch` (a live list of Kubernetes objects: a snapshot, then added, modified and deleted rows; `id` is `<kind>/<namespace or ->`).
- **Server messages.** The server replies with `data`, `gap`, `end` and `error` frames. It also sends `env_status` when an environment goes up or down.
- **Exec.** Terminal input and output use binary frames: `[u32 sub id, big-endian][payload]`.

The full protocol is in section 5.2 of the spec.

## Security

- **Authentication.** All Docker routes need a signed-in session, and mutations also need the `X-CSRF-Token` header.
- **Cookies.** Session cookies are `HttpOnly` and `SameSite=Strict`, and `Secure` over TLS.
- **Session expiry.** Sessions expire after 7 days idle or 30 days in total.
- **Headers.** Responses send a strict CSP, `frame-ancestors 'none'`, `X-Content-Type-Options` and `Referrer-Policy`. They also send HSTS when you enable it.
- **Secret masking is display-only.** The API returns values unredacted, and anyone with access can reveal them.
- **Agents.** Join tokens last 15 minutes and work once; the server stores only SHA-256 hashes of tokens and keys. Agent credentials open the tunnel and nothing else. Failed agent connects are rate-limited per address. Agents must use HTTPS unless they connect from the same machine.
- **Trust.** A compromised server is root on every agent host, as it already is on its local socket. Treat the server like the hosts it manages.
- **Reverse proxies.** Agents need TLS all the way to FleetOps: the server does not trust `X-Forwarded-Proto`, so a proxy that terminates TLS and forwards plain HTTP gets `403` for agents unless it runs on the same machine (a loopback peer). Either re-encrypt to `--tls-bind`, or pass TLS through (TCP mode). Pass WebSocket upgrades through to `/api/v1/agent/connect`, and keep idle timeouts above 30 s (the server pings agents every 15 s).
- **No telemetry.** FleetOps does not phone home.
- **Container image.** It runs as a non-root user on `distroless/static`, with no shell. Docker access comes only from the socket group you grant with `--group-add`.

## Roadmap

| # | Sub-project | Summary |
|---|---|---|
| **1** | **Core + local Docker** (done) | Server, embedded UI, auth, environments, full Docker resource management, real-time channel |
| **2** | **Agent + multi-host** (done) | Rust agent that dials out to the server over a multiplexed websocket tunnel |
| **3** | **Kubernetes** (slices 3 and 3b done) | Clusters through kubeconfig or an in-cluster agent: workloads, services, ingress, config and secrets, logs and exec; live watch streams and YAML create, edit and apply (3b) |
| **3c** | **RBAC** (done) | Per-environment viewer and operator roles for Docker and Kubernetes environments, with namespace scoping inside clusters |
| 3d | Helm | Releases in Kubernetes environments |
| 4 | Stacks + GitOps | Compose authoring, diff before deploy, rollback, git-backed stacks with webhook or poll sync |
| 5 | Teams & enterprise (free) | Teams, OIDC/SSO, LDAP, API tokens, backup and restore |

Progress on sub-project 1:

- **Done:**
  - P1, backend foundation
  - P2a, Docker environments and resources
  - P2b, websocket hub and tasks
  - P2c, OpenAPI
  - P3a, web foundation
  - P3b, resource and admin pages
  - P3c, logs, terminal and stats
  - P3d, Playwright e2e, container image and CI
