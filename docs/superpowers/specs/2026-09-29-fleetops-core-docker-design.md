# FleetOps — Sub-project 1: Core + Local Docker

- **Date:** 2026-09-29
- **Status:** Draft for review
- **Reference:** `./portainer` (Portainer CE, Go + Angular/React) — used for feature reference only; no code is copied.

## 1. Intent

FleetOps is an open-source container management platform intended to compete with Portainer. It differentiates on:

1. **Lightweight single binary** — Rust server with the web UI embedded, SQLite embedded, low RAM/CPU, fast startup.
2. **No paywalled features** — RBAC, SSO, GitOps, audit are free (delivered across sub-projects).
3. **Superior real-time UX** — event-driven live UI, first-class log viewer, terminal, live stats.
4. **Compose/GitOps-first** — delivered in sub-project 3; this sub-project lays the groundwork.

### Roadmap (each sub-project gets its own spec → plan → implementation)

| # | Sub-project | Summary |
|---|---|---|
| **1** | **Core + local Docker (this spec)** | Server, embedded UI, auth, environment abstraction, full Docker resource management on the local socket, real-time channel |
| 2 | Agent + multi-host | Rust agent dials out to the server over a multiplexed websocket tunnel |
| 3 | Stacks + GitOps | Compose authoring, diff-before-deploy, rollback, git-backed stacks with webhook/poll sync |
| 4 | Kubernetes | Clusters via kubeconfig or in-cluster agent; workloads, services, ingress, config/secrets, logs/exec, Helm |
| 5 | Teams & enterprise (free) | Per-environment RBAC, OIDC/SSO, LDAP, audit UI, API tokens, backup/restore |

### Success criteria for sub-project 1

- One `fleetops` binary (and a ~20 MB image) that, pointed at a Docker or Podman socket, lets an admin set up an account, log in, and manage containers, images, volumes, and networks entirely from the browser.
- Lists and detail views update live from Docker events with no manual refresh.
- Log viewer stays responsive at 100k+ lines; terminal supports resize and shell auto-detect.
- The `Environment` abstraction, `authorize()` hook, and `role_bindings` schema exist so sub-projects 2, 4, and 5 extend rather than rewrite.
- Idle server RSS under 50 MB against a host with ~50 containers.

## 2. Architecture

### 2.1 Repository layout

```
fleetops/
├─ Cargo.toml                 workspace
├─ crates/
│  ├─ fleetops-server/        bin: CLI/config, startup, TLS, embeds web/dist (rust-embed)
│  ├─ fleetops-core/          domain types, traits, Error; no I/O dependencies
│  ├─ fleetops-docker/        bollard-backed implementation of core traits
│  ├─ fleetops-store/         sqlx + SQLite: migrations and repositories
│  └─ fleetops-api/           axum router, handlers, auth middleware, WS hub, OpenAPI (utoipa)
├─ web/                       React + TypeScript + Vite
│  └─ src/api/                client generated from the OpenAPI spec
├─ Makefile
└─ .github/workflows/
```

`portainer/` is a reference checkout and is git-ignored.

### 2.2 Environment abstraction

`fleetops-core` defines:

```rust
pub enum EnvKind { DockerLocal /* later: DockerAgent, Kubernetes */ }

#[async_trait]
pub trait Environment: Send + Sync {
    fn id(&self) -> &EnvId;
    fn kind(&self) -> EnvKind;
    async fn status(&self) -> EnvStatus;            // Up { version, .. } | Down { reason }
    fn docker(&self) -> Option<&dyn DockerOps>;     // Some for Docker kinds
    // sub-project 4 adds: fn kube(&self) -> Option<&dyn KubeOps>;
}

#[async_trait]
pub trait DockerOps: Send + Sync {
    // containers, images, volumes, networks, system, exec, logs, stats, events
    // each method returns core domain types, never bollard types
}
```

- Sub-project 1 ships one implementation: `LocalDocker`, which connects via a unix socket (default `/var/run/docker.sock`), a Podman socket, or `DOCKER_HOST`.
- An `EnvRegistry` loads environments from SQLite. On first start it auto-registers `local` when it finds a reachable socket. Handlers resolve `{env}` through the registry and never touch bollard directly.
- UI routes are environment-scoped from day one: `/envs/:env/...`.

### 2.3 Runtime

