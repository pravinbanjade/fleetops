# FleetOps — Sub-project 2: Agent + Multi-host

- **Date:** 2026-10-01
- **Status:** Draft for review
- **Builds on:** [Sub-project 1 spec](./2026-09-29-fleetops-core-docker-design.md)

## 1. Intent

One FleetOps server manages Docker on many hosts. Each remote host runs a small Rust agent that **dials out** to the server, so the host needs no inbound port and its Docker socket is never exposed on the network. An agent host appears as an ordinary environment: every page from sub-project 1 — lists, live events, logs, terminal, stats, files, image build — works on it unchanged.

### Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Topologies | Dial-out agent only. No direct `tcp://`+TLS remotes, no listening agent. |
| Scale | Up to ~50 agents per server. One process, all sessions in memory, no clustering. |
| Trust | One-time join token, exchanged on first connect for a long-lived per-agent key. Hashes only on the server. |
| What crosses the tunnel | Raw Docker API bytes. The server's bollard client dials through the tunnel; the agent is a pipe to its socket. A small typed control stream carries hello, version and heartbeat. |

### Success criteria

- An admin adds an agent environment in the UI, copies one `docker run` command to a remote host, and sees the environment come online live, within 5 s of the agent starting.
- The full sub-project 1 Docker integration suite passes through a tunnel, including hijacked exec.
- Killing an agent turns its environment offline in the UI within 45 s; restarting it brings it back and the event-driven caches catch up without a page reload.
- Agent idle RSS ≤ 15 MiB. Server idle RSS ≤ 50 MiB + 1 MiB per connected agent (measured at 10 agents).
- Agent and server of the same protocol major version interoperate; a mismatch is reported in the environment's status, never as a silent failure.

## 2. Architecture

### 2.1 Layout changes

```
crates/
├─ fleetops-tunnel/   NEW  yamux session over a WebSocket; control-stream codec; protocol version. No Docker code.
├─ fleetops-agent/    NEW  bin `fleetops-agent`: dial-out loop, credential file, stream → Docker socket pipe
├─ fleetops-core/          EnvKind::DockerAgent
├─ fleetops-docker/        LocalDocker::with_client; AgentDockerEnv (bollard over a custom transport)
├─ fleetops-api/           AgentHub; GET /api/v1/agent/connect; agent enrollment endpoints
├─ fleetops-store/         migration 0002: agent_credentials
└─ fleetops-server/        wires AgentHub into the env factory
web/                       "Add agent environment" dialog, online/offline badges, rotate key
```

### 2.2 `fleetops-tunnel`

- **Transport.** One WebSocket per agent, binary frames only, adapted to `AsyncRead + AsyncWrite` and carrying a yamux session (`yamux` crate). The **agent is the yamux client** (it dialled), but **only the server opens data streams**; the agent opens exactly one stream, the control stream, immediately after connecting.
- **Control stream.** Length-prefixed (u32 BE) JSON messages:

  | Direction | Message |
  |---|---|
  | agent → server | `Hello { proto: u16, agent_version, host: { hostname, os, arch, docker_socket } }` |
  | server → agent | `Welcome { env_name, server_version, agent_key: Option<String> }` |
  | both | `Ping { n }` / `Pong { n }` |
  | both | `Bye { reason: revoked \| replaced \| incompatible \| shutdown }` |

- **Versioning.** `PROTO_MAJOR` is a `u16` constant. The server accepts an agent whose `proto` equals its own major; anything else gets `Bye { incompatible }` and the reason is recorded on the environment. Additive control fields are allowed within a major (unknown fields ignored).
- **Limits.** 256 concurrent streams per session; WebSocket max frame 16 MiB; yamux per-stream receive window 256 KiB, which gives each stream true backpressure (a slow reader stalls only its own writer).

### 2.3 `fleetops-agent`

- **Config** (flags with `FLEETOPS_AGENT_*` env equivalents): `--server <https://…>`, `--join-token` (used only when no key is stored), `--data` (default `/data`, holds `agent.key`, mode 0600), `--docker` (default `unix:///var/run/docker.sock`), `--log`.
- **Dial loop.** Connect to `<server>/api/v1/agent/connect` with `Authorization: Bearer <fk_key | fj_token>`; on failure back off 1 s doubling to 30 s with ±20 % jitter; reset after a session has stayed up 60 s. `Bye { revoked }` or HTTP 401 with a stored key exits non-zero with a clear message instead of retrying forever.
- **Data streams.** Each inbound yamux stream gets a fresh connection to the Docker socket and a bidirectional copy until either side closes. The agent does not parse HTTP. Upgrades (exec, attach) work because they are just bytes on the same connection.
- Target idle RSS ≤ 15 MiB; static musl binary on distroless, like the server.

