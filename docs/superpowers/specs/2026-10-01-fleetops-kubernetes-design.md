# FleetOps — Sub-project 3: Kubernetes (read-mostly core)

- **Date:** 2026-10-01
- **Status:** Draft
- **Builds on:** [Sub-project 1 spec](./2026-09-29-fleetops-core-docker-design.md), [Sub-project 2 spec](./2026-10-01-fleetops-agent-multihost-design.md)

## 1. Intent

A FleetOps server manages Kubernetes clusters next to its Docker environments. A cluster is added either with a kubeconfig (the server reaches the API directly) or by applying a manifest that runs `fleetops-agent` inside the cluster; the agent dials out over the sub-project 2 tunnel, so private clusters need no inbound access and the server never holds their credentials. This first Kubernetes slice is **read-mostly**: browse the common kinds, inspect objects as YAML, follow pod logs, open a pod terminal, and scale, restart or delete. Creating and editing resources, Helm, and watch streams come in later slices.

Kubernetes moves ahead of Stacks + GitOps on the roadmap: it becomes sub-project 3, and Stacks + GitOps becomes sub-project 4.

### Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Order | Kubernetes now; the roadmap's 3 and 4 swap. |
| First slice | Read-mostly core: 12 kinds, YAML detail, events, logs, exec, scale, restart, delete. |
| Connection | Both: kubeconfig (direct) and an in-cluster agent (tunnel). |
| Stored credentials | Encrypted at rest with AES-256-GCM under a server key. |
| Kubeconfig auth | Bearer token and client certificate, inline data only. No exec or auth-provider plugins. |
| API shape | Generic over a closed set of kinds (one `KubeOps` trait, common row DTO), with typed action routes. No apiserver passthrough. |

### Success criteria

- An admin pastes a kubeconfig, picks a context, and sees the cluster's workloads within 5 s of saving.
- An admin adds an agent cluster, runs one `kubectl apply` command, and sees the environment come online within 10 s of the agent pod starting.
- The `kube_it` integration suite passes against a `kind` cluster both through a kubeconfig and through the agent tunnel, including follow-logs and exec with resize.
- No API response, log line or audit row ever contains a stored kubeconfig, a ServiceAccount token or a Secret value.
- Server idle RSS with 5 kubeconfig environments being polled stays within the sub-project 2 gate + 5 MiB.
- Every existing Docker page and test behaves as before.

## 2. Architecture

### 2.1 Layout changes

```
crates/
  fleetops-core/     + kube.rs (KubeOps trait, KubeKind, DTOs); env.rs gains kinds, kube(), EnvVersion
  fleetops-kube/     NEW: kube-rs implementation (KubeClient), summaries, masking, kubeconfig parsing
  fleetops-store/    + secrets.rs (AES-256-GCM), migration 0003_env_credentials.sql
  fleetops-tunnel/   control.rs: HostInfo.serves
  fleetops-agent/    + kube_proxy.rs, creds backends (File, KubeSecret), --kube mode
  fleetops-docker/   env factory delegates kube kinds to fleetops-kube
  fleetops-api/      + routes/kube/ (lists, detail, actions, logs, exec)
web/src/pages/kube/  NEW: overview, kind lists, detail, namespace picker
```

`fleetops-core` stays free of kube-rs, the way it stays free of bollard.

### 2.2 Environment kinds

`EnvKind` gains:

- `kube-config` — the server talks to the cluster's API with a stored kubeconfig.
- `kube-agent` — the cluster is reached through an in-cluster `fleetops-agent`.

`Environment` gains `fn kube(&self) -> Option<Arc<dyn KubeOps>>`. Docker environments return `None` from it; Kubernetes environments return `None` from `docker()`.

`EnvStatus::Up { version: EngineVersion }` becomes `EnvStatus::Up { version: EnvVersion }`:

```rust
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EnvVersion {
    Docker(EngineVersion),
    Kube { git_version: String, platform: String, nodes: u32, nodes_ready: u32 },
}
```

This changes the wire shape of an environment's status. The OpenAPI document and the generated web types are regenerated in the same change.

`EnvFactory::build` takes the stored record instead of loose fields, so a factory can read credentials:

```rust
fn build(&self, rec: &EnvRecord, cred: Option<&EnvCredential>) -> Result<Arc<dyn Environment>>;
```

### 2.3 `fleetops-core::kube`