- `fleetops` listens on `:9000` (HTTP) and optionally on `:9443` (HTTPS via rustls). If TLS is enabled without a cert/key, it generates a self-signed cert on first run and stores it in the data dir.
- Data dir (default `/data`) holds `fleetops.db` (SQLite, WAL mode) and TLS material.
- Config comes from CLI flags or env vars. There is no config file.

| Flag | Env | Default |
|---|---|---|
| `--data` | `FLEETOPS_DATA` | `/data` |
| `--bind` | `FLEETOPS_BIND` | `0.0.0.0:9000` |
| `--tls-bind` | `FLEETOPS_TLS_BIND` | unset (disabled) |
| `--tls-cert` / `--tls-key` | `FLEETOPS_TLS_CERT` / `FLEETOPS_TLS_KEY` | unset → self-signed |
| `--docker-host` | `DOCKER_HOST` | `unix:///var/run/docker.sock` |
| `--log-level` | `FLEETOPS_LOG` | `info` |

### 2.4 Tech stack

- **Backend:** tokio, axum, bollard, sqlx (SQLite, embedded migrations), argon2, rustls/axum-server, rust-embed, utoipa, tracing, thiserror.
- **Frontend:** React 19, TypeScript, Vite, TanStack Router/Query/Table/Virtual, Tailwind + shadcn/ui, xterm.js, CodeMirror 6, uPlot for sparklines/charts, openapi-ts for the generated client.

## 3. Data model (SQLite)

```
users(id, username UNIQUE, password_hash, role, created_at, updated_at)
sessions(id, user_id, token_hash UNIQUE, csrf_token, created_at, expires_at, last_seen_at, user_agent, ip)
environments(id, name UNIQUE, kind, url, created_at, updated_at)
role_bindings(user_id, environment_id NULLABLE, role)   -- v1: global binding mirrors users.role
settings(key PRIMARY KEY, value JSON)
audit_log(id, ts, user_id, environment_id, action, target_type, target_id, result, detail JSON)
```

- `role` values in v1 are `admin` and `user`. `user` has full access to Docker resources but cannot manage users, environments, or settings.
- `audit_log` rows are kept for 90 days by default (configurable through `settings`) and pruned daily.

## 4. Features

### 4.1 Auth and users

- **First-run setup:** when no users exist, every route redirects to `/setup`, which creates the admin. The setup endpoint refuses once any user exists. If no admin is created within 10 minutes of start, setup is locked until restart, matching Portainer's protection against a stranger claiming an exposed instance.
- **Login:** username + password. Passwords are hashed with argon2id.
- **Sessions:** a random 256-bit token is sent as an `HttpOnly; Secure (when TLS); SameSite=Strict` cookie, and only its SHA-256 is stored. Sessions expire after 7 days idle or 30 days absolute. Users can revoke sessions from their profile page.
- **CSRF:** mutating requests must send an `X-CSRF-Token` header matching the session's token, which the client gets from `/api/v1/auth/me`.
- **Rate limiting:** login is limited to 5 failures per username+IP per 5 minutes, then backs off.
- **Admin:** user CRUD (create, reset password, change role, delete) and environment CRUD (local socket kinds only in v1).
- **Audit:** every mutating Docker and admin action writes an `audit_log` row. v1 includes a simple admin-only audit list page.

### 4.2 Docker resources (per environment)

**Dashboard**
- Counts: containers by state, images, volumes, networks.
- Disk usage from `system df`.
- Host info: engine version, OS, CPUs, memory.
- Live recent-events feed.

**Containers**
- List: name, image, state, status, ports (clickable), created, compose project, live CPU/mem sparklines. Filter/search, column visibility, bulk start/stop/restart/kill/pause/unpause/remove.
- Detail page tabs:
  - **Overview:** state, ports, restart policy, labels.
  - **Inspect:** raw JSON viewer.
  - **Env:** secret-looking values masked.
  - **Mounts** and **Networks**.
  - **Stats:** live CPU/mem/net/IO charts.
  - **Logs:** see 4.3.
  - **Terminal:** see 4.3.
  - **Files:** read-only browsing via the archive API, with single-file download.
- Create form: image, name, command/entrypoint, ports, env, volumes/binds, network, restart policy, labels, resource limits, capabilities/privileged, and an "advanced" JSON override.
- **Recreate with changes:** opens the create form pre-filled from inspect, then stops, renames, creates, starts, and removes the old container. If the new container fails to start, it rolls back to the old one.
- **Duplicate:** the same pre-filled form without removing the original.