### 2.4 Server side

- **`EnvKind::DockerAgent`** (`"docker-agent"`). The environment's `url` is `agent://<env-id>`; it is created by the server, never typed by a user, and `validate_env_url` accepts `agent://` only from the enrollment path.
- **`AgentHub`** (`fleetops-api`) holds `env_id → Session { yamux control handle, opened_at, agent_version, host }`. At most one session per environment: a new authenticated connection sends `Bye { replaced }` to the old one and takes its place. It exposes `open_stream(env_id) -> Result<yamux::Stream>` and publishes `env_status` (the message already in the WS protocol) on every connect and disconnect.
- **`AgentDockerEnv`** (`fleetops-docker`) implements `Environment`:
  - `status()` — `Up { version }` from a ping through the tunnel when a session exists; otherwise `Down { reason }` such as `"agent offline since 14:02"` or `"agent v0.3 uses protocol 2; server needs 1"`.
  - `docker()` — `None` with no session; otherwise a `LocalDocker` built with `LocalDocker::with_client(Docker::connect_with_custom_transport(…))`. The transport owns a hyper-util client whose connector calls `AgentHub::open_stream`. Idle pooled streams close after 30 s.
  - Because the result is a `LocalDocker`, every `DockerOps` and `StreamOps` method — and every handler — is reused unchanged.
- **Events hub.** The per-environment events subscription stops when the session ends. On reconnect it resubscribes and replays from the last event time it delivered, using the existing `/events` replay logic, so clients' caches converge without a reload.

## 3. Data model

Migration `0002_agents.sql`:

```sql
CREATE TABLE agent_credentials (
    environment_id   INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    key_hash         BLOB,              -- SHA-256 of the agent key; NULL until enrolled or after rotate
    join_token_hash  BLOB,              -- SHA-256 of the pending join token; NULL once used
    join_expires_at  INTEGER,
    last_seen_at     INTEGER,
    agent_version    TEXT,
    host_info        TEXT               -- JSON from Hello.host
);
```

Tokens are 32 random bytes, base64url, prefixed `fj_` (join) or `fk_` (key). SHA-256 suffices because they are high-entropy; lookups compare in constant time.

## 4. Flows

### 4.1 Enrollment

1. Admin: *Environments → Add → Agent*, enters a name. The server creates the `environments` row (`docker-agent`, `agent://<id>`) and a join token valid for **15 minutes, single use**, and returns it once with a ready command:
   ```sh
   docker run -d --name fleetops-agent --restart unless-stopped \
     -v fleetops-agent:/data -v /var/run/docker.sock:/var/run/docker.sock \
     --group-add <socket gid> \
     ghcr.io/pravinbanjade/fleetops-agent:<server version> \
     --server https://fleetops.example.com --join-token fj_…
   ```
2. Agent connects with `Bearer fj_…`. Server verifies hash and expiry, generates `fk_…`, stores its hash, clears the join token, and sends `Welcome { agent_key: Some(…) }`. The agent writes `agent.key` before doing anything else; on later starts it uses the key and ignores any join token.
3. **Rotate** (admin): clears `key_hash`, mints a new join token, closes the live session with `Bye { revoked }`. The UI shows the new command.
4. A join token that expires unused can be re-minted from the environment's page.

### 4.2 Connect and request path

Agent dials `wss://…/api/v1/agent/connect` → server authenticates (no cookie, no CSRF; agent credentials grant nothing else) → upgrade → agent opens the control stream and sends `Hello` → server checks `proto`, records `last_seen_at`, `agent_version`, `host_info`, replies `Welcome`, publishes `env_status`.

`GET /envs/web-01/containers` → handler → `env.docker()` → `LocalDocker` → bollard → custom transport → `AgentHub::open_stream` → yamux stream → agent → `/var/run/docker.sock`. Logs, stats, events, exec and build uploads are long-lived streams on the same path.

### 4.3 Liveness

The server sends `Ping` every 15 s; two missed `Pong`s (~45 s) or a closed WebSocket ends the session. In-flight streams then fail with `EnvUnavailable`; the UI's existing honest states apply (logs and terminal show "disconnected", lists show the environment as down). `last_seen_at` is written on connect, disconnect, and at most once a minute while up.

