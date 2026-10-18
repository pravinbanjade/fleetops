# Architecture

```
fleetops/
├─ crates/
│  ├─ fleetops-server/   bin `fleetops`: CLI and config, startup, TLS, maintenance jobs, embeds web/dist
│  ├─ fleetops-api/      axum router, handlers, auth and CSRF, authz, audit, websocket hub, OpenAPI (utoipa)
│  ├─ fleetops-core/     domain types, the Environment and DockerOps traits, errors, recreate planning; no I/O
│  ├─ fleetops-docker/   bollard-backed implementation of the core traits (LocalDocker, AgentDockerEnv)
│  ├─ fleetops-kube/     kube-rs implementation of KubeOps: lists, watches, apply, Helm, metrics
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
| Kubernetes | `/envs/{env}/k8s/…` |
| Tasks | `GET /tasks/{id}` for long-running image pulls and builds, which return `202 {task_id}` |
| Health | `GET /health` |

Errors use the shape `{code, message, details?, request_id}`. Every response carries an `X-Request-Id` header. Docker's own error messages are passed through.

### Websocket

The websocket is at `/api/v1/ws`. Each browser tab opens one connection, authenticated by the session cookie and checked against the Origin header.

- **Subscribing.** Clients send `{op:"sub", sub, topic, env, id?, opts?}`. The topics are `events`, `stats`, `logs`, `exec`, `task` and `kube-watch` (a live list of Kubernetes objects: a snapshot, then added, modified and deleted rows; `id` is `<kind>/<namespace or ->`).
- **Server messages.** The server replies with `data`, `gap`, `end` and `error` frames. It also sends `env_status` when an environment goes up or down.
- **Exec.** Terminal input and output use binary frames: `[u32 sub id, big-endian][payload]`.