**Images**
- List (tags, size, created, in-use flag), pull with streamed layer progress, remove (force option), prune (dangling / unused), tag, inspect, history.
- Build from an uploaded Dockerfile or tar context, with a streamed build log.

**Volumes**
- List (driver, mountpoint, in-use by), create (driver + options), remove, prune, inspect.

**Networks**
- List, create (driver, subnet/gateway, internal, attachable), remove, prune, inspect, connect/disconnect container.

**Compose projects (read-only)**
- Containers grouped by the `com.docker.compose.project` label.
- Per project: start, stop, restart, and "down" (remove containers). Authoring comes in sub-project 3.

**Global UX**
- Command palette (⌘K / Ctrl-K): jump to any container, image, volume, or network, or run an action on one.
- Keyboard shortcuts, dark/light/system theme, toast notifications for task results.

### 4.3 Real-time UX

- **Live lists:** the UI subscribes to `events` and applies each event to the TanStack Query cache, patching or invalidating the affected resource. There is no polling.
- **Log viewer:**
  - Virtualized rendering, capped at a 200k-line ring buffer in the client.
  - Follow mode with auto-pause on scroll-up.
  - Regex/text search with match highlighting and next/prev navigation.
  - stdout/stderr toggle, timestamps toggle, `since`/`tail` controls, ANSI color rendering, wrap toggle.
  - Download of the visible or full range.
- **Terminal:**
  - xterm.js with a fit addon and resize propagated to Docker exec.
  - Shell auto-detect: try `bash`, fall back to `sh`, with a manual command override and an optional user override.
  - Copy/paste support; the session ends cleanly when the process exits.
- **Stats:** the list page subscribes to aggregated stats for visible containers only. The detail page subscribes to one container.

### 4.4 Out of scope for sub-project 1

Registries and registry credentials, app templates, Swarm, the agent/remote environments, stack editing/GitOps, Kubernetes, SSO/LDAP, fine-grained RBAC, API tokens, backup/restore, i18n.

## 5. API

### 5.1 REST

Base path `/api/v1`, JSON bodies, OpenAPI served at `/api/v1/openapi.json`.

```
POST   /auth/setup | /auth/login | /auth/logout      GET /auth/me
GET    /auth/sessions                                  DELETE /auth/sessions/{id}
CRUD   /users            (admin)
CRUD   /envs             (admin for mutations)          GET /envs/{env}/status
GET    /audit            (admin)

/envs/{env}/
  GET    info | df
  GET    containers                      POST containers               (create)
  GET    containers/{id}                 DELETE containers/{id}?force&volumes
  POST   containers/{id}/{start|stop|restart|kill|pause|unpause|rename}
  POST   containers/{id}/recreate        POST containers/{id}/duplicate
  GET    containers/{id}/files?path=     GET containers/{id}/files/download?path=
  GET    images                          POST images/pull              → 202 {task_id}
  POST   images/build (multipart)        → 202 {task_id}
  GET    images/{id} | images/{id}/history
  POST   images/{id}/tag                 DELETE images/{id}?force
  POST   images/prune | volumes/prune | networks/prune | containers/prune
  GET    volumes | volumes/{name}        POST volumes     DELETE volumes/{name}
  GET    networks | networks/{id}        POST networks    DELETE networks/{id}
  POST   networks/{id}/{connect|disconnect}
  GET    compose-projects                POST compose-projects/{name}/{start|stop|restart|down}
```

**Handler pipeline:**

```
session auth → CSRF check (mutations) → resolve env → authorize(user, env, action) → env.docker()?.op() → audit → response
```

`authorize` lives in one function in `fleetops-api::authz`, so sub-project 5 replaces its body without touching handlers.

### 5.2 Websocket: `/api/v1/ws`

One connection per browser tab, authenticated by the session cookie. The server checks the Origin header against its own host. Frames are JSON text; exec I/O uses binary frames.

```
client → {op:"sub",   sub:<u32>, topic:"events"|"stats"|"logs"|"exec"|"task", env, id?, opts?}
client → {op:"unsub", sub}
client → {op:"input", sub, data}                (exec stdin, text fallback)
client → {op:"resize", sub, cols, rows}         (exec)
server → {sub, type:"data",  data}
server → {sub, type:"gap",   dropped:<n>}       (logs overflow)
server → {sub, type:"end",   reason?}
server → {sub, type:"error", code, message}
server → {type:"env_status", env, status}
binary frame: [u32 sub id BE][payload]          (exec stdin/stdout)
```