```rust
pub enum KubeKind {
    Namespaces, Nodes,                                         // cluster-scoped
    Deployments, StatefulSets, DaemonSets, Jobs, CronJobs,
    Pods, Services, Ingresses, ConfigMaps, Secrets,
}

pub struct KubeRow {
    pub name: String,
    pub namespace: Option<String>,
    pub created: i64,                    // unix seconds
    pub status: String,                  // e.g. "Running", "3/3", "Complete"
    pub health: Health,                  // Ok | Warn | Bad | Unknown, for the status dot
    pub columns: BTreeMap<String, String>, // kind-specific: ready, restarts, node, type, ports, hosts…
}

pub struct KubeList { pub rows: Vec<KubeRow>, pub truncated: bool }

pub struct KubeObject {
    pub row: KubeRow,
    pub yaml: String,                    // managedFields stripped, Secret values masked
    pub conditions: Vec<KubeCondition>,
    pub owners: Vec<KubeRef>,            // the owner chain, e.g. Pod → ReplicaSet → Deployment
    pub containers: Vec<String>,         // pods only; for the logs and terminal pickers
}

#[async_trait]
pub trait KubeOps: Send + Sync {
    async fn version(&self) -> Result<KubeVersion>;
    async fn namespaces(&self) -> Result<Vec<String>>;
    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList>;
    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject>;
    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>>;
    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()>;
    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    async fn logs(&self, ns: &str, pod: &str, opts: KubeLogOpts) -> Result<ByteStream>;
    async fn exec(&self, ns: &str, pod: &str, opts: KubeExecOpts) -> Result<ExecIo>;
}
```

For cluster-scoped kinds, `ns` is ignored and passed as `"-"` in URLs. `scale` accepts Deployments and StatefulSets; `restart` accepts Deployments, StatefulSets and DaemonSets; other kinds return `Error::Invalid`. `ByteStream` and `ExecIo` reuse the stream types that `StreamOps` already uses for Docker logs and exec, so the API's WebSocket bridges are shared.

### 2.4 `fleetops-kube`

`KubeClient` implements `KubeOps` over a `kube::Client`. A client is built from one of two sources:

- **Kubeconfig:** a `kube::Config` from the decrypted, single-context kubeconfig.
- **Tunnel:** `kube::Client::new(service, default_ns)` over a tower service whose connector opens a yamux stream through a `StreamOpener` and speaks plain HTTP/1.1 on it. This is the same pattern as bollard's custom transport in sub-project 2.

Each kind has one summary function (`fn row(obj) -> KubeRow`) in `summaries.rs`; these are the only kind-specific code paths. Restart patches `spec.template.metadata.annotations["kubectl.kubernetes.io/restartedAt"]` with the current time, the same as `kubectl rollout restart`. Scale patches the `scale` subresource.

The crate also holds:

- `kubeconfig.rs` — parse, list contexts, pick one, reject unsupported users, minify to a single self-contained context.
- `mask.rs` — YAML rendering with `metadata.managedFields` removed and every value under a Secret's `data` and `stringData` replaced by `"••••"` (keys stay visible).

Environments:

- `KubeConfigEnv` — builds its `KubeClient` on first use and keeps it. A changed credential rebuilds the environment, as a changed URL does today.
- `KubeAgentEnv` — takes the live session's `StreamOpener` from the sub-project 2 `Registry`; offline when no session exists. `wait_change` delegates to the registry.

`status()` calls `/version` and reads a node count cached for 30 s.

### 2.5 Agent: `--kube` mode

`fleetops-agent` gains `--kube` (env `FLEETOPS_KUBE=1`), which conflicts with `--docker`. In kube mode:

- Each inbound yamux stream is served as HTTP/1.1 with hyper's server side.
- Only paths that start with `/api/` or `/apis/`, or that equal `/version`, are forwarded; anything else gets 403 from the agent.
- Any `Authorization` header from the server is removed and replaced with `Bearer <token>`, read from `/var/run/secrets/kubernetes.io/serviceaccount/token`. The token is re-read every 60 s, because projected tokens rotate.
- Requests go through one pooled hyper client over TLS to `https://kubernetes.default.svc` (overridable with `KUBERNETES_SERVICE_HOST`/`PORT`), trusting `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt`.
- Upgrade requests (exec over WebSocket or SPDY) are passed through: the agent forwards the upgrade, then splices the two upgraded connections with `hyper::upgrade`.

`HostInfo` gains `serves: Serves` (`docker` | `kube`, serde default `docker`). This is an additive field, so `PROTO_MAJOR` stays 1 and existing Docker agents keep working. `HostInfo.docker` is renamed in meaning to the agent's target and keeps its wire name.

**Credential storage.** The `creds` module gets a backend trait with two implementations:

- `File` — today's 0600 key file in `--data`.
- `KubeSecret` — in kube mode without `--data`, the key is stored in Secret `fleetops-agent-key` in the agent's namespace (read from the downward API env `POD_NAMESPACE`). The agent creates it on first enrollment and updates it on re-enrollment.

The join token comes from env `FLEETOPS_JOIN_TOKEN`, fed from a Secret in the manifest.

### 2.6 Server side

- The factory in `fleetops-docker` builds Docker kinds as today and delegates `kube-config` and `kube-agent` to `fleetops-kube`.
- `connect.rs` checks the agent's `serves` against the environment's kind after the hello: a mismatch sends `Bye { Incompatible }` and records the reason `agent serves docker; this environment expects kube` (or the reverse) on the environment.
- The enrollment command for a `kube-agent` environment is a manifest piped to `kubectl apply -f -` (§4.2).

## 3. Data model

Migration `0003_env_credentials.sql`:

```sql
CREATE TABLE env_credentials (
  env_id      INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL,            -- 'kubeconfig'
  ciphertext  TEXT NOT NULL,            -- 'v1:' || base64(nonce || ciphertext || tag)
  summary     TEXT NOT NULL,            -- JSON: {server, context, user_kind}; safe to show
  updated_at  INTEGER NOT NULL
);
```

For `kube-config` environments, `environments.url` holds the cluster's API server URL, for display. For `kube-agent` environments it holds `agent://`, as `docker-agent` does. Agent credentials stay in `agent_credentials`, unchanged.

### 3.1 Server secret key (`fleetops-store::secrets`)

- `FLEETOPS_SECRET_KEY`: 32 bytes, base64. If it is unset, the server uses `<data>/secret.key`, creating it with mode 0600 on first start and logging once that it did.
- A malformed `FLEETOPS_SECRET_KEY` is a startup error.
- AES-256-GCM with a random 96-bit nonce per encryption; the environment id is the associated data, so a ciphertext copied to another row fails to decrypt.
- The `v1:` prefix leaves room for key rotation later.
- A ciphertext that fails to decrypt (lost or changed key) does not stop the server: that environment is Down with `credential cannot be decrypted; re-enter the kubeconfig`.

## 4. Flows

### 4.1 Adding a kubeconfig cluster

`POST /envs` with `{name, kind: "kube-config", kubeconfig, context?, skip_check?}`:

1. Parse the kubeconfig. Reject YAML errors, a missing or unknown context (when there are several contexts and none is given, return 400 listing them), `exec` and `auth-provider` users (`use a ServiceAccount token or the FleetOps agent`), and any file reference (`certificate-authority`, `client-certificate`, `client-key`, `tokenFile`: inline `*-data` only).
2. Minify to the chosen context.
3. Unless `skip_check`, call `/version` with a 5 s timeout; on failure return 400 `cannot reach cluster: <reason>` and store nothing.
4. Encrypt and store; `url` = the cluster's server URL.

`POST /envs/kube/contexts` with `{kubeconfig}` returns the context names and the default context, so the form can show a picker before saving. It stores nothing.

`PATCH /envs/{env}` may replace `kubeconfig` (same checks) or rename. The stored kubeconfig is never returned; environment views show `summary` (server, context, user kind).

### 4.2 Adding an agent cluster

`POST /envs` with `{name, kind: "kube-agent"}` creates the environment and a join token exactly as sub-project 2 does, and returns an `Enrollment` whose `command` is:

```
kubectl apply -f - <<'EOF'
<manifest>
EOF
```

The manifest contains:

- Namespace `fleetops`.
- ServiceAccount `fleetops-agent`.
- ClusterRole `fleetops-agent` + ClusterRoleBinding: `get`, `list`, `watch` on the 12 kinds and `events`; `delete` on the 12 kinds except nodes and namespaces; `patch` on deployments, statefulsets, daemonsets and their `scale` subresource; `get` on `pods/log`; `create` and `get` on `pods/exec`; `get` on `/version`.
- Role + RoleBinding in `fleetops`: `get`, `update` on Secret `fleetops-agent-key` (`resourceNames`), and `create` on secrets (Kubernetes cannot restrict `create` by name).
- Secret `fleetops-agent-join` with `FLEETOPS_JOIN_TOKEN`.
- Deployment `fleetops-agent`: one replica, the agent image, `--kube`, `FLEETOPS_SERVER`, `POD_NAMESPACE` from the downward API, a read-only root filesystem, non-root, requests of 10m CPU and 16Mi memory, limit 64Mi.

Rotating the key (`POST /envs/{env}/agent/rotate`) returns a fresh manifest; re-applying it updates the join Secret and restarts the pod (the pod template carries a hash annotation of the join token). The agent's key-rejected fallback from sub-project 2 then re-enrolls.