## 5. API

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/v1/agent/connect` | WebSocket upgrade, `Authorization: Bearer fj_…\|fk_…`. Not in the browser client. |
| `POST` | `/api/v1/envs` | Accepts `{ kind: "docker-agent", name }`; response includes `join_token` and `command` once. |
| `POST` | `/api/v1/envs/{env}/agent/rotate` | Admin. New join token + command; revokes the current key and session. |
| `GET` | `/api/v1/envs` | Agent environments add `agent: { connected, last_seen_at, agent_version, host }`. |

All admin endpoints use the existing session, CSRF, authorize and audit pipeline. The OpenAPI document and generated web client are regenerated (`make openapi`).

## 6. Web UI

- Environment create dialog gains a kind choice (Local socket / Agent). Agent shows the one-time command with copy, the expiry countdown, and the socket-gid hint already used in the README.
- Environment list and switcher show `online`, `offline · last seen …`, `waiting for agent` (enrolled never) and `incompatible` badges, updated live from `env_status`.
- Environment page: agent version, host info, *Rotate key*.

## 7. Error handling

| Situation | Behaviour |
|---|---|
| No session | `docker()` returns `None` → `EnvUnavailable` → 503 with reason; UI shows the environment down |
| Stream reset mid-request | bollard I/O error → `EnvUnavailable` |
| Protocol mismatch | `Bye { incompatible }`; reason stored and shown in status |
| Bad or expired token | 401, rate-limited; agent backs off (join token) or exits (stored key revoked) |
| Duplicate connection | Newer wins; older gets `Bye { replaced }` |
| Stream limit reached | `open_stream` fails → 503 "agent busy" |

## 8. Security

- Agent connect requires TLS, except `http://127.0.0.1`/`[::1]` for development.
- `/agent/connect` is rate-limited per IP with the existing limiter.
- Only the server opens data streams; the agent resets any other stream it did not expect.
- A compromised server is root on every agent host, exactly as it already is on its local socket; the README states this.
- Audit: agent environment created, key rotated, agent connected/disconnected (disconnects debounced to one entry per 5 minutes per environment), protocol rejected. Docker actions on agent environments are audited by the existing handlers.

## 9. Testing

- **Unit:** control codec round-trip; token lifecycle (expiry, single use, rotate); session replacement in `AgentHub`; backoff schedule.
- **Tunnel integration (no Docker):** in-process server and agent over a loopback WebSocket with an echo backend — throughput, backpressure (slow reader: writer stalls, memory stays flat), killing the socket mid-stream, reconnect and replacement.
- **docker-it through the tunnel:** the existing `docker_it` suite runs a second time with `LocalDocker` connected through an in-process tunnel to the real socket; it must pass in full, including exec. **This is the first plan task** — it proves bollard's custom transport plus hyper upgrades over yamux before anything else is built.
- **e2e:** a Playwright spec enrolls an agent container against the CI daemon, sees it come online, opens a container's logs and terminal, then stops the agent and sees the offline state.
- **Performance:** `rss-bench` gains an agent mode: server with 10 agents ≤ 50 MiB + 10 MiB; a single idle agent ≤ 15 MiB. Both run in CI.

## 10. Build and delivery

- Same Dockerfile, new targets `agent` (distroless image) and `agent-binary`; `make image-agent`.
- CI builds and smoke-tests the agent image; release publishes `ghcr.io/pravinbanjade/fleetops-agent` and `fleetops-agent-<version>-linux-{amd64,arm64}` binaries alongside the server's.
- Agent and server share the workspace version; the enrollment command pins the agent image to the server's version.

## 11. Out of scope

Agent self-update, policy or filtering on the agent, direct `tcp://` remotes, a listening agent, server HA or clustering, Swarm, host metrics beyond Docker's, per-environment RBAC (sub-project 5), Kubernetes (sub-project 4).

## 12. Risks

- **bollard custom transport + hyper upgrades over yamux.** Mitigated by making the through-tunnel docker-it run the first task; fallback is a small hyper `Connection` wrapper of our own.
- **yamux crate maturity under long-lived streams.** The tunnel integration tests cover stall and reset cases; `tokio-yamux` is the alternative if `yamux` misbehaves.
- **Reverse proxies in front of the server** may buffer or time out WebSockets. The 15 s ping keeps idle connections alive; the README documents proxy settings.