- **Batching:** logs and stats data are flushed at most every 50 ms per subscription.
- **Backpressure:** each subscription has a bounded channel (default 1024 messages). On overflow, `events` and `stats` drop the oldest item, and `logs` drops lines and emits `gap`. `exec` applies true backpressure (pausing reads from Docker). A slow client never blocks other subscriptions or other clients.
- **Fan-out:** a per-environment `EventHub` runs one Docker events stream into a `tokio::sync::broadcast` channel. Stats streams are ref-counted per container and shared across all subscribers, and are dropped when the last subscriber leaves.
- **Authorization:** each `sub` is checked with `authorize(user, env, action)`, the same as REST. Exec is audited.
- **Resilience:** when a Docker stream drops, the server reconnects with exponential backoff (up to 30 s) and broadcasts `env_status`. The web client reconnects the websocket with backoff, resubscribes all active subs, and refetches affected queries.

## 6. Error handling

`fleetops_core::Error` variants map to HTTP status codes:

| Variant | HTTP |
|---|---|
| `NotFound` | 404 |
| `Conflict` | 409 |
| `Invalid` | 400 / 422 |
| `Unauthorized` | 401 |
| `Forbidden` | 403 |
| `EnvUnavailable` | 503 |
| `Upstream { status, message }` (Docker's message preserved) | mapped from the Docker status |
| `Internal` | 500 |

- Response body: `{code, message, details?, request_id}`.
- Every request carries a `request_id` in logs (`tracing` span) and in the `X-Request-Id` header. `Internal` errors are logged with full context and never leak internals to the client.
- The UI shows Docker's own error message in toasts or inline, together with the request id for `Internal` errors.

## 7. Security

- Access to the Docker socket is root-equivalent, so all Docker routes require an authenticated session. Mutations also require CSRF.
- Headers: strict CSP (self-only scripts), `X-Content-Type-Options`, `Referrer-Policy`, `frame-ancestors 'none'`, and HSTS when TLS is enabled.
- Login rate limiting and first-run setup lockout (4.1).
- Secret masking: env keys matching `*PASS*|*SECRET*|*TOKEN*|*KEY*` are masked in the UI and revealed on click. The server does not redact values; masking is a UX guard only.
- No telemetry, no phone-home.
- Container image: distroless/static, runs as non-root. Access to the socket is granted through group membership, and the docs explain the trade-off.

## 8. Testing

| Layer | Approach |
|---|---|
| `fleetops-core` | Unit tests for domain logic (e.g. recreate plan, secret-key matcher) |
| `fleetops-store` | sqlx tests on in-memory SQLite, with migrations applied |
| `fleetops-api` | axum `oneshot` tests against a `MockDockerOps`: auth, CSRF, authorization, error mapping, audit writes; WS protocol tests (sub/unsub, gap on overflow, resubscribe) |
| `fleetops-docker` | Integration tests against a real daemon behind the `docker-it` cargo feature; run in CI with a Docker service |
| `web` | Vitest + React Testing Library for components and the event→cache reducer |
| e2e | Playwright: setup → login → pull image → run container → live list update → logs search → exec → remove |

TDD is used throughout implementation.

## 9. Build and delivery

- `make dev`: `cargo watch -x run` plus the Vite dev server on `:5173`, proxying `/api` (including websockets) to `:9000`.
- `make build`: `pnpm -C web build`, then `cargo build --release` with `web/dist` embedded.
- `make image`: multi-arch (linux/amd64, linux/arm64) static musl binary via `cargo-zigbuild`, packaged in a distroless/static image.
- CI (GitHub Actions): `cargo fmt --check`, `clippy -D warnings`, `cargo test`, `docker-it` tests, `pnpm typecheck lint test`, Playwright e2e, image build.
- The OpenAPI spec is generated at build time. `pnpm gen:api` regenerates the TS client, and CI fails if it drifts.

## 10. Open decisions (resolved)

- Typed domain API rather than a Docker passthrough proxy (chosen for RBAC/audit hooks and Kubernetes reuse).
- Opaque session cookies rather than JWT (revocable, simpler, no client token storage). API tokens arrive in sub-project 5.
- Podman is supported through its Docker-compatible socket using the same `LocalDocker` implementation. Podman-specific features are out of scope.