The README documents a read-only ClusterRole variant without the write verbs; actions then fail with the Kubernetes 403 reason.

### 4.3 Request path through an agent

`KubeClient` (server) → tower connector → `Session::open_stream` → yamux → agent → hyper server on the stream → path check, token injection → pooled TLS client → kube-apiserver. Responses stream back the same way. Upgrades are spliced end to end. The sub-project 2 limit of 256 streams per session applies; kube-rs pools connections, so normal use opens a handful.

### 4.4 Liveness

Kubeconfig environments: the existing hub calls `status()` with its backoff and publishes changes. Agent environments: sub-project 2's ping/pong and registry changes, unchanged.

## 5. API

All routes are under `/api/v1/envs/{env}/k8s`. A Kubernetes route on a Docker environment, or a Docker route on a Kubernetes environment, returns 409 `wrong environment kind`. `{kind}` is the plural lowercase kind name; an unknown kind is a 404. `{ns}` is `-` for cluster-scoped kinds.

| Method | Path | Role | Notes |
|---|---|---|---|
| GET | `/namespaces` | viewer | Names only, for the picker |
| GET | `/{kind}?namespace=` | viewer | `KubeList`; no `namespace` means all namespaces |
| GET | `/{kind}/{ns}/{name}` | viewer | `KubeObject` |
| GET | `/{kind}/{ns}/{name}/events` | viewer | `KubeEvent[]`, newest first |
| DELETE | `/{kind}/{ns}/{name}` | operator | Audited `k8s.delete` |
| POST | `/{kind}/{ns}/{name}/scale` `{replicas}` | operator | Deployments, StatefulSets; audited `k8s.scale` |
| POST | `/{kind}/{ns}/{name}/restart` | operator | Deployments, StatefulSets, DaemonSets; audited `k8s.restart` |
| WS | `/pods/{ns}/{name}/logs?container=&follow=&tail=&timestamps=` | operator | Same framing as Docker logs |
| WS | `/pods/{ns}/{name}/exec?container=&cmd=` | operator | Same terminal protocol as Docker exec, including resize; audited `k8s.exec` |

Plus, outside `/k8s`: `POST /envs/kube/contexts` (admin) from §4.1. Audit targets are `kind/ns/name` within the environment. Roles follow sub-project 1: logs and exec need operator, as they do for containers.

Limits: a list stops at 5,000 rows (`truncated: true`), paging with `limit`/`continue` underneath; a YAML document over 1 MiB is cut with a note; requests to the cluster time out after 30 s, except logs and exec.

## 6. Web UI

- The environment switcher lists all kinds. On a Kubernetes environment the sidebar shows: Overview, Workloads (tabs: Deployments, StatefulSets, DaemonSets, Jobs, CronJobs), Pods, Services, Ingresses, ConfigMaps, Secrets, Namespaces, Nodes.
- A namespace picker sits in the page header, defaults to "All namespaces", and is remembered for each environment in local storage.
- **Overview:** Kubernetes version, platform, nodes ready of total, and pods by phase.
- **Lists** reuse the existing table with kind-specific columns, for example Deployment: Ready, Up-to-date, Available, Age; Pod: Ready, Status, Restarts, Node, Age; Service: Type, Cluster IP, Ports, Age. They refresh every 5 s while the tab is visible. A 403 shows "No permission to list {kind} in {namespace}", not an empty table. A truncated list says so.
- **Detail:** tabs Summary (row, conditions, owner chain as links), YAML (read-only, highlighted), Events. Pods add Logs (container picker, follow) and Terminal, reusing the existing components. Actions (scale with a number dialog, restart, delete with confirm) appear for operators and admins only.
- **Admin › Environments:** the kind radios gain "Kubernetes (kubeconfig)", with a kubeconfig textarea, a context picker filled from `/envs/kube/contexts`, and a "save without checking" box, and "Kubernetes (agent)", which shows the `kubectl apply` command in the existing enrollment dialog. The list shows the server, context and user kind for kubeconfig clusters.

## 7. Error handling

| Kubernetes API | `fleetops_core::Error` | HTTP |
|---|---|---|
| 404 | NotFound | 404 |
| 403 | Forbidden (message keeps the RBAC reason) | 403 |
| 409 | Conflict | 409 |
| 422 | Invalid | 400 |
| timeout, connect, 5xx | Upstream | 502 |

- An unreachable cluster or offline agent makes the environment Down with the reason; routes on it return 503 `environment unavailable`, as Docker routes do.
- An agent in kube mode that cannot read its ServiceAccount token or CA exits with a clear error (the pod restarts and the reason is in its logs).
- A credential that cannot be decrypted: §3.1.

