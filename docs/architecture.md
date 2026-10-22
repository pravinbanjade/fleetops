# Architecture

```
fleetops/
├─ crates/
│  ├─ fleetops-server/   bin `fleetops`: CLI and config, startup, TLS, maintenance jobs, embeds web/dist
│  ├─ fleetops-api/      axum router, handlers, auth and CSRF, authz, audit, websocket hub, OpenAPI (utoipa)
│  ├─ fleetops-core/     domain types, the Environment and DockerOps traits, errors, recreate planning, Compose parsing and stack plans; no I/O
│  ├─ fleetops-docker/   bollard-backed implementation of the core traits (LocalDocker, AgentDockerEnv)
│  ├─ fleetops-kube/     kube-rs implementation of KubeOps: lists, watches, apply, Helm, metrics
│  ├─ fleetops-git/      gitoxide: a shallow fetch of one branch, reads of files and tree ids at a commit, build-context tars and fetching one commit, for git-backed stacks
│  ├─ fleetops-tunnel/   yamux over a WebSocket, the agent control protocol, the live-session registry
│  ├─ fleetops-agent/    bin `fleetops-agent`: dial-out, key storage, byte pipe to the host's Docker socket
│  └─ fleetops-store/    sqlx + SQLite: migrations and repositories (users, sessions, environments, settings, audit), backup files and restores
├─ web/                  React 19 + TypeScript + Vite
│  └─ src/api/gen/       client generated from the OpenAPI document
└─ ee/                   Enterprise edition (source-available): fleetops-ee crate, license tool, UI
```

- **Environments.** Every Docker route is scoped to an environment (`/envs/{env}/…`). Handlers resolve the environment through an `EnvRegistry` and call the `DockerOps` trait. They never call bollard directly. Local environments use `LocalDocker` on a socket; agent environments (`docker-agent`) use the same `LocalDocker` over a tunnel. Kubernetes environments (`kube-config`, `kube-agent`) implement `KubeOps` instead, on kube-rs, either from a stored kubeconfig or over the same tunnel to an in-cluster agent; their routes live under `/envs/{env}/k8s/…`, and pod logs and exec use the same websocket topics as containers.
- **Request pipeline.** Each request passes through: session auth → CSRF check (mutations only) → resolve the environment → `authorize(user, env, action)` → Docker operation → audit entry → response. Authorization lives in one place, `fleetops-api::authz`, which checks the user's role on the environment and, on Kubernetes, the namespace.
- **Agent tunnel.** Each agent opens one WebSocket to `/api/v1/agent/connect` and runs a yamux session over it. The agent's first stream carries a small JSON control protocol (hello, welcome, ping, bye). Every other stream is opened by the server and is a raw connection to the host's Docker socket: bollard runs over a custom transport whose connector opens a stream, so hijacked exec and streamed logs need nothing special. Each stream has its own 256 KiB flow-control window, so a slow reader stalls only its own stream.
- **Stacks.** `fleetops-core::compose` parses a Compose file into a normalised project, hashes each service's config, and plans it against the stack's live containers (found by the `com.docker.compose.project` label). The deploy runner in `fleetops-api::stacks` applies the plan through `DockerOps` as a background task. Replaced containers are stopped and renamed `<name>-fleetops-old-<revision>-<run|stop>` and deleted only once every service is up. A failure, or a deploy cut off by a restart, is undone from those names and the `io.fleetops.stack.revision` label alone. Builds use `fleetops-core::compose::build`: a service's image tag is `<repo>:<keyed hash>` of its context's git tree id and build settings, so code changes recreate exactly the services that build it; the runner builds missing images (a tar of the context from the repository cache, `.dockerignore` applied) before touching containers, records what each revision built for rollback, and removes the stack's images that neither the current nor the previous deploy uses. Secrets and configs are resolved by `fleetops-core::compose::files`: each one's content goes into the service's config hash as an HMAC keyed from the server key (so the hash label reveals nothing), and the runner uploads them as one tar (`PUT /containers/{id}/archive`) between creating a container and starting it. The values live sealed in `stack_files`, and every revision keeps what it deployed in `stack_revision_files`, which a rollback uses. A git-backed stack is synced by `fleetops-api::stacks::git`: `fleetops-git` fetches the branch at depth 1 into `<data>/git/<stack id>`, the file and the config files it names (read at the same commit) go into the stack's draft, and it is deployed or held as pending. Syncs come from the API, a 30 s polling loop or the webhook route, and at most one runs per stack.
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
| Stacks | `/envs/{env}/stacks[/{stack}[/draft,plan,deploy,revisions[/{n}],{start,stop,restart}]]`; a deploy returns `202 {task_id, revision}` |
| Stack secrets and configs | `/envs/{env}/stacks/{stack}/files[/{secret,config}/{name}]`: list, set (write-only for secrets) and remove |
| Stack git sources | `/envs/{env}/stacks/{stack}/git[/sync,resume,secret]` · `POST /stacks/hooks/{id}`: push webhooks, verified by the stack's secret, with no session |
| Kubernetes | `/envs/{env}/k8s/…` |
| Tasks | `GET /tasks/{id}` for long-running image pulls and builds, which return `202 {task_id}` |
| Health | `GET /health` |

Errors use the shape `{code, message, details?, request_id}`. Every response carries an `X-Request-Id` header. Docker's own error messages are passed through.

### Websocket

The websocket is at `/api/v1/ws`. Each browser tab opens one connection, authenticated by the session cookie and checked against the Origin header.

- **Subscribing.** Clients send `{op:"sub", sub, topic, env, id?, opts?}`. The topics are `events`, `stats`, `logs`, `exec`, `task` and `kube-watch` (a live list of Kubernetes objects: a snapshot, then added, modified and deleted rows; `id` is `<kind>/<namespace or ->`).
- **Server messages.** The server replies with `data`, `gap`, `end` and `error` frames. It also sends `env_status` when an environment goes up or down.
- **Exec.** Terminal input and output use binary frames: `[u32 sub id, big-endian][payload]`.