## 8. Security

- Stored kubeconfigs are encrypted at rest (§3.1) and never returned by the API.
- Secret values are never returned: masked in YAML on the server, and absent from every DTO. Revealing a value is out of scope.
- Tracing redacts `Authorization` headers on both the server's kube client and the agent's proxy; kubeconfig contents are never logged.
- The agent forwards only Kubernetes API paths and always sends its own token, so the server cannot present other credentials through it.
- The agent's in-cluster permissions are the ClusterRole in §4.2: no Secret writes beyond its own key Secret's namespace, no wildcard verbs.
- Agents must connect over HTTPS, or from loopback, as in sub-project 2.
- The README says to back up `secret.key` (or `FLEETOPS_SECRET_KEY`) with the database: without it, stored kubeconfigs are lost.

## 9. Testing

- **Task 1 proves the risky path first:** a kube-rs `Client` over a yamux `StreamOpener`, through an in-process agent kube proxy, to a real `kind` cluster: list, follow-logs, and exec with resize. If the upgrade splice fails, the design changes before anything is built on it.
- **`make test-kube`** creates or reuses a `kind` cluster `fleetops-test` and runs the `kube_it` suite twice: through a kubeconfig, and through the tunnel with the agent's kube proxy using a ServiceAccount token from the cluster. It covers list and get for every kind, scale and restart (confirmed by re-reading), delete, logs (tail and follow), exec (echo and resize), Secret masking, and a 403 from a ServiceAccount restricted to one namespace. CI gains a `kube` job using `helm/kind-action`.
- **Unit tests:** summary functions for each kind against fixture JSON; masking and `managedFields` stripping; error mapping; kubeconfig context selection, rejection of exec, auth-provider and file-ref users, and minify; secrets round trip, wrong key, associated data, `v1:` prefix, 0600 key file; agent proxy against a fake TLS apiserver (token replaced, path allowlist, upgrade passthrough, token re-read); the `KubeSecret` creds backend against a fake API; `serves` defaults to docker, and a mismatch gets a bye and a status reason.
- **API tests** with a `FakeKube` through the existing fake factory: every route, 409 on wrong kind, role gating, audit rows, 403 passthrough, truncation; kubeconfig create errors, `skip_check`, contexts endpoint, and that the kubeconfig never appears in a response. OpenAPI operation count updated.
- **Web tests (vitest):** namespace picker persistence, kind columns, the 403 state, masked YAML, the kube radios and context picker.
- **Playwright `kube.spec.ts`**, run when `FLEETOPS_KIND=1`: add a kubeconfig cluster, browse deployments, scale one, read pod logs, open a terminal.
- **`scripts/agent-kube-smoke.sh`:** loads the agent image into kind, applies the generated manifest, waits for the environment to show Up.
- **rss-bench** gains a kube phase: 5 kubeconfig environments with lists polled; the server stays within the sub-project 2 gate + 5 MiB.

## 10. Build and delivery

- New dependencies: `kube` (client, ws features), `k8s-openapi` (minimum supported Kubernetes 1.28), `aes-gcm`, and a maintained YAML serializer (`serde_yaml` is archived; the plan picks one). All pure Rust with rustls; the images stay distroless.
- The agent image is unchanged apart from the new mode; the same image serves Docker and Kubernetes.
- Makefile: `test-kube`, `agent-kube-smoke`. CI: the `kube` job. Release: no new artifacts.
- README: a "Kubernetes clusters" section (both ways in, required permissions, the read-only variant, the secret key and backups), and the roadmap renumbered (3 Kubernetes, 4 Stacks + GitOps).

## 11. Out of scope

Creating or editing resources (YAML apply), Helm, watch streams (lists poll), CRDs, port-forward, metrics-server graphs, multi-cluster views, exec and auth-provider kubeconfig users, revealing Secret values, per-namespace RBAC inside FleetOps (sub-project 5), key rotation for `secret.key`.

## 12. Risks

| Risk | Mitigation |
|---|---|
| kube-rs exec upgrade does not survive the custom tower transport or the agent's splice | Task 1 proves it end to end against a real cluster before other work. |
| `EnvStatus` wire change breaks the UI | Regenerated types in the same change; existing web tests cover status rendering. |
| Agent holds cluster-wide delete rights | Narrow ClusterRole, documented read-only variant, actions audited. |
| Lost `secret.key` loses every kubeconfig | Clear Down reason, re-entry flow, README backup guidance. |
| Polling many lists costs server memory and cluster load | 5 s poll only while visible, 5,000-row cap, rss-bench gate. |
