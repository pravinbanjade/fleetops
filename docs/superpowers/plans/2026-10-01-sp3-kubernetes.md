# Sub-project 3: Kubernetes (read-mostly core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FleetOps manages Kubernetes clusters next to Docker environments, through a stored kubeconfig or an in-cluster agent. The first slice can browse 12 kinds, show YAML (Secrets masked) and events, follow pod logs, open a pod terminal, and scale, restart or delete.

**Architecture:**
- `fleetops-core` gains a kube-rs-free `KubeOps` trait over a closed `KubeKind` enum. `fleetops-kube` implements it on kube-rs, with one of two clients:
  - built from a kubeconfig the server stores encrypted;
  - a kube-rs client whose connector opens yamux streams through the sub-project 2 tunnel to an agent running `--kube`. The agent swaps in its ServiceAccount token and forwards to the apiserver over TLS.
- The API adds typed REST routes under `/envs/{env}/k8s`. Logs and exec reuse the existing multiplexed WebSocket topics. The web gets a kind-aware sidebar and Kubernetes pages that reuse the Docker table, log and terminal components.

**Tech Stack:**
- Rust: kube 4.2, k8s-openapi 0.28 (`v1_32`), hyper 1 and hyper-util, hyper-rustls 0.27, aes-gcm 0.10, serde-saphyr, axum 0.8, sqlx/SQLite.
- Web: React 19 with TanStack Router, Query and Table, xterm, vitest, Playwright.
- Test cluster: kind.

**Spec:** `docs/superpowers/specs/2026-10-01-fleetops-kubernetes-design.md`

## Global Constraints

- Workspace `rust-version = "1.89"` (kube 4.2 needs it); edition 2021.
- `kube = { version = "4.2", default-features = false, features = ["client", "rustls-tls", "ring", "ws"] }`; `k8s-openapi = { version = "0.28", features = ["v1_32"] }`. Minimum supported Kubernetes: 1.32.
- TLS is rustls with the ring provider everywhere, so the images stay distroless.
- `fleetops-core` must not depend on kube-rs or k8s-openapi.
- No new web dependencies. YAML is shown in a read-only `<pre>`, like `JsonView`.
- Environment kinds, as the wire strings: `docker-local`, `docker-agent`, `kube-config`, `kube-agent`.
- Kind path names: `namespaces`, `nodes`, `deployments`, `statefulsets`, `daemonsets`, `jobs`, `cronjobs`, `pods`, `services`, `ingresses`, `configmaps`, `secrets`. Cluster-scoped kinds (namespaces, nodes) use `-` as the namespace in paths.
- The Secret mask string is `••••`. Secret `data`/`stringData` values and the `kubectl.kubernetes.io/last-applied-configuration` annotation on Secrets are always masked.
- Limits:
  - 5,000 rows per list (`truncated: true` beyond that), fetched in pages of 500;
  - 1 MiB of YAML;
  - 30 s cluster request timeout, except logs and exec;
  - 5 s reachability check on create;
  - node counts cached for 30 s;
  - the agent re-reads its token every 60 s;
  - web lists poll every 5 s while visible;
  - web status polls every 15 s for kube environments.
- Authz: `Action::KubeRead` covers lists, get, events and logs; `Action::KubeWrite` covers delete, scale, restart and exec. Both are granted to `admin` and `user`, the same as Docker actions.
- Audit:
  - actions `k8s.delete`, `k8s.scale`, `k8s.restart` and `k8s.exec`;
  - `target_type = "k8s"`, `target_id = "<kind>/<ns>/<name>"`;
  - `environment_id` is set.
- Secret key:
  - `FLEETOPS_SECRET_KEY` holds 32 bytes, base64-encoded. Otherwise the server uses `<data>/secret.key`, created with mode 0600.
  - Ciphertext is `v1:` + base64(nonce ‖ ciphertext‖tag).
  - The AAD is the environment id as decimal ASCII.
- Agent environment variables: `FLEETOPS_AGENT_SERVER`, `FLEETOPS_AGENT_JOIN_TOKEN` and `FLEETOPS_AGENT_KUBE`. The in-cluster namespace comes from `POD_NAMESPACE`.
- Error messages are verbatim:
  - `wrong environment kind: '<env>' is <kind>`
  - `use a ServiceAccount token or the FleetOps agent: exec and auth-provider users are not supported`
  - `kubeconfig must embed its files: '<field>' points to a file`
  - `cannot reach cluster: <reason>`
  - `credential cannot be decrypted; re-enter the kubeconfig`
  - `fleetops-agent forwards only Kubernetes API paths`
  - `agent serves <a>; this environment expects <b>`
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3
  ```
- Tests that need a cluster are gated: the Rust ones by the `kube-it` cargo feature plus the `FLEETOPS_KUBE_IT` directory, and the Playwright one by `FLEETOPS_KIND=1`. `make test` never needs a cluster.

## Review Focus

These are the input classes the spec implies but doesn't test, most likely first. Each one has a test in the task that owns the code.

1. **A pod with several containers, with no container chosen:** logs and exec must default to the first container in `spec.containers` and never surface Kubernetes's "a container name must be specified". Task 7 `logs_and_exec_default_to_the_first_container`.
2. **A Secret created with `kubectl apply`:** it carries its whole body, values included, in `metadata.annotations["kubectl.kubernetes.io/last-applied-configuration"]`. The YAML view must mask that annotation too. Tests: Task 6 `a_secrets_last_applied_annotation_is_masked`, and Task 7 `secret_yaml_never_contains_its_value`.
3. **The pod goes away mid-exec:** the terminal must get `end` quickly, with `exit_code: null`, and not hang. Task 13 `a_kube_exec_whose_pod_disappears_ends_promptly`.
4. **An unreachable cluster at server start:** the server must start, the environment must show Down with a reason, and startup must not block. Task 8 `an_unreachable_cluster_is_down_without_blocking`.
5. **A browser tab still on a Docker route** (`/envs/k/containers`) after the environment became Kubernetes, or opened from history: it must redirect to the Kubernetes overview, not issue Docker calls that fail with 409. Task 14 `docker_routes_under_a_kube_env_redirect_to_its_overview`.

## File Map

```
Cargo.toml                                  rust-version 1.89; kube, k8s-openapi, hyper-rustls, aes-gcm, base64, serde-saphyr
crates/fleetops-tunnel/src/http.rs          NEW  TunnelConnector (moved from fleetops-docker/src/tunnel.rs)
crates/fleetops-tunnel/src/testing.rs       + loopback_with
crates/fleetops-tunnel/src/control.rs       HostInfo.serves (Serves enum)
crates/fleetops-core/src/kube.rs            NEW  KubeKind, KubeRow, Health, KubeList, KubeObject, KubeCondition, KubeRef, KubeEvent, KubeVersion, KubeExec, KubeOps
crates/fleetops-core/src/env.rs             EnvKind::{KubeConfig,KubeAgent}, EnvVersion, EnvSpec, Credential, Environment::kube(), EnvFactory::build(&EnvSpec)
crates/fleetops-core/src/streams.rs         LogOptions.container, ExecOptions.container
crates/fleetops-store/src/secrets.rs        NEW  SecretKey, SecretBox (AES-256-GCM)
crates/fleetops-store/src/credentials.rs    NEW  put/get/delete env credential
crates/fleetops-store/migrations/0003_env_credentials.sql  NEW
crates/fleetops-kube/                       NEW crate
  src/lib.rs            re-exports
  src/tunnel.rs         kube::Client over a StreamOpener
  src/kubeconfig.rs     parse, contexts, validate, minify, summary
  src/kinds.rs          KubeKind -> ApiResource, kind names
  src/summaries.rs      DynamicObject -> KubeRow per kind
  src/mask.rs           YAML render with managedFields stripped and Secret values masked
  src/errors.rs         kube::Error -> fleetops_core::Error
  src/client.rs         KubeClient: KubeOps
  src/env.rs            KubeConfigEnv, KubeAgentEnv, KubeEnvFactory
  tests/common/mod.rs   kind fixture loader, clients
  tests/kube_it.rs      cluster suite (feature kube-it)
crates/fleetops-docker/src/tunnel.rs        uses fleetops_tunnel::http::TunnelConnector
crates/fleetops-docker/src/env.rs           DockerEnvFactory::build(&EnvSpec)
crates/fleetops-server/src/factory.rs       NEW  Factory { docker, kube } composite
crates/fleetops-server/src/lib.rs           build_state uses Factory; secret key loading
crates/fleetops-server/src/config.rs        secret_key option
crates/fleetops-agent/src/kube_proxy.rs     NEW  KubeUpstream, KubeProxy, allowed()
crates/fleetops-agent/src/creds.rs          KeyStore trait: FileKeys, SecretKeys
crates/fleetops-agent/src/config.rs         --kube
crates/fleetops-agent/src/session.rs        serve kube or docker streams; Hello.serves
crates/fleetops-api/src/authz.rs            KubeRead, KubeWrite
crates/fleetops-api/src/envs.rs             insert/replace take Option<Credential>; load reads credentials
crates/fleetops-api/src/routes/envs.rs      kube-config create/update, contexts, kube-agent manifest
crates/fleetops-api/src/agents/manifest.rs  NEW  kube-agent manifest
crates/fleetops-api/src/routes/kube.rs      NEW  /envs/{env}/k8s routes
crates/fleetops-api/src/ws/{topics,conn}.rs logs/exec dispatch to kube; ExecCtl resize handle
crates/fleetops-api/tests/common/fake_kube.rs NEW FakeKube, FakeKubeEnv
crates/fleetops-api/tests/kube_routes.rs    NEW
crates/fleetops-api/tests/kube_envs.rs      NEW
crates/fleetops-api/tests/ws_kube.rs        NEW
web/src/components/nav.ts, Shell.tsx        kind-aware nav, switcher icon
web/src/router.tsx                          k8s routes, env home switch, Docker-route redirect
web/src/api/queries.ts, kube.ts             kube queries, polling
web/src/live/LiveProvider.tsx               skip kube envs for events
web/src/pages/kube/                         NEW  KubeOverviewPage, KubeListPage, KubeObjectPage, NamespacePicker, columns.ts, YamlView, actions
web/src/pages/admin/EnvironmentsPage.tsx    kube kinds, kubeconfig form, context picker
web/e2e/kube.spec.ts                        NEW  (FLEETOPS_KIND=1)
scripts/kind-up.sh, scripts/kube-it-fixtures.yaml, scripts/agent-kube-smoke.sh   NEW
scripts/rss-bench.sh                        kube phase
Makefile, .github/workflows/ci.yml, README.md, Dockerfile (unchanged targets)
```

---

### Task 1: Prove kube-rs through the tunnel (risk-first)

This proves the design's riskiest assumption before anything is built on it. A kube-rs `Client` whose connector opens yamux streams goes through an in-process agent kube proxy to a real kind cluster: it lists pods, follows a log stream, and runs a TTY exec that sees a resize. If the WebSocket upgrade can't be spliced, stop and report: the design changes.

**Files:**
- Modify: `Cargo.toml` (workspace): `rust-version = "1.89"`; add workspace deps
- Create: `crates/fleetops-tunnel/src/http.rs`; Modify: `crates/fleetops-tunnel/src/lib.rs`, `crates/fleetops-tunnel/Cargo.toml`, `crates/fleetops-tunnel/src/testing.rs`
- Modify: `crates/fleetops-docker/src/tunnel.rs` (use the moved connector), `crates/fleetops-docker/Cargo.toml`
- Create: `crates/fleetops-kube/Cargo.toml`, `crates/fleetops-kube/src/lib.rs`, `crates/fleetops-kube/src/tunnel.rs`
- Create: `crates/fleetops-agent/src/kube_proxy.rs`; Modify: `crates/fleetops-agent/src/lib.rs`, `crates/fleetops-agent/Cargo.toml`
- Create: `scripts/kind-up.sh`, `scripts/kube-it-fixtures.yaml`
- Create: `crates/fleetops-kube/tests/common/mod.rs`, `crates/fleetops-kube/tests/kube_it.rs`
- Modify: `Makefile` (`kind-up`, `test-kube`)

**Interfaces:**
- Produces:
  - `fleetops_tunnel::http::TunnelConnector::new(opener: Arc<dyn StreamOpener>) -> TunnelConnector`, a hyper-util connector (tower `Service<Uri>`).
  - `fleetops_tunnel::testing::loopback_with(serve: impl FnMut(StreamHandle) + Send + 'static) -> Session`.
  - `fleetops_kube::tunnel::client(opener: Arc<dyn StreamOpener>) -> kube::Client`.
  - `fleetops_agent::kube_proxy::{KubeUpstream { base: Uri, ca: PathBuf, token: PathBuf }, KubeUpstream::in_cluster() -> anyhow::Result<KubeUpstream>, KubeProxy, KubeProxy::new(KubeUpstream) -> anyhow::Result<Arc<KubeProxy>>, KubeProxy::serve(self: Arc<Self>, StreamHandle) -> Result<(), hyper::Error>, KubeProxy::refresh_token(&self) -> io::Result<()>, KubeProxy::spawn_refresh(self: &Arc<Self>, Duration) -> JoinHandle<()>, allowed(path: &str) -> bool, TOKEN_REFRESH: Duration}`.
  - The kind fixture directory, `$FLEETOPS_KUBE_IT`, holds `kubeconfig`, `server`, `ca.crt`, `token` and `narrow-token`.

- [ ] **Step 0: Make sure `kind` is installed**

Run: `command -v kind || echo missing`
Expected: a path. If it prints `missing`, stop and ask your human partner to run `brew install kind`. That installs software outside the repo, so it isn't yours to do.

- [ ] **Step 1: Workspace dependencies**

In `Cargo.toml`, set `rust-version = "1.89"` under `[workspace.package]` and add to `[workspace.dependencies]` (keep the list alphabetical):

```toml
fleetops-kube = { path = "crates/fleetops-kube" }
aes-gcm = "0.10"
base64 = "0.22"
hyper-rustls = { version = "0.27", default-features = false, features = ["http1", "ring", "tls12", "logging"] }
k8s-openapi = { version = "0.28", features = ["v1_32"] }
kube = { version = "4.2", default-features = false, features = ["client", "rustls-tls", "ring", "ws"] }
serde-saphyr = "0.0.29"
```

Change `hyper` to `{ version = "1", features = ["client", "server", "http1"] }` and `hyper-util` to `{ version = "0.1", features = ["client-legacy", "server", "http1", "tokio"] }`.

- [ ] **Step 2: Move the tunnel connector into `fleetops-tunnel`**

Create `crates/fleetops-tunnel/src/http.rs` with the `TunnelConnector` and `TunnelIo` code from `crates/fleetops-docker/src/tunnel.rs` (they move unchanged, made public):

```rust
//! A hyper-util connector whose "connections" are yamux streams. Shared by
//! bollard (Docker agents) and kube-rs (Kubernetes agents); both run plain
//! HTTP/1.1 over the stream, and upgrades work because hyper owns it.

use crate::{StreamHandle, StreamOpener};
use hyper::Uri;
use hyper_util::{
    client::legacy::connect::{Connected, Connection},
    rt::TokioIo,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Opens a fresh tunnel stream for every connection hyper asks for. The
/// URI is ignored: the agent decides where the stream goes.
#[derive(Clone)]
pub struct TunnelConnector(Arc<dyn StreamOpener>);

impl TunnelConnector {
    pub fn new(opener: Arc<dyn StreamOpener>) -> Self {
        Self(opener)
    }
}

impl tower_service::Service<Uri> for TunnelConnector {
    type Response = TunnelIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<TunnelIo>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let opener = self.0.clone();
        Box::pin(async move { Ok(TunnelIo(TokioIo::new(opener.open().await?))) })
    }
}

pub struct TunnelIo(TokioIo<StreamHandle>);

impl Connection for TunnelIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for TunnelIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for TunnelIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
```

In `crates/fleetops-tunnel/src/lib.rs`, add `pub mod http;`. Change the crate doc's last line to: `//! Only \`http\` knows about HTTP: it adapts streams to a hyper connector.`

In `crates/fleetops-tunnel/Cargo.toml` `[dependencies]`, add `hyper.workspace = true`, `hyper-util.workspace = true` and `tower-service.workspace = true`.

In `crates/fleetops-docker/src/tunnel.rs`, delete `TunnelConnector`, `TunnelIo` and their impls. Import `fleetops_tunnel::http::TunnelConnector`, and build the client with `.build(TunnelConnector::new(opener))`. Drop the imports that are now unused.

- [ ] **Step 3: Confirm the move broke nothing**

Run: `cargo test -p fleetops-docker -p fleetops-tunnel 2>&1 | tail -5`
Expected: all pass. The existing tunnel tests in `fleetops-docker/src/tunnel.rs` exercise the connector.

- [ ] **Step 4: `loopback_with`**

Replace the body of `crates/fleetops-tunnel/src/testing.rs`:

```rust
//! An in-memory tunnel for tests in other crates: no WebSocket, no agent
//! binary, the same yamux session and forwarding.

use crate::{forward, Role, Session, StreamHandle, Target};

/// The server side of a tunnel whose agent side forwards every stream to
/// `target`. The agent side runs until the server side closes.
pub fn loopback(target: Target) -> Session {
    loopback_with(move |stream| {
        let target = target.clone();
        tokio::spawn(async move {
            if let Err(e) = forward(stream, &target).await {
                tracing::debug!(error = %e, "loopback forward ended");
            }
        });
    })
}

/// The server side of a tunnel whose agent side hands every stream to
/// `serve`, as the agent's kube proxy does.
pub fn loopback_with(serve: impl FnMut(StreamHandle) + Send + 'static) -> Session {
    let (server_io, agent_io) = tokio::io::duplex(1 << 20);
    let _agent = Session::spawn(agent_io, Role::Client, serve);
    Session::spawn(server_io, Role::Server, |_unexpected| {})
}
```

- [ ] **Step 5: The kind cluster and its fixtures**

Create `scripts/kube-it-fixtures.yaml`. Every later cluster test relies on these objects:

```yaml
apiVersion: v1
kind: Namespace
metadata: { name: fleetops-it }
---
apiVersion: v1
kind: ServiceAccount
metadata: { name: fleetops-it, namespace: fleetops-it }
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata: { name: fleetops-it }
roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: cluster-admin }
subjects: [{ kind: ServiceAccount, name: fleetops-it, namespace: fleetops-it }]
---
# Can read pods in fleetops-it only: lists elsewhere are 403.
apiVersion: v1
kind: ServiceAccount
metadata: { name: fleetops-it-narrow, namespace: fleetops-it }
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata: { name: pods-read, namespace: fleetops-it }
rules: [{ apiGroups: [""], resources: [pods], verbs: [get, list] }]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata: { name: narrow-pods-read, namespace: fleetops-it }
roleRef: { apiGroup: rbac.authorization.k8s.io, kind: Role, name: pods-read }
subjects: [{ kind: ServiceAccount, name: fleetops-it-narrow, namespace: fleetops-it }]
---
# Two containers; `main` is first, so it is the default for logs and exec.
apiVersion: v1
kind: Pod
metadata: { name: ticker, namespace: fleetops-it, labels: { app: ticker } }
spec:
  terminationGracePeriodSeconds: 1
  containers:
    - name: main
      image: busybox:1.37
      imagePullPolicy: IfNotPresent
      command: [sh, -c, 'i=0; while true; do echo "tick $i"; i=$((i+1)); sleep 1; done']
    - name: side
      image: busybox:1.37
      imagePullPolicy: IfNotPresent
      command: [sh, -c, 'while true; do echo side; sleep 5; done']
---
apiVersion: apps/v1
kind: Deployment
metadata: { name: web, namespace: fleetops-it }
spec:
  replicas: 2
  selector: { matchLabels: { app: web } }
  template:
    metadata: { labels: { app: web } }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: web
          image: busybox:1.37
          imagePullPolicy: IfNotPresent
          command: [sh, -c, 'while true; do echo serving; sleep 10; done']
---
apiVersion: apps/v1
kind: StatefulSet
metadata: { name: db, namespace: fleetops-it }
spec:
  replicas: 1
  serviceName: db
  selector: { matchLabels: { app: db } }
  template:
    metadata: { labels: { app: db } }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - { name: db, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [sleep, "3600"] }
---
apiVersion: apps/v1
kind: DaemonSet
metadata: { name: agentd, namespace: fleetops-it }
spec:
  selector: { matchLabels: { app: agentd } }
  template:
    metadata: { labels: { app: agentd } }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - { name: agentd, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [sleep, "3600"] }
---
apiVersion: batch/v1
kind: Job
metadata: { name: once, namespace: fleetops-it }
spec:
  template:
    spec:
      restartPolicy: Never
      containers:
        - { name: once, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [echo, done] }
---
apiVersion: batch/v1
kind: CronJob
metadata: { name: nightly, namespace: fleetops-it }
spec:
  schedule: "0 3 * * *"
  suspend: true
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers:
            - { name: nightly, image: busybox:1.37, imagePullPolicy: IfNotPresent, command: [echo, nightly] }
---
apiVersion: v1
kind: Service
metadata: { name: web, namespace: fleetops-it }
spec:
  selector: { app: web }
  ports: [{ name: http, port: 80, targetPort: 8080 }]
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata: { name: web, namespace: fleetops-it }
spec:
  rules:
    - host: web.example.test
      http:
        paths:
          - path: /
            pathType: Prefix
            backend: { service: { name: web, port: { number: 80 } } }
---
apiVersion: v1
kind: ConfigMap
metadata: { name: settings, namespace: fleetops-it }
data: { mode: production }
---
# Applied with `kubectl apply`, so its last-applied annotation holds the value.
apiVersion: v1
kind: Secret
metadata: { name: creds, namespace: fleetops-it }
stringData: { password: hunter2-do-not-leak }
```

Create `scripts/kind-up.sh` (`chmod +x`):

```sh
#!/bin/sh
# Creates (or reuses) the kind cluster the Kubernetes tests run against and
# writes what they need into $1 (default target/kube-it): kubeconfig,
# server, ca.crt, token (cluster-admin) and narrow-token (pods in
# fleetops-it only).
set -eu
name=${KIND_CLUSTER:-fleetops-test}
out=${1:-target/kube-it}
for tool in kind kubectl docker; do
  command -v "$tool" >/dev/null || { echo "$tool is required (kind: brew install kind)" >&2; exit 1; }
done
kind get clusters 2>/dev/null | grep -qx "$name" || kind create cluster --name "$name" --wait 120s
mkdir -p "$out"
kind get kubeconfig --name "$name" > "$out/kubeconfig"
export KUBECONFIG="$out/kubeconfig"
docker pull -q busybox:1.37 >/dev/null
kind load docker-image busybox:1.37 --name "$name" >/dev/null
kubectl apply -f "$(dirname "$0")/kube-it-fixtures.yaml" >/dev/null
kubectl -n fleetops-it rollout status deploy/web --timeout=120s >/dev/null
kubectl -n fleetops-it wait --for=condition=Ready pod/ticker --timeout=120s >/dev/null
kubectl -n fleetops-it create token fleetops-it --duration=2h > "$out/token"
kubectl -n fleetops-it create token fleetops-it-narrow --duration=2h > "$out/narrow-token"
kubectl config view --raw -o jsonpath='{.clusters[0].cluster.certificate-authority-data}' | base64 -d > "$out/ca.crt"
kubectl config view --raw -o jsonpath='{.clusters[0].cluster.server}' > "$out/server"
echo "kind cluster $name ready; fixtures in $out"
```

Add to `Makefile` (next to `test-docker`):

```make
KUBE_IT ?= $(CURDIR)/target/kube-it

kind-up:
	scripts/kind-up.sh $(KUBE_IT)

test-kube: kind-up
	FLEETOPS_KUBE_IT=$(KUBE_IT) cargo test -p fleetops-kube --features kube-it --test kube_it -- --test-threads=4
```

and add `kind-up test-kube` to the `.PHONY` line.

- [ ] **Step 6: The `fleetops-kube` crate with the tunnel client**

`crates/fleetops-kube/Cargo.toml`:

```toml
[package]
name = "fleetops-kube"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[features]
# The cluster suite: `make test-kube`.
kube-it = []

[dependencies]
fleetops-core.workspace = true
fleetops-tunnel.workspace = true
hyper.workspace = true
hyper-util.workspace = true
k8s-openapi.workspace = true
kube.workspace = true
tower.workspace = true

[dev-dependencies]
fleetops-agent = { path = "../fleetops-agent" }
futures-util = { workspace = true, features = ["io", "sink"] }
rustls.workspace = true
tokio.workspace = true
```

`crates/fleetops-kube/src/lib.rs`:

```rust
//! Kubernetes environments on kube-rs: a client from a kubeconfig or
//! through an agent tunnel, and the `KubeOps` implementation over it.

pub mod tunnel;
```

`crates/fleetops-kube/src/tunnel.rs`:

```rust
//! kube-rs over an agent tunnel. Each HTTP connection is one yamux stream;
//! the agent adds the ServiceAccount token and TLS, so this side speaks
//! plain HTTP with no credentials (spec §4.3).

use fleetops_tunnel::{http::TunnelConnector, StreamOpener};
use hyper::Uri;
use hyper_util::{
    client::legacy::Client,
    rt::{TokioExecutor, TokioTimer},
};
use std::{sync::Arc, time::Duration};

/// Idle pooled streams close after this, as for Docker agents.
const POOL_IDLE: Duration = Duration::from_secs(30);
/// Never dialled: the connector ignores it. kube-rs needs an absolute base.
const BASE: &str = "http://kube-agent";

pub fn client(opener: Arc<dyn StreamOpener>) -> kube::Client {
    let http = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE)
        .pool_timer(TokioTimer::new())
        .build(TunnelConnector::new(opener));
    let service = tower::ServiceBuilder::new()
        .layer(kube::client::middleware::BaseUriLayer::new(Uri::from_static(BASE)))
        .service(http);
    kube::Client::new(service, "default")
}
```

- [ ] **Step 7: The agent's kube proxy, with a unit test for the path allowlist**

In `crates/fleetops-agent/Cargo.toml` `[dependencies]`, add:

```toml
bytes.workspace = true
http-body-util.workspace = true
hyper.workspace = true
hyper-rustls.workspace = true
hyper-util.workspace = true
```

In `crates/fleetops-agent/src/lib.rs`, add `pub mod kube_proxy;`.

`crates/fleetops-agent/src/kube_proxy.rs`:

```rust
//! `--kube` mode: each tunnel stream carries plain HTTP/1.1 meant for the
//! kube-apiserver. The agent forwards only Kubernetes API paths, replaces
//! any Authorization header with its own ServiceAccount token, and talks
//! TLS to the apiserver. Upgrades (exec) are spliced end to end.

use anyhow::Context as _;
use bytes::Bytes;
use fleetops_tunnel::StreamHandle;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::{
    convert::Infallible,
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

/// Where every pod finds its ServiceAccount credentials.
pub const SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
/// Projected tokens rotate; re-read the file this often.
pub const TOKEN_REFRESH: Duration = Duration::from_secs(60);

type Body = BoxBody<Bytes, hyper::Error>;

/// The apiserver to forward to and the credentials to use.
#[derive(Debug, Clone)]
pub struct KubeUpstream {
    /// e.g. `https://10.96.0.1:443`; no trailing slash.
    pub base: Uri,
    pub ca: PathBuf,
    pub token: PathBuf,
}

impl KubeUpstream {
    /// The apiserver as every pod sees it.
    pub fn in_cluster() -> anyhow::Result<Self> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .context("KUBERNETES_SERVICE_HOST is not set: --kube must run inside a pod")?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
        let host = if host.contains(':') { format!("[{host}]") } else { host };
        Ok(Self {
            base: format!("https://{host}:{port}").parse()?,
            ca: Path::new(SA_DIR).join("ca.crt"),
            token: Path::new(SA_DIR).join("token"),
        })
    }
}

/// Forwards only what kube-rs sends: the core and group APIs and
/// `/version`. Dot segments are refused so a path can't climb out.
pub fn allowed(path: &str) -> bool {
    let api = path == "/version"
        || path == "/api"
        || path == "/apis"
        || path.starts_with("/api/")
        || path.starts_with("/apis/");
    api && !path.split('/').any(|seg| seg == ".." || seg == ".")
}

pub struct KubeProxy {
    up: KubeUpstream,
    client: Client<HttpsConnector<HttpConnector>, Body>,
    token: RwLock<String>,
}

fn read_token(path: &Path) -> io::Result<String> {
    let t = std::fs::read_to_string(path)?.trim().to_string();
    if t.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token file is empty"));
    }
    Ok(t)
}

fn reply(status: StatusCode, message: &str) -> Response<Body> {
    let body = format!(r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":{message:?},"code":{}}}"#, status.as_u16());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed())
        .expect("static response")
}

impl KubeProxy {
    pub fn new(up: KubeUpstream) -> anyhow::Result<Arc<Self>> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(&up.ca)
            .with_context(|| format!("reading the cluster CA {}", up.ca.display()))?
        {
            roots.add(cert?)?;
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(TokioTimer::new())
            .build(https);
        let token = read_token(&up.token)
            .with_context(|| format!("reading the ServiceAccount token {}", up.token.display()))?;
        Ok(Arc::new(Self {
            up,
            client,
            token: RwLock::new(token),
        }))
    }

    /// Re-reads the token file; the old token stays in use on failure.
    pub fn refresh_token(&self) -> io::Result<()> {
        let t = read_token(&self.up.token)?;
        *self.token.write().unwrap() = t;
        Ok(())
    }

    pub fn spawn_refresh(self: &Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = me.refresh_token() {
                    tracing::warn!(error = %e, "cannot re-read the ServiceAccount token");
                }
            }
        })
    }

    /// Serves one tunnel stream: HTTP/1.1 requests, keep-alive, upgrades.
    pub async fn serve(self: Arc<Self>, stream: StreamHandle) -> Result<(), hyper::Error> {
        let svc = hyper::service::service_fn(move |req| {
            let me = self.clone();
            async move { Ok::<_, Infallible>(me.handle(req).await) }
        });
        hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), svc)
            .with_upgrades()
            .await
    }

    async fn handle(&self, mut req: Request<Incoming>) -> Response<Body> {
        if !allowed(req.uri().path()) {
            return reply(StatusCode::FORBIDDEN, "fleetops-agent forwards only Kubernetes API paths");
        }
        let pq = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
        let uri = format!("{}{pq}", self.up.base.to_string().trim_end_matches('/'));
        match uri.parse() {
            Ok(u) => *req.uri_mut() = u,
            Err(_) => return reply(StatusCode::BAD_REQUEST, "bad request path"),
        }
        let token = self.token.read().unwrap().clone();
        let headers = req.headers_mut();
        headers.remove(header::HOST);
        headers.remove(header::AUTHORIZATION);
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("token is a header value"),
        );
        let client_upgrade = req
            .headers()
            .contains_key(header::UPGRADE)
            .then(|| hyper::upgrade::on(&mut req));
        let req = req.map(|b| b.boxed());
        let mut resp = match self.client.request(req).await {
            Ok(r) => r,
            Err(e) => {
                return reply(StatusCode::BAD_GATEWAY, &format!("kube-apiserver unreachable: {e}"))
            }
        };
        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_up) = client_upgrade {
                let server_up = hyper::upgrade::on(&mut resp);
                tokio::spawn(async move {
                    match tokio::try_join!(client_up, server_up) {
                        Ok((a, b)) => {
                            let _ = tokio::io::copy_bidirectional(
                                &mut TokioIo::new(a),
                                &mut TokioIo::new(b),
                            )
                            .await;
                        }
                        Err(e) => tracing::debug!(error = %e, "upgrade splice failed"),
                    }
                });
            }
        }
        resp.map(|b| b.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::allowed;

    #[test]
    fn only_kubernetes_api_paths_are_forwarded() {
        for ok in ["/version", "/api", "/api/v1/pods", "/apis/apps/v1/namespaces/x/deployments"] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in ["/", "/healthz", "/metrics", "/logs/x", "/api/../metrics", "/apis/./x", "/apix"] {
            assert!(!allowed(bad), "{bad}");
        }
    }
}
```

Run: `cargo test -p fleetops-agent kube_proxy 2>&1 | tail -5`
Expected: `only_kubernetes_api_paths_are_forwarded ... ok`.

- [ ] **Step 8: Write the cluster test**

`crates/fleetops-kube/tests/common/mod.rs`:

```rust
//! The kind fixtures written by `scripts/kind-up.sh`, and clients for them.
#![allow(dead_code)]

use fleetops_agent::kube_proxy::{KubeProxy, KubeUpstream};
use std::{path::PathBuf, sync::Arc};

pub const NS: &str = "fleetops-it";

pub fn dir() -> PathBuf {
    let _ = rustls::crypto::ring::default_provider().install_default();
    PathBuf::from(std::env::var("FLEETOPS_KUBE_IT").expect("run through `make test-kube`"))
}

pub fn read(name: &str) -> String {
    std::fs::read_to_string(dir().join(name)).unwrap().trim().to_string()
}

/// A kube-rs client through an in-process tunnel whose agent side is the
/// real kube proxy, using the token file `token`.
pub fn tunnel_client_with(token: &str) -> kube::Client {
    let d = dir();
    let up = KubeUpstream {
        base: read("server").parse().unwrap(),
        ca: d.join("ca.crt"),
        token: d.join(token),
    };
    let proxy = KubeProxy::new(up).unwrap();
    let session = fleetops_tunnel::testing::loopback_with(move |stream| {
        let proxy = proxy.clone();
        tokio::spawn(async move {
            let _ = proxy.serve(stream).await;
        });
    });
    fleetops_kube::tunnel::client(Arc::new(session))
}

pub fn tunnel_client() -> kube::Client {
    tunnel_client_with("token")
}
```

`crates/fleetops-kube/tests/kube_it.rs`:

```rust
//! Against a real kind cluster: `make test-kube`.
#![cfg(feature = "kube-it")]

mod common;

use common::NS;
use futures_util::{AsyncBufReadExt, SinkExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, ListParams, LogParams, TerminalSize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn through_the_tunnel_lists_pods_follows_logs_and_resizes_exec() {
    let pods: Api<Pod> = Api::namespaced(common::tunnel_client(), NS);

    let names: Vec<String> = pods
        .list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .filter_map(|p| p.metadata.name)
        .collect();
    assert!(names.iter().any(|n| n == "ticker"), "{names:?}");

    // Two different lines prove the stream follows rather than ends.
    let lp = LogParams {
        container: Some("main".into()),
        follow: true,
        tail_lines: Some(1),
        ..LogParams::default()
    };
    let mut lines = pods.log_stream("ticker", &lp).await.unwrap().lines();
    let a = timeout(WAIT, lines.try_next()).await.unwrap().unwrap().unwrap();
    let b = timeout(WAIT, lines.try_next()).await.unwrap().unwrap().unwrap();
    assert!(a.starts_with("tick") && b.starts_with("tick") && a != b, "{a:?} {b:?}");

    // A TTY exec that sees the size we set: the upgrade survived both hops.
    let ap = AttachParams::interactive_tty().container("main");
    let mut p = pods.exec("ticker", vec!["sh"], &ap).await.unwrap();
    let mut size = p.terminal_size().unwrap();
    size.send(TerminalSize { width: 123, height: 45 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut stdin = p.stdin().unwrap();
    let mut stdout = p.stdout().unwrap();
    stdin.write_all(b"stty size; exit\n").await.unwrap();
    let mut out = Vec::new();
    timeout(WAIT, stdout.read_to_end(&mut out)).await.unwrap().unwrap();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("45 123"), "{out}");
}
```

- [ ] **Step 9: Run it**

Run: `make test-kube 2>&1 | tail -15`
Expected: `through_the_tunnel_lists_pods_follows_logs_and_resizes_exec ... ok`. The first run creates the cluster, which takes about a minute.

This test was written after the code, because there was no seam to watch it fail first. It is the design proof the spec requires: if it fails on the exec half, the upgrade splice is the problem. Debug with `RUST_LOG=debug`, and check whether the 101 reaches kube-rs and whether both `hyper::upgrade::on` futures resolve. If no fix exists inside this design, stop and tell your human partner.

To watch it fail for the right reason, temporarily make `allowed` return `false`. The list then fails with `403 ... forwards only Kubernetes API paths`. Revert that change.

- [ ] **Step 10: Full suite, lint, commit**

Run: `make lint && make test 2>&1 | tail -3`
Expected: lint clean; all tests pass.

```bash
git add Cargo.toml Cargo.lock Makefile scripts/kind-up.sh scripts/kube-it-fixtures.yaml crates/fleetops-tunnel crates/fleetops-docker/src/tunnel.rs crates/fleetops-docker/Cargo.toml crates/fleetops-kube crates/fleetops-agent
git commit -m "feat(kube): prove kube-rs through the agent tunnel against a kind cluster

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 2: Core Kubernetes types and the `KubeOps` trait

These are pure additions to `fleetops-core`: the closed kind enum, the DTOs the API returns, the trait `fleetops-kube` implements, and the credential types environments are built from.

**Files:**
- Create: `crates/fleetops-core/src/kube.rs`
- Modify: `crates/fleetops-core/src/lib.rs` (`pub mod kube;`), `crates/fleetops-core/src/env.rs` (add `Credential`, `KubeconfigSummary`, `UserKind`)
- Test: `crates/fleetops-core/tests/kube_types.rs`

**Interfaces:**
- Produces:
  - in `fleetops_core::kube`:
    - `KubeKind` (12 variants; `ALL`, `as_str`, `kind_name`, `from_kind_name`, `cluster_scoped`, `scalable`, `restartable`, `FromStr` with an unknown kind giving `Error::NotFound`);
    - `Health`, `KubeRow`, `KubeList`, `KubeCondition`, `KubeRef`, `KubeObject`, `KubeEvent`, `KubeVersion`, `KubeExec`;
    - `trait KubeOps`;
    - the constants `LIST_CAP`, `PAGE_SIZE`, `YAML_CAP`, `MASK`.
  - in `fleetops_core::env`: `Credential { Kubeconfig(String), Unreadable(String) }` (its `Debug` redacts the kubeconfig), `KubeconfigSummary { server, context, user_kind }` and `UserKind { Token, Cert }`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-core/tests/kube_types.rs`:

```rust
use fleetops_core::env::Credential;
use fleetops_core::kube::KubeKind;
use fleetops_core::Error;

#[test]
fn every_kind_round_trips_through_its_path_name() {
    assert_eq!(KubeKind::ALL.len(), 12);
    for k in KubeKind::ALL {
        assert_eq!(k.as_str().parse::<KubeKind>().unwrap(), k);
        assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        assert_eq!(KubeKind::from_kind_name(k.kind_name()), Some(k));
    }
    assert_eq!(KubeKind::StatefulSets.as_str(), "statefulsets");
    assert_eq!(KubeKind::ConfigMaps.kind_name(), "ConfigMap");
}

#[test]
fn an_unknown_kind_is_not_found() {
    assert!(matches!("replicasets".parse::<KubeKind>(), Err(Error::NotFound(_))));
}

#[test]
fn scope_and_actions_follow_the_spec() {
    use KubeKind::*;
    let cluster: Vec<_> = KubeKind::ALL.into_iter().filter(|k| k.cluster_scoped()).collect();
    assert_eq!(cluster, [Namespaces, Nodes]);
    let scalable: Vec<_> = KubeKind::ALL.into_iter().filter(|k| k.scalable()).collect();
    assert_eq!(scalable, [Deployments, StatefulSets]);
    let restartable: Vec<_> = KubeKind::ALL.into_iter().filter(|k| k.restartable()).collect();
    assert_eq!(restartable, [Deployments, StatefulSets, DaemonSets]);
}

#[test]
fn a_credential_never_prints_its_kubeconfig() {
    let c = Credential::Kubeconfig("token: s3cret".into());
    assert!(!format!("{c:?}").contains("s3cret"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-core --test kube_types 2>&1 | tail -5`
Expected: compile error, unresolved import `fleetops_core::kube`.

- [ ] **Step 3: Implement**

`crates/fleetops-core/src/kube.rs`:

```rust
//! Kubernetes domain types and the operations a Kubernetes environment
//! offers. No kube-rs here: `fleetops-kube` implements `KubeOps`.

use crate::streams::{BoxStream, ExecOptions, LogLine, LogOptions};
use crate::{Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, str::FromStr};
use tokio::sync::{mpsc, oneshot};
use utoipa::ToSchema;

/// Most rows one list returns; more sets `truncated` (spec §5).
pub const LIST_CAP: usize = 5000;
/// Rows asked of the apiserver per page.
pub const PAGE_SIZE: u32 = 500;
/// Largest YAML document returned; longer is cut with a note.
pub const YAML_CAP: usize = 1 << 20;
/// Shown in place of every Secret value.
pub const MASK: &str = "••••";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum KubeKind {
    Namespaces,
    Nodes,
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
    CronJobs,
    Pods,
    Services,
    Ingresses,
    ConfigMaps,
    Secrets,
}

impl KubeKind {
    pub const ALL: [KubeKind; 12] = [
        KubeKind::Namespaces,
        KubeKind::Nodes,
        KubeKind::Deployments,
        KubeKind::StatefulSets,
        KubeKind::DaemonSets,
        KubeKind::Jobs,
        KubeKind::CronJobs,
        KubeKind::Pods,
        KubeKind::Services,
        KubeKind::Ingresses,
        KubeKind::ConfigMaps,
        KubeKind::Secrets,
    ];

    /// The plural lowercase name used in URLs.
    pub fn as_str(self) -> &'static str {
        match self {
            KubeKind::Namespaces => "namespaces",
            KubeKind::Nodes => "nodes",
            KubeKind::Deployments => "deployments",
            KubeKind::StatefulSets => "statefulsets",
            KubeKind::DaemonSets => "daemonsets",
            KubeKind::Jobs => "jobs",
            KubeKind::CronJobs => "cronjobs",
            KubeKind::Pods => "pods",
            KubeKind::Services => "services",
            KubeKind::Ingresses => "ingresses",
            KubeKind::ConfigMaps => "configmaps",
            KubeKind::Secrets => "secrets",
        }
    }

    /// The Kubernetes `kind`, e.g. `Deployment`.
    pub fn kind_name(self) -> &'static str {
        match self {
            KubeKind::Namespaces => "Namespace",
            KubeKind::Nodes => "Node",
            KubeKind::Deployments => "Deployment",
            KubeKind::StatefulSets => "StatefulSet",
            KubeKind::DaemonSets => "DaemonSet",
            KubeKind::Jobs => "Job",
            KubeKind::CronJobs => "CronJob",
            KubeKind::Pods => "Pod",
            KubeKind::Services => "Service",
            KubeKind::Ingresses => "Ingress",
            KubeKind::ConfigMaps => "ConfigMap",
            KubeKind::Secrets => "Secret",
        }
    }

    pub fn from_kind_name(name: &str) -> Option<KubeKind> {
        KubeKind::ALL.into_iter().find(|k| k.kind_name() == name)
    }

    pub fn cluster_scoped(self) -> bool {
        matches!(self, KubeKind::Namespaces | KubeKind::Nodes)
    }

    pub fn scalable(self) -> bool {
        matches!(self, KubeKind::Deployments | KubeKind::StatefulSets)
    }

    pub fn restartable(self) -> bool {
        matches!(
            self,
            KubeKind::Deployments | KubeKind::StatefulSets | KubeKind::DaemonSets
        )
    }
}

impl FromStr for KubeKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        KubeKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| Error::NotFound(format!("unknown kind '{s}'")))
    }
}

/// The status dot's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warn,
    Bad,
    Unknown,
}

/// One object in a list: common fields plus columns that depend on its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeRow {
    pub name: String,
    pub namespace: Option<String>,
    /// Unix seconds.
    pub created: i64,
    /// e.g. "Running", "2/3", "Complete".
    pub status: String,
    pub health: Health,
    /// Kind-specific: ready, restarts, node, type, ports, hosts…
    pub columns: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeList {
    pub rows: Vec<KubeRow>,
    /// More objects exist than `LIST_CAP`.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

/// An owner, e.g. a Pod's ReplicaSet. `link` is set when FleetOps has a
/// page for the owner's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeRef {
    pub kind: String,
    pub name: String,
    pub link: Option<KubeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeObject {
    pub row: KubeRow,
    /// `managedFields` stripped; Secret values masked.
    pub yaml: String,
    pub conditions: Vec<KubeCondition>,
    /// Nearest first: Pod → ReplicaSet → Deployment.
    pub owners: Vec<KubeRef>,
    /// Pods only: container names, first is the default.
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeEvent {
    /// "Normal" or "Warning".
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i32,
    /// Unix seconds.
    pub last_seen: Option<i64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeVersion {
    pub git_version: String,
    pub platform: String,
    pub nodes: u32,
    pub nodes_ready: u32,
}

/// A running TTY exec in a pod.
pub struct KubeExec {
    /// Raw TTY output; slow reads are backpressure.
    pub output: BoxStream<Vec<u8>>,
    /// Dropping it closes stdin.
    pub input: mpsc::Sender<Vec<u8>>,
    /// (cols, rows).
    pub resize: mpsc::Sender<(u16, u16)>,
    /// Resolves when the process ends; `None` when the code is unknown.
    pub exit_code: oneshot::Receiver<Option<i64>>,
}

#[async_trait]
pub trait KubeOps: Send + Sync {
    async fn version(&self) -> Result<KubeVersion>;
    /// `ns` is ignored for cluster-scoped kinds; `None` lists all namespaces.
    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList>;
    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject>;
    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>>;
    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()>;
    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()>;
    /// `opts.container` `None` means the pod's first container.
    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine>;
    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec>;
}
```

At this point `LogOptions` and `ExecOptions` have no `container` field. Task 4 adds it. The trait only passes the options through, so this task compiles without it.

In `crates/fleetops-core/src/lib.rs`, add `pub mod kube;` after `pub mod env;`.

Append to `crates/fleetops-core/src/env.rs`:

```rust
/// A stored secret an environment is built from, already decrypted by
/// the store. `Unreadable` carries why it could not be decrypted.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Kubeconfig(String),
    Unreadable(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Kubeconfig(_) => f.write_str("Kubeconfig(<redacted>)"),
            Credential::Unreadable(why) => write!(f, "Unreadable({why:?})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserKind {
    Token,
    Cert,
}

/// What may be shown about a stored kubeconfig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KubeconfigSummary {
    pub server: String,
    pub context: String,
    pub user_kind: UserKind,
}
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-core 2>&1 | tail -5`
Expected: all pass, including the 4 new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-core
git commit -m "feat(core): Kubernetes kinds, DTOs and the KubeOps trait

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 3: Encrypted credentials in the store

**Files:**
- Create: `crates/fleetops-store/src/secrets.rs`, `crates/fleetops-store/src/credentials.rs`, `crates/fleetops-store/migrations/0003_env_credentials.sql`
- Modify: `crates/fleetops-store/src/lib.rs` (`Store` gains `secrets`; `with_secret_key`; `pub mod secrets; mod credentials;`), `crates/fleetops-store/Cargo.toml`
- Test: unit tests in `secrets.rs`; `crates/fleetops-store/tests/credentials.rs`

**Interfaces:**
- Consumes: `fleetops_core::env::Credential` (Task 2).
- Produces:
  - `fleetops_store::secrets::SecretKey` with `random() -> SecretKey`, `from_base64(&str) -> Result<SecretKey>` and `load_or_create(&Path) -> Result<(SecretKey, bool)>` (`true` when the key was created).
  - `fleetops_store::secrets::SecretBox` with `new(&SecretKey)`, `seal(&self, aad: &[u8], plain: &[u8]) -> String` and `open(&self, aad: &[u8], sealed: &str) -> Result<Vec<u8>>`.
  - `Store::with_secret_key(self, key: &SecretKey) -> Store`.
  - `Store::create_environment_with_credential(&self, name: &str, kind: &str, url: &str, cred_kind: &str, plaintext: &str, summary: &serde_json::Value, now: i64) -> Result<EnvironmentRecord>`.
  - `Store::put_env_credential(&self, env_id: i64, cred_kind: &str, plaintext: &str, summary: &Value, now: i64) -> Result<()>`.
  - `Store::env_credential(&self, env_id: i64) -> Result<Option<Credential>>`.
  - `Store::env_credential_summaries(&self) -> Result<HashMap<i64, Value>>`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/fleetops-store/Cargo.toml` `[dependencies]`: `aes-gcm.workspace = true`, `base64.workspace = true` and `rand.workspace = true`.

`crates/fleetops-store/src/secrets.rs` (tests only for now, at the bottom of an empty module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_text_opens_with_the_same_key_and_aad() {
        let b = SecretBox::new(&SecretKey::random());
        let sealed = b.seal(b"7", b"token: abc");
        assert!(sealed.starts_with("v1:"), "{sealed}");
        assert!(!sealed.contains("abc"));
        assert_eq!(b.open(b"7", &sealed).unwrap(), b"token: abc");
    }

    #[test]
    fn another_key_another_aad_or_a_tampered_text_fails() {
        let b = SecretBox::new(&SecretKey::random());
        let sealed = b.seal(b"7", b"x");
        assert!(SecretBox::new(&SecretKey::random()).open(b"7", &sealed).is_err());
        assert!(b.open(b"8", &sealed).is_err());
        let mut t = sealed.clone().into_bytes();
        let last = t.len() - 2;
        t[last] = if t[last] == b'A' { b'B' } else { b'A' };
        assert!(b.open(b"7", std::str::from_utf8(&t).unwrap()).is_err());
        assert!(b.open(b"7", "v2:abc").is_err());
    }

    #[test]
    fn a_key_file_is_created_owner_only_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        let (a, created) = SecretKey::load_or_create(&path).unwrap();
        assert!(created);
        let (b, created) = SecretKey::load_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(a.0, b.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_key_must_be_32_bytes_of_base64() {
        assert!(SecretKey::from_base64("c2hvcnQ=").is_err());
        assert!(SecretKey::from_base64("not base64!").is_err());
        let k = SecretKey::random();
        use base64::Engine;
        let text = base64::engine::general_purpose::STANDARD.encode(k.0);
        assert_eq!(SecretKey::from_base64(&text).unwrap().0, k.0);
    }
}
```

`crates/fleetops-store/tests/credentials.rs`:

```rust
use fleetops_core::env::Credential;
use fleetops_store::{secrets::SecretKey, Store};
use serde_json::json;

const KUBECONFIG: &str = "apiVersion: v1\nusers: [{name: u, user: {token: s3cret}}]\n";

#[tokio::test]
async fn a_credential_round_trips_and_is_stored_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("f.db");
    let key = SecretKey::random();
    let store = Store::open(&db).await.unwrap().with_secret_key(&key);
    let summary = json!({"server": "https://k:6443", "context": "c", "user_kind": "token"});
    let rec = store
        .create_environment_with_credential("k", "kube-config", "https://k:6443", "kubeconfig", KUBECONFIG, &summary, 1)
        .await
        .unwrap();
    assert_eq!(rec.kind, "kube-config");
    assert_eq!(
        store.env_credential(rec.id).await.unwrap(),
        Some(Credential::Kubeconfig(KUBECONFIG.into()))
    );
    assert_eq!(store.env_credential_summaries().await.unwrap()[&rec.id], summary);
    drop(store);

    // The file on disk never holds the plaintext.
    let raw = std::fs::read(&db).unwrap();
    assert!(!raw.windows(6).any(|w| w == b"s3cret"));

    // Another key cannot read it, and says so instead of failing.
    let other = Store::open(&db).await.unwrap().with_secret_key(&SecretKey::random());
    assert!(matches!(other.env_credential(rec.id).await.unwrap(), Some(Credential::Unreadable(_))));
}

#[tokio::test]
async fn replacing_and_deleting_follow_the_environment() {
    let store = Store::in_memory().await.unwrap();
    let rec = store
        .create_environment_with_credential("k", "kube-config", "https://a", "kubeconfig", "one", &json!({}), 1)
        .await
        .unwrap();
    store.put_env_credential(rec.id, "kubeconfig", "two", &json!({"v": 2}), 2).await.unwrap();
    assert_eq!(store.env_credential(rec.id).await.unwrap(), Some(Credential::Kubeconfig("two".into())));
    store.delete_environment(rec.id).await.unwrap();
    assert_eq!(store.env_credential(rec.id).await.unwrap(), None);
    assert!(store.env_credential_summaries().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_docker_environment_has_no_credential() {
    let store = Store::in_memory().await.unwrap();
    let rec = store.create_environment("d", "docker-local", "unix:///x.sock", 1).await.unwrap();
    assert_eq!(store.env_credential(rec.id).await.unwrap(), None);
}
```

If `delete_environment` returns something other than `Result<()>`, adapt the call; the test is about the cascade.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-store 2>&1 | tail -5`
Expected: compile errors, because `SecretBox`, `SecretKey` and `with_secret_key` are not defined.

- [ ] **Step 3: Implement**

`crates/fleetops-store/migrations/0003_env_credentials.sql`:

```sql
-- Sub-project 3: secrets an environment is built from (kubeconfigs),
-- encrypted with the server key. `summary` is safe to show.
CREATE TABLE env_credentials (
  env_id     INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  ciphertext TEXT NOT NULL,
  summary    TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
```

Top of `crates/fleetops-store/src/secrets.rs`, above the tests:

```rust
//! The server's secret key and the AES-256-GCM box sealed with it
//! (spec §3.1). Sealed text is `v1:` + base64(nonce ‖ ciphertext ‖ tag);
//! the caller's AAD (the environment id) binds it to its row.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use fleetops_core::{Error, Result};
use rand::RngCore;
use std::{io::Write, path::Path};

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

pub struct SecretKey(pub(crate) [u8; 32]);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl SecretKey {
    pub fn random() -> Self {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        Self(k)
    }

    pub fn from_base64(text: &str) -> Result<Self> {
        let bad = || Error::Invalid("FLEETOPS_SECRET_KEY must be 32 bytes, base64-encoded".into());
        let bytes = STANDARD.decode(text.trim()).map_err(|_| bad())?;
        let k: [u8; 32] = bytes.try_into().map_err(|_| bad())?;
        Ok(Self(k))
    }

    /// Reads the key file, or creates it (mode 0600) when it is missing.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        let io = |e: std::io::Error| Error::Internal(format!("secret key {}: {e}", path.display()));
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((Self::from_base64(&text)?, false)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::random();
                let mut opts = std::fs::OpenOptions::new();
                opts.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600);
                }
                let mut f = opts.open(path).map_err(io)?;
                writeln!(f, "{}", STANDARD.encode(key.0)).map_err(io)?;
                f.sync_all().map_err(io)?;
                Ok((key, true))
            }
            Err(e) => Err(io(e)),
        }
    }
}

pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn new(key: &SecretKey) -> Self {
        Self {
            cipher: Aes256Gcm::new(&key.0.into()),
        }
    }

    pub fn seal(&self, aad: &[u8], plain: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
            .expect("AES-GCM encryption does not fail for in-memory input");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        format!("{PREFIX}{}", STANDARD.encode(out))
    }

    pub fn open(&self, aad: &[u8], sealed: &str) -> Result<Vec<u8>> {
        let fail = || Error::Invalid("credential cannot be decrypted; re-enter the kubeconfig".into());
        let body = sealed.strip_prefix(PREFIX).ok_or_else(fail)?;
        let bytes = STANDARD.decode(body).map_err(|_| fail())?;
        if bytes.len() < NONCE_LEN {
            return Err(fail());
        }
        let (nonce, ct) = bytes.split_at(NONCE_LEN);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| fail())
    }
}
```

In `crates/fleetops-store/src/lib.rs`:
- add `pub mod secrets;` and `mod credentials;`;
- add the field `secrets: std::sync::Arc<secrets::SecretBox>` to `Store`;
- make the constructor that builds `Store` (the shared `init(pool)`) set `secrets: Arc::new(SecretBox::new(&SecretKey::random()))`;
- add:

```rust
    /// Seals and opens credentials with `key` instead of the random
    /// per-process key every store starts with.
    pub fn with_secret_key(mut self, key: &secrets::SecretKey) -> Self {
        self.secrets = std::sync::Arc::new(secrets::SecretBox::new(key));
        self
    }
```

If `Store` derives `Clone`, the `Arc` keeps it cheap.

`crates/fleetops-store/src/credentials.rs`:

```rust
//! Encrypted environment credentials (spec §3). The store seals and opens
//! them, so callers see plaintext or `Credential::Unreadable` only.

use crate::{db_err, EnvironmentRecord, Store};
use fleetops_core::env::Credential;
use fleetops_core::Result;
use serde_json::Value;
use std::collections::HashMap;

fn aad(env_id: i64) -> Vec<u8> {
    env_id.to_string().into_bytes()
}

impl Store {
    /// Creates the environment and its credential in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_environment_with_credential(
        &self,
        name: &str,
        kind: &str,
        url: &str,
        cred_kind: &str,
        plaintext: &str,
        summary: &Value,
        now: i64,
    ) -> Result<EnvironmentRecord> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let rec = crate::environments::insert_environment(&mut tx, name, kind, url, now).await?;
        let sealed = self.secrets.seal(&aad(rec.id), plaintext.as_bytes());
        sqlx::query("INSERT INTO env_credentials (env_id, kind, ciphertext, summary, updated_at) VALUES (?, ?, ?, ?, ?)")
            .bind(rec.id)
            .bind(cred_kind)
            .bind(sealed)
            .bind(summary.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(rec)
    }

    pub async fn put_env_credential(
        &self,
        env_id: i64,
        cred_kind: &str,
        plaintext: &str,
        summary: &Value,
        now: i64,
    ) -> Result<()> {
        let sealed = self.secrets.seal(&aad(env_id), plaintext.as_bytes());
        sqlx::query(
            "INSERT INTO env_credentials (env_id, kind, ciphertext, summary, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(env_id) DO UPDATE SET kind = excluded.kind, ciphertext = excluded.ciphertext,
               summary = excluded.summary, updated_at = excluded.updated_at",
        )
        .bind(env_id)
        .bind(cred_kind)
        .bind(sealed)
        .bind(summary.to_string())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn env_credential(&self, env_id: i64) -> Result<Option<Credential>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT ciphertext FROM env_credentials WHERE env_id = ?")
                .bind(env_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(row.map(|(sealed,)| match self.secrets.open(&aad(env_id), &sealed) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Credential::Kubeconfig(text),
                Err(_) => Credential::Unreadable("credential is not UTF-8".into()),
            },
            Err(e) => Credential::Unreadable(e.to_string()),
        }))
    }

    pub async fn env_credential_summaries(&self) -> Result<HashMap<i64, Value>> {
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT env_id, summary FROM env_credentials")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, s)| serde_json::from_str(&s).ok().map(|v| (id, v)))
            .collect())
    }
}
```

`create_environment` in `crates/fleetops-store/src/environments.rs` currently runs its own `INSERT`. Extract the insert into `pub(crate) async fn insert_environment(conn: &mut sqlx::SqliteConnection, name, kind, url, now) -> Result<EnvironmentRecord>`, keeping its duplicate-name → `Conflict` mapping. Make `create_environment` call it with a pooled connection (`&mut *self.pool.acquire().await.map_err(db_err)?`). If `db_err` is not `pub(crate)` yet, make it so.

`Error::Invalid` from `open` has the `invalid input: ` prefix in its `Display`. `env_credential` maps it into `Credential::Unreadable(e.to_string())`. Task 8 shows the spec's exact message for any `Unreadable`, so the prefix never reaches a user.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-store 2>&1 | tail -5`
Expected: all pass (the 4 secrets unit tests, the 3 credentials tests, and the existing tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-store
git commit -m "feat(store): AES-256-GCM sealed environment credentials

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 4: Environment plumbing (kinds, versions, specs, authz, options)

This task widens the core contracts every later task builds on:
- the new environment kinds;
- a status version that can describe a cluster;
- a factory that takes an `EnvSpec` carrying the credential;
- a registry that reads credentials from the store;
- `KubeRead` and `KubeWrite`;
- `container` on the log and exec options.

Nothing creates a Kubernetes environment yet. Creating one is refused until Task 11.

**Files:**
- Modify: `crates/fleetops-core/src/env.rs`, `crates/fleetops-core/src/streams.rs`, `crates/fleetops-core/tests/docker_types.rs`
- Modify: `crates/fleetops-docker/src/env.rs`, `crates/fleetops-docker/src/agent_env.rs`, `crates/fleetops-docker/tests/docker_it.rs`
- Modify: `crates/fleetops-api/src/envs.rs`, `crates/fleetops-api/src/routes/envs.rs`, `crates/fleetops-api/src/ws/hub.rs`, `crates/fleetops-api/src/authz.rs`, `crates/fleetops-api/tests/common/fake_docker.rs`
- Modify: `crates/fleetops-api/openapi.json` and `web/src/api/gen/*` (regenerated), plus the web fixtures `web/src/test/app.tsx`, `web/src/api/queries.test.ts` and `web/src/pages/DashboardPage.test.tsx`
- Test: `crates/fleetops-core/tests/env_kinds.rs` (new); `crates/fleetops-api/tests/envs_credentials.rs` (new)

**Interfaces:**
- Consumes: `Credential` (Task 2), `Store::env_credential` (Task 3).
- Produces:
  - `EnvKind::{KubeConfig, KubeAgent}` (`"kube-config"`, `"kube-agent"`), with `EnvKind::is_kube(self) -> bool`.
  - `EnvVersion { Docker(EngineVersion), Kube(KubeVersion) }`, serialised with a `kind` tag (`docker` | `kube`).
  - `EnvStatus::Up { version: EnvVersion }`.
  - `EnvSpec { id, name, kind, url, credential: Option<Credential> }`, `EnvFactory::build(&self, spec: &EnvSpec)`, and `Environment::kube(&self) -> Option<Arc<dyn KubeOps>>` (default `None`).
  - `EnvRegistry::insert(&self, rec: &EnvironmentRecord, credential: Option<Credential>)` and `EnvRegistry::replace(&self, old_name: &str, rec: &EnvironmentRecord, credential: Option<Credential>)`. `EnvRegistry::load` reads credentials.
  - `Action::{KubeRead, KubeWrite}`.
  - `LogOptions.container: Option<String>` and `ExecOptions.container: Option<String>`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-core/tests/env_kinds.rs`:

```rust
use fleetops_core::docker::EngineVersion;
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion};
use fleetops_core::kube::KubeVersion;
use serde_json::json;

#[test]
fn kube_kinds_parse_and_print() {
    for (k, s) in [
        (EnvKind::DockerLocal, "docker-local"),
        (EnvKind::DockerAgent, "docker-agent"),
        (EnvKind::KubeConfig, "kube-config"),
        (EnvKind::KubeAgent, "kube-agent"),
    ] {
        assert_eq!(k.as_str(), s);
        assert_eq!(s.parse::<EnvKind>().unwrap(), k);
        assert_eq!(k.is_kube(), s.starts_with("kube-"));
    }
}

#[test]
fn an_up_status_tags_its_version() {
    let docker = EnvStatus::Up {
        version: EnvVersion::Docker(EngineVersion {
            version: "27.0".into(),
            api_version: "1.47".into(),
            os: "linux".into(),
            arch: "amd64".into(),
        }),
    };
    assert_eq!(
        serde_json::to_value(&docker).unwrap(),
        json!({"state": "up", "version": {"kind": "docker", "version": "27.0", "api_version": "1.47", "os": "linux", "arch": "amd64"}})
    );
    let kube = EnvStatus::Up {
        version: EnvVersion::Kube(KubeVersion {
            git_version: "v1.32.0".into(),
            platform: "linux/amd64".into(),
            nodes: 3,
            nodes_ready: 2,
        }),
    };
    assert_eq!(
        serde_json::to_value(&kube).unwrap(),
        json!({"state": "up", "version": {"kind": "kube", "git_version": "v1.32.0", "platform": "linux/amd64", "nodes": 3, "nodes_ready": 2}})
    );
}
```

`crates/fleetops-api/tests/envs_credentials.rs`:

```rust
mod common;

use fleetops_core::env::{Credential, EnvKind};
use serde_json::json;

/// The registry builds each environment with its decrypted credential.
#[tokio::test]
async fn load_hands_a_kube_environment_its_kubeconfig() {
    let t = common::app().await;
    let rec = t
        .store
        .create_environment_with_credential("k", "kube-config", "https://k:6443", "kubeconfig", "token: x", &json!({}), 1)
        .await
        .unwrap();
    t.state.envs.load(&t.store).await.unwrap();
    let built = t.fake.built();
    let spec = built.iter().find(|s| s.id == rec.id).expect("built");
    assert_eq!(spec.kind, EnvKind::KubeConfig);
    assert_eq!(spec.credential, Some(Credential::Kubeconfig("token: x".into())));
}

#[tokio::test]
async fn kube_environments_cannot_be_created_yet() {
    let t = common::app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(common::req("POST", "/api/v1/envs", Some(json!({"name": "k", "kind": "kube-config"})), Some(&admin)))
        .await;
    assert_eq!(r.status, 400, "{:?}", r.json);
}
```

Match `common::req`'s real signature in `tests/common/mod.rs`. The test sends an admin `POST /api/v1/envs` with that body.

Add to `crates/fleetops-api/src/authz.rs` tests:

```rust
    #[test]
    fn users_and_admins_may_read_and_change_kubernetes() {
        for role in [Role::Admin, Role::User] {
            assert!(authorize(&user(role), Some(1), Action::KubeRead).is_ok());
            assert!(authorize(&user(role), Some(1), Action::KubeWrite).is_ok());
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-core --test env_kinds 2>&1 | tail -3; cargo test -p fleetops-api --test envs_credentials 2>&1 | tail -3`
Expected: compile errors, because `EnvVersion`, `EnvKind::KubeConfig`, `FakeDocker::built` and `Action::KubeRead` don't exist yet.

- [ ] **Step 3: Implement the core changes**

In `crates/fleetops-core/src/env.rs`:

```rust
use crate::kube::{KubeOps, KubeVersion};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EnvKind {
    DockerLocal,
    /// A remote Docker host reached through a `fleetops-agent` tunnel.
    DockerAgent,
    /// A cluster reached directly with a stored kubeconfig.
    KubeConfig,
    /// A cluster reached through an in-cluster `fleetops-agent --kube`.
    KubeAgent,
}

impl EnvKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvKind::DockerLocal => "docker-local",
            EnvKind::DockerAgent => "docker-agent",
            EnvKind::KubeConfig => "kube-config",
            EnvKind::KubeAgent => "kube-agent",
        }
    }

    pub fn is_kube(self) -> bool {
        matches!(self, EnvKind::KubeConfig | EnvKind::KubeAgent)
    }
}
```

Extend `FromStr` with `"kube-config" => Ok(EnvKind::KubeConfig)` and `"kube-agent" => Ok(EnvKind::KubeAgent)`.

Replace `EnvStatus`:

```rust
/// What an up environment reports about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EnvVersion {
    Docker(EngineVersion),
    Kube(KubeVersion),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum EnvStatus {
    Up { version: EnvVersion },
    Down { reason: String },
}
```

Add `fn kube(&self) -> Option<Arc<dyn KubeOps>> { None }` to `trait Environment`, after `docker()`, with the doc comment `/// Kubernetes operations, for Kubernetes-kind environments that can be reached.`

Replace `EnvFactory`:

```rust
/// Everything a factory needs to build one live environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSpec {
    pub id: i64,
    pub name: String,
    pub kind: EnvKind,
    pub url: String,
    pub credential: Option<Credential>,
}

/// Builds live environments from stored records. The server supplies the
/// real factory; tests supply fakes.
pub trait EnvFactory: Send + Sync {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>>;
}
```

In `crates/fleetops-core/src/streams.rs`, add `/// Kubernetes only: the container; \`None\` is the pod's first. Docker ignores it.` and `pub container: Option<String>,` to both `LogOptions` and `ExecOptions`, with `container: None` in both `Default` impls.

- [ ] **Step 4: Follow the compiler through the call sites**

Run: `cargo check --workspace --all-targets 2>&1 | grep -E "^error" -A3 | head -60`. Fix each site:

- `crates/fleetops-docker/src/env.rs`:
  - `Ok(version) => EnvStatus::Up { version: EnvVersion::Docker(version) }`.
  - `DockerEnvFactory::build(&self, spec: &EnvSpec)` matches `spec.kind`, uses `spec.id`, `&spec.name` and `&spec.url`, and returns this for the kube kinds:
    ```rust
    EnvKind::KubeConfig | EnvKind::KubeAgent => Err(Error::Invalid(format!(
        "{} is not a Docker environment kind",
        spec.kind.as_str()
    ))),
    ```
  - Update the unit test to build an `EnvSpec`.
- `crates/fleetops-docker/src/agent_env.rs`: `EnvStatus::Up { version: EnvVersion::Docker(version) }`.
- `crates/fleetops-docker/tests/docker_it.rs`: build an `EnvSpec { id, name: name.into(), kind, url: url.into(), credential: None }` at both `build(` call sites.
- `crates/fleetops-api/src/ws/hub.rs`: only matches `EnvStatus::Up { .. }`, so no change is expected.
- `crates/fleetops-api/src/envs.rs`:

```rust
    /// Builds every stored environment, each with its credential.
    pub async fn load(&self, store: &Store) -> Result<()> {
        for rec in store.list_environments().await? {
            let credential = store.env_credential(rec.id).await?;
            self.insert(&rec, credential)?;
        }
        Ok(())
    }

    fn spec(rec: &EnvironmentRecord, credential: Option<Credential>) -> Result<EnvSpec> {
        Ok(EnvSpec {
            id: rec.id,
            name: rec.name.clone(),
            kind: rec.kind.parse()?,
            url: rec.url.clone(),
            credential,
        })
    }

    /// Adds or replaces the live environment for `rec`.
    pub fn insert(&self, rec: &EnvironmentRecord, credential: Option<Credential>) -> Result<Arc<dyn Environment>> {
        let env = self.factory.build(&Self::spec(rec, credential)?)?;
        self.envs.write().unwrap().insert(rec.name.clone(), env.clone());
        Ok(env)
    }

    pub fn replace(&self, old_name: &str, rec: &EnvironmentRecord, credential: Option<Credential>) -> Result<Arc<dyn Environment>> {
        let env = self.factory.build(&Self::spec(rec, credential)?)?;
        let mut envs = self.envs.write().unwrap();
        envs.remove(old_name);
        envs.insert(rec.name.clone(), env.clone());
        Ok(env)
    }
```

  `bootstrap_local` builds `EnvSpec { id: 0, name: LOCAL_ENV_NAME.into(), kind: EnvKind::DockerLocal, url: url.into(), credential: None }` and calls `registry.insert(&rec, None)`.
- `crates/fleetops-api/src/routes/envs.rs`:
  - `create` calls `s.envs.insert(&rec, None)?`. The `match body.kind` gains:
    ```rust
    EnvKind::KubeConfig | EnvKind::KubeAgent => {
        return Err(Error::Invalid("Kubernetes environments are not available yet".into()).into())
    }
    ```
    Task 11 replaces it.
  - `update` passes `s.store.env_credential(rec.id).await?` to `replace`.
- `crates/fleetops-api/src/authz.rs`:
  ```rust
      /// Read Kubernetes objects, events and pod logs.
      KubeRead,
      /// Delete, scale, restart, and exec into pods.
      KubeWrite,
  ```
  The `match` is unchanged: users get everything but `EnvManage`.
- `crates/fleetops-api/tests/common/fake_docker.rs`:
  - add `built: Mutex<Vec<EnvSpec>>` to `FakeDocker`'s state (with `Default`), and `pub fn built(&self) -> Vec<EnvSpec>`;
  - `FakeFactory::build(&self, spec: &EnvSpec)` pushes `spec.clone()` and keeps returning a `FakeEnv` with `spec.id` and `spec.name`;
  - `EnvStatus::Up { version: EnvVersion::Docker(...) }`.
- `crates/fleetops-core/tests/docker_types.rs`: wrap the version in `EnvVersion::Docker(...)`, and update its expected JSON to carry `"kind": "docker"`.
- Every other `EnvFactory` implementation the compiler finds gets the same `&EnvSpec` signature.

- [ ] **Step 5: Regenerate the API document and the web client, and update the fixtures**

Run: `make openapi && git diff --stat crates/fleetops-api/openapi.json web/src/api/gen`
Expected: both changed (EnvKind, EnvStatus, EnvVersion, KubeVersion, LogOptions and ExecOptions).

In `web/src/test/app.tsx`, `web/src/api/queries.test.ts` and `web/src/pages/DashboardPage.test.tsx`, change every status fixture's `version: { ... }` to `version: { kind: "docker", ... }`.

- [ ] **Step 6: Run everything**

Run: `make lint && make test 2>&1 | tail -3 && make test-web 2>&1 | tail -5`
Expected: all green, including `env_kinds` (2 tests), `envs_credentials` (2 tests) and the new authz test.

- [ ] **Step 7: Commit**

```bash
git add -A crates web/src
git commit -m "feat(core): kube environment kinds, tagged status versions, credential-carrying env specs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 5: Kubeconfig parsing, validation and minifying

**Files:**
- Create: `crates/fleetops-kube/src/kubeconfig.rs`
- Modify: `crates/fleetops-kube/src/lib.rs` (`pub mod kubeconfig;`), `crates/fleetops-kube/Cargo.toml`
- Test: unit tests in `kubeconfig.rs`

**Interfaces:**
- Consumes: `KubeconfigSummary` and `UserKind` (Task 2).
- Produces:
  - `fleetops_kube::kubeconfig::UNSUPPORTED_USER: &str`.
  - `fleetops_kube::kubeconfig::Contexts { contexts: Vec<String>, current: Option<String> }` and `contexts(text: &str) -> Result<Contexts>`.
  - `fleetops_kube::kubeconfig::Prepared { yaml: String, summary: KubeconfigSummary }` and `prepare(text: &str, context: Option<&str>) -> Result<Prepared>`.
  - `fleetops_kube::kubeconfig::config(yaml: &str) -> Result<kube::Config>`, which is synchronous and does no I/O.
  - `fleetops_kube::kubeconfig::check(yaml: &str) -> Result<()>`, which calls `/version` with a 5 s limit and returns `Error::Invalid("cannot reach cluster: …")`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/fleetops-kube/Cargo.toml` `[dependencies]`:

```toml
base64.workspace = true
rustls.workspace = true
serde-saphyr.workspace = true
serde_json.workspace = true
tokio.workspace = true
```

Create `crates/fleetops-kube/src/kubeconfig.rs` holding only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::env::UserKind;

    fn kc(users: &str, current: Option<&str>) -> String {
        format!(
            "apiVersion: v1\nkind: Config\n{}clusters:\n- name: a\n  cluster: {{server: 'https://a.example:6443', certificate-authority-data: ''}}\n- name: b\n  cluster: {{server: 'https://b.example:6443'}}\ncontexts:\n- name: ctx-a\n  context: {{cluster: a, user: ua, namespace: apps}}\n- name: ctx-b\n  context: {{cluster: b, user: ub}}\nusers:\n{users}",
            current.map(|c| format!("current-context: {c}\n")).unwrap_or_default()
        )
    }

    const TOKENS: &str = "- name: ua\n  user: {token: token-a}\n- name: ub\n  user: {token: token-b}\n";

    #[test]
    fn contexts_lists_names_and_the_current_one() {
        let c = contexts(&kc(TOKENS, Some("ctx-b"))).unwrap();
        assert_eq!(c.contexts, ["ctx-a", "ctx-b"]);
        assert_eq!(c.current.as_deref(), Some("ctx-b"));
        assert!(contexts("not: [yaml").is_err());
    }

    #[test]
    fn prepare_keeps_only_the_chosen_context() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a")).unwrap();
        assert_eq!(p.summary.server, "https://a.example:6443");
        assert_eq!(p.summary.context, "ctx-a");
        assert_eq!(p.summary.user_kind, UserKind::Token);
        assert!(p.yaml.contains("token-a"));
        assert!(!p.yaml.contains("token-b") && !p.yaml.contains("b.example"), "{}", p.yaml);
        // The minified file is itself a valid one-context kubeconfig.
        let again = contexts(&p.yaml).unwrap();
        assert_eq!(again.contexts, ["ctx-a"]);
        assert_eq!(again.current.as_deref(), Some("ctx-a"));
    }

    #[test]
    fn without_a_choice_the_current_context_wins_and_ambiguity_is_refused() {
        assert_eq!(prepare(&kc(TOKENS, Some("ctx-b")), None).unwrap().summary.context, "ctx-b");
        let e = prepare(&kc(TOKENS, None), None).unwrap_err().to_string();
        assert!(e.contains("choose a context: ctx-a, ctx-b"), "{e}");
        let e = prepare(&kc(TOKENS, None), Some("nope")).unwrap_err().to_string();
        assert!(e.contains("unknown context 'nope'"), "{e}");
    }

    #[test]
    fn exec_and_auth_provider_users_are_refused() {
        let exec = "- name: ua\n  user:\n    exec: {apiVersion: client.authentication.k8s.io/v1, command: aws}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(exec, None), Some("ctx-a")).unwrap_err().to_string();
        assert!(e.contains(UNSUPPORTED_USER), "{e}");
        let ap = "- name: ua\n  user:\n    auth-provider: {name: gcp}\n- name: ub\n  user: {token: x}\n";
        assert!(prepare(&kc(ap, None), Some("ctx-a")).unwrap_err().to_string().contains(UNSUPPORTED_USER));
    }

    #[test]
    fn file_references_are_refused() {
        let files = "- name: ua\n  user: {client-certificate: /home/me/c.pem, client-key-data: eA==}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(files, None), Some("ctx-a")).unwrap_err().to_string();
        assert!(e.contains("kubeconfig must embed its files: 'client-certificate' points to a file"), "{e}");
        let tf = "- name: ua\n  user: {tokenFile: /t}\n- name: ub\n  user: {token: x}\n";
        assert!(prepare(&kc(tf, None), Some("ctx-a")).unwrap_err().to_string().contains("'tokenFile'"));
    }

    #[test]
    fn a_client_certificate_user_is_cert_kind() {
        let certs = "- name: ua\n  user: {client-certificate-data: Y2VydA==, client-key-data: a2V5}\n- name: ub\n  user: {token: x}\n";
        assert_eq!(prepare(&kc(certs, None), Some("ctx-a")).unwrap().summary.user_kind, UserKind::Cert);
    }

    #[test]
    fn a_user_with_no_token_or_certificate_is_refused() {
        let none = "- name: ua\n  user: {username: admin, password: pw}\n- name: ub\n  user: {token: x}\n";
        let e = prepare(&kc(none, None), Some("ctx-a")).unwrap_err().to_string();
        assert!(e.contains("has no token or client certificate"), "{e}");
    }

    #[test]
    fn config_reads_the_server_namespace_and_token_without_io() {
        let p = prepare(&kc(TOKENS, None), Some("ctx-a")).unwrap();
        let c = config(&p.yaml).unwrap();
        assert_eq!(c.cluster_url.to_string(), "https://a.example:6443/");
        assert_eq!(c.default_namespace, "apps");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-kube --lib kubeconfig 2>&1 | tail -5`
Expected: compile errors, because `contexts`, `prepare` and `config` are not defined.

- [ ] **Step 3: Implement**

Above the tests in `kubeconfig.rs`:

```rust
//! Kubeconfig handling for `kube-config` environments (spec §4.1): pick a
//! context, refuse users that need a helper binary or local files (the
//! server is distroless), and reduce the file to that one context.

use base64::{engine::general_purpose::STANDARD, Engine};
use fleetops_core::env::{KubeconfigSummary, UserKind};
use fleetops_core::{Error, Result};
use kube::config::{AuthInfo, Kubeconfig, NamedAuthInfo, NamedCluster, NamedContext};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use std::time::Duration;

pub const UNSUPPORTED_USER: &str =
    "use a ServiceAccount token or the FleetOps agent: exec and auth-provider users are not supported";
/// How long the create-time reachability check waits.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn parse(text: &str) -> Result<Kubeconfig> {
    Kubeconfig::from_yaml(text).map_err(|e| invalid(format!("not a kubeconfig: {e}")))
}

pub struct Contexts {
    pub contexts: Vec<String>,
    pub current: Option<String>,
}

pub fn contexts(text: &str) -> Result<Contexts> {
    let kc = parse(text)?;
    let contexts: Vec<String> = kc.contexts.iter().map(|c| c.name.clone()).collect();
    if contexts.is_empty() {
        return Err(invalid("the kubeconfig has no contexts"));
    }
    let current = kc.current_context.filter(|c| contexts.contains(c));
    Ok(Contexts { contexts, current })
}

pub struct Prepared {
    /// A self-contained kubeconfig with exactly one context.
    pub yaml: String,
    pub summary: KubeconfigSummary,
}

fn pick(kc: &Kubeconfig, context: Option<&str>) -> Result<String> {
    let names: Vec<&str> = kc.contexts.iter().map(|c| c.name.as_str()).collect();
    match context {
        Some(c) if names.contains(&c) => Ok(c.to_string()),
        Some(c) => Err(invalid(format!(
            "unknown context '{c}'; the kubeconfig has: {}",
            names.join(", ")
        ))),
        None => match (kc.current_context.as_deref(), names.as_slice()) {
            (Some(cur), _) if names.contains(&cur) => Ok(cur.to_string()),
            (_, [only]) => Ok(only.to_string()),
            (_, []) => Err(invalid("the kubeconfig has no contexts")),
            _ => Err(invalid(format!("choose a context: {}", names.join(", ")))),
        },
    }
}

fn file_ref(field: &str) -> Error {
    invalid(format!("kubeconfig must embed its files: '{field}' points to a file"))
}

fn check_user(context: &str, user: &AuthInfo) -> Result<UserKind> {
    if user.exec.is_some() || user.auth_provider.is_some() {
        return Err(invalid(UNSUPPORTED_USER));
    }
    if user.token_file.is_some() {
        return Err(file_ref("tokenFile"));
    }
    if user.client_certificate.is_some() {
        return Err(file_ref("client-certificate"));
    }
    if user.client_key.is_some() {
        return Err(file_ref("client-key"));
    }
    if user.token.is_some() {
        Ok(UserKind::Token)
    } else if user.client_certificate_data.is_some() && user.client_key_data.is_some() {
        Ok(UserKind::Cert)
    } else {
        Err(invalid(format!(
            "the user in context '{context}' has no token or client certificate"
        )))
    }
}

pub fn prepare(text: &str, context: Option<&str>) -> Result<Prepared> {
    let kc = parse(text)?;
    let name = pick(&kc, context)?;
    let ctx = kc.contexts.iter().find(|c| c.name == name).expect("picked");
    let inner = ctx
        .context
        .clone()
        .ok_or_else(|| invalid(format!("context '{name}' is empty")))?;
    let cluster = kc
        .clusters
        .iter()
        .find(|c| c.name == inner.cluster)
        .ok_or_else(|| invalid(format!("context '{name}' names a missing cluster '{}'", inner.cluster)))?;
    let cl = cluster
        .cluster
        .clone()
        .ok_or_else(|| invalid(format!("cluster '{}' is empty", cluster.name)))?;
    if cl.certificate_authority.is_some() {
        return Err(file_ref("certificate-authority"));
    }
    let server = cl
        .server
        .clone()
        .ok_or_else(|| invalid(format!("the cluster in context '{name}' has no server")))?;
    let user_name = inner.user.clone().unwrap_or_default();
    let user = kc
        .auth_infos
        .iter()
        .find(|u| u.name == user_name)
        .ok_or_else(|| invalid(format!("context '{name}' names a missing user '{user_name}'")))?;
    let auth = user.auth_info.clone().unwrap_or_default();
    let user_kind = check_user(&name, &auth)?;

    let minified = Kubeconfig {
        api_version: Some("v1".into()),
        kind: Some("Config".into()),
        current_context: Some(name.clone()),
        clusters: vec![NamedCluster {
            name: cluster.name.clone(),
            cluster: Some(cl),
            ..cluster.clone()
        }],
        auth_infos: vec![NamedAuthInfo {
            name: user.name.clone(),
            auth_info: Some(auth),
        }],
        contexts: vec![NamedContext {
            name: name.clone(),
            context: Some(inner),
            ..ctx.clone()
        }],
        ..Kubeconfig::default()
    };
    let yaml = serde_saphyr::to_string(&minified)
        .map_err(|e| Error::Internal(format!("cannot write kubeconfig: {e}")))?;
    Ok(Prepared {
        yaml,
        summary: KubeconfigSummary {
            server,
            context: name,
            user_kind,
        },
    })
}

/// A kube-rs config for a prepared kubeconfig, built without I/O so the
/// environment factory (synchronous) can use it.
pub fn config(yaml: &str) -> Result<kube::Config> {
    let kc = parse(yaml)?;
    let ctx = kc
        .contexts
        .first()
        .and_then(|c| c.context.clone())
        .ok_or_else(|| invalid("stored kubeconfig has no context"))?;
    let cl = kc
        .clusters
        .first()
        .and_then(|c| c.cluster.clone())
        .ok_or_else(|| invalid("stored kubeconfig has no cluster"))?;
    let server = cl.server.clone().unwrap_or_default();
    let mut c = kube::Config::new(
        server
            .parse()
            .map_err(|e| invalid(format!("bad server URL '{server}': {e}")))?,
    );
    c.default_namespace = ctx.namespace.unwrap_or_else(|| "default".into());
    c.accept_invalid_certs = cl.insecure_skip_tls_verify.unwrap_or(false);
    c.tls_server_name = cl.tls_server_name.clone();
    c.connect_timeout = Some(CHECK_TIMEOUT);
    if let Some(data) = cl.certificate_authority_data.as_deref().filter(|d| !d.is_empty()) {
        let pem = STANDARD
            .decode(data)
            .map_err(|_| invalid("certificate-authority-data is not base64"))?;
        let certs: Vec<Vec<u8>> = CertificateDer::pem_slice_iter(&pem)
            .filter_map(|c| c.ok())
            .map(|c| c.to_vec())
            .collect();
        c.root_cert = Some(certs);
    }
    c.auth_info = kc
        .auth_infos
        .first()
        .and_then(|u| u.auth_info.clone())
        .unwrap_or_default();
    Ok(c)
}

/// `GET /version` with a short limit: used before saving a kubeconfig.
pub async fn check(yaml: &str) -> Result<()> {
    let client = kube::Client::try_from(config(yaml)?)
        .map_err(|e| invalid(format!("cannot reach cluster: {e}")))?;
    match tokio::time::timeout(CHECK_TIMEOUT, client.apiserver_version()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(invalid(format!("cannot reach cluster: {e}"))),
        Err(_) => Err(invalid(format!(
            "cannot reach cluster: no answer within {}s",
            CHECK_TIMEOUT.as_secs()
        ))),
    }
}
```

If `NamedCluster`/`NamedContext` have no other fields besides those set (no `other` map), drop the `..cluster.clone()`/`..ctx.clone()` spreads. The compiler says which.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-kube --lib kubeconfig 2>&1 | tail -5`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-kube
git commit -m "feat(kube): validate, pick and minify kubeconfigs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 6: Kind summaries, YAML masking and error mapping

These are the only kind-specific code paths. All of it is pure and unit-tested against JSON fixtures, so no cluster is needed.

**Files:**
- Create: `crates/fleetops-kube/src/kinds.rs`, `crates/fleetops-kube/src/summaries.rs`, `crates/fleetops-kube/src/mask.rs`, `crates/fleetops-kube/src/errors.rs`
- Modify: `crates/fleetops-kube/src/lib.rs`
- Test: `crates/fleetops-kube/tests/summaries.rs`, plus unit tests in `mask.rs` and `errors.rs`

**Interfaces:**
- Consumes: `KubeKind`, `KubeRow`, `Health`, `KubeCondition`, `KubeRef`, `MASK`, `YAML_CAP` (Task 2).
- Produces:
  - `fleetops_kube::kinds::api_resource(kind: KubeKind) -> kube::discovery::ApiResource`.
  - `fleetops_kube::summaries::row(kind: KubeKind, obj: &DynamicObject) -> KubeRow`.
  - `fleetops_kube::summaries::conditions(obj: &DynamicObject) -> Vec<KubeCondition>`.
  - `fleetops_kube::summaries::owners(obj: &DynamicObject) -> Vec<KubeRef>`.
  - `fleetops_kube::summaries::containers(obj: &DynamicObject) -> Vec<String>`.
  - `fleetops_kube::mask::yaml(kind: KubeKind, obj: &DynamicObject) -> String`.
  - `fleetops_kube::errors::map(e: kube::Error) -> fleetops_core::Error`.

- [ ] **Step 1: Write the failing tests**

`crates/fleetops-kube/tests/summaries.rs`:

```rust
use fleetops_core::kube::{Health, KubeKind};
use fleetops_kube::summaries::{containers, owners, row};
use kube::api::DynamicObject;
use serde_json::json;

fn obj(v: serde_json::Value) -> DynamicObject {
    serde_json::from_value(v).unwrap()
}

fn meta(name: &str) -> serde_json::Value {
    json!({"name": name, "namespace": "apps", "creationTimestamp": "2026-10-01T00:00:00Z"})
}

#[test]
fn a_partly_available_deployment_is_a_warning() {
    let d = obj(json!({"metadata": meta("web"), "spec": {"replicas": 3, "selector": {}, "template": {}},
        "status": {"readyReplicas": 1, "updatedReplicas": 3, "availableReplicas": 1}}));
    let r = row(KubeKind::Deployments, &d);
    assert_eq!((r.name.as_str(), r.namespace.as_deref()), ("web", Some("apps")));
    assert_eq!(r.created, 1_790_812_800);
    assert_eq!(r.status, "1/3");
    assert_eq!(r.health, Health::Warn);
    assert_eq!(r.columns["ready"], "1/3");
    assert_eq!(r.columns["up_to_date"], "3");
    assert_eq!(r.columns["available"], "1");
}

#[test]
fn a_crash_looping_pod_shows_its_reason_and_restarts() {
    let p = obj(json!({"metadata": meta("web-1"), "spec": {"nodeName": "n1", "containers": [{"name": "a"}, {"name": "b"}]},
        "status": {"phase": "Running", "podIP": "10.0.0.5", "containerStatuses": [
            {"name": "a", "ready": true, "restartCount": 0, "image": "x", "imageID": "", "state": {"running": {}}},
            {"name": "b", "ready": false, "restartCount": 5, "image": "x", "imageID": "", "state": {"waiting": {"reason": "CrashLoopBackOff"}}}]}}));
    let r = row(KubeKind::Pods, &p);
    assert_eq!(r.status, "CrashLoopBackOff");
    assert_eq!(r.health, Health::Bad);
    assert_eq!(r.columns["ready"], "1/2");
    assert_eq!(r.columns["restarts"], "5");
    assert_eq!(r.columns["node"], "n1");
    assert_eq!(containers(&p), ["a", "b"]);
}

#[test]
fn a_ready_running_pod_is_ok() {
    let p = obj(json!({"metadata": meta("ok"), "spec": {"containers": [{"name": "a"}]},
        "status": {"phase": "Running", "containerStatuses": [
            {"name": "a", "ready": true, "restartCount": 0, "image": "x", "imageID": "", "state": {"running": {}}}]}}));
    let r = row(KubeKind::Pods, &p);
    assert_eq!((r.status.as_str(), r.health), ("Running", Health::Ok));
}

#[test]
fn a_service_lists_its_type_and_ports() {
    let s = obj(json!({"metadata": meta("web"), "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.9",
        "ports": [{"port": 80, "protocol": "TCP"}, {"port": 53, "protocol": "UDP"}]}}));
    let r = row(KubeKind::Services, &s);
    assert_eq!(r.status, "ClusterIP");
    assert_eq!(r.columns["cluster_ip"], "10.96.0.9");
    assert_eq!(r.columns["ports"], "80/TCP, 53/UDP");
}

#[test]
fn nodes_jobs_and_secrets_summarise() {
    let n = obj(json!({"metadata": {"name": "n1", "labels": {"node-role.kubernetes.io/control-plane": ""}},
        "status": {"conditions": [{"type": "Ready", "status": "True"}], "nodeInfo": {"kubeletVersion": "v1.32.0",
        "architecture": "", "bootID": "", "containerRuntimeVersion": "", "kernelVersion": "", "kubeProxyVersion": "",
        "machineID": "", "operatingSystem": "", "osImage": "", "systemUUID": ""}}}));
    let r = row(KubeKind::Nodes, &n);
    assert_eq!((r.status.as_str(), r.health), ("Ready", Health::Ok));
    assert_eq!(r.namespace, None);
    assert_eq!(r.columns["roles"], "control-plane");
    assert_eq!(r.columns["version"], "v1.32.0");

    let j = obj(json!({"metadata": meta("once"), "spec": {"template": {}}, "status": {"succeeded": 1,
        "conditions": [{"type": "Complete", "status": "True"}]}}));
    let r = row(KubeKind::Jobs, &j);
    assert_eq!((r.status.as_str(), r.health), ("Complete", Health::Ok));
    assert_eq!(r.columns["completions"], "1/1");

    let s = obj(json!({"metadata": meta("creds"), "type": "Opaque", "data": {"a": "eA==", "b": "eQ=="}}));
    let r = row(KubeKind::Secrets, &s);
    assert_eq!(r.status, "Opaque");
    assert_eq!(r.columns["keys"], "2");
}

#[test]
fn an_unparseable_object_still_gets_a_row() {
    let d = obj(json!({"metadata": meta("odd"), "spec": {"replicas": "three"}}));
    let r = row(KubeKind::Deployments, &d);
    assert_eq!((r.name.as_str(), r.health), ("odd", Health::Unknown));
}

#[test]
fn owners_link_kinds_fleetops_has_pages_for() {
    let p = obj(json!({"metadata": {"name": "web-1", "namespace": "apps", "ownerReferences": [
        {"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "web-7d9", "uid": "u"}]}}));
    let o = owners(&p);
    assert_eq!(o.len(), 1);
    assert_eq!((o[0].kind.as_str(), o[0].name.as_str(), o[0].link), ("ReplicaSet", "web-7d9", None));
}
```

Unit tests at the bottom of `mask.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> DynamicObject {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn managed_fields_are_stripped() {
        let o = obj(json!({"metadata": {"name": "c", "managedFields": [{"manager": "kubectl"}]}, "data": {"mode": "prod"}}));
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(!y.contains("managedFields") && y.contains("mode: prod") && y.contains("kind: ConfigMap"), "{y}");
    }

    #[test]
    fn secret_values_are_masked_but_keys_stay() {
        let o = obj(json!({"metadata": {"name": "s"}, "type": "Opaque",
            "data": {"password": "aHVudGVyMg=="}, "stringData": {"token": "plain"}}));
        let y = yaml(KubeKind::Secrets, &o);
        assert!(y.contains("password") && y.contains("token") && y.contains(MASK), "{y}");
        assert!(!y.contains("aHVudGVyMg==") && !y.contains("plain"), "{y}");
    }

    #[test]
    fn a_secrets_last_applied_annotation_is_masked() {
        let applied = r#"{"apiVersion":"v1","kind":"Secret","stringData":{"password":"hunter2"}}"#;
        let o = obj(json!({"metadata": {"name": "s", "annotations": {
            "kubectl.kubernetes.io/last-applied-configuration": applied, "team": "a"}}, "data": {}}));
        let y = yaml(KubeKind::Secrets, &o);
        assert!(!y.contains("hunter2"), "{y}");
        assert!(y.contains("team"), "{y}");
    }

    #[test]
    fn a_huge_document_is_cut_with_a_note() {
        let big = "x".repeat(YAML_CAP + 10);
        let o = obj(json!({"metadata": {"name": "c"}, "data": {"big": big}}));
        let y = yaml(KubeKind::ConfigMaps, &o);
        assert!(y.len() <= YAML_CAP + 64);
        assert!(y.ends_with("# … cut at 1 MiB\n"), "{}", &y[y.len() - 40..]);
    }
}
```

Unit tests at the bottom of `errors.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn api(code: u16, msg: &str) -> kube::Error {
        let s: kube::core::Status = serde_json::from_value(json!({
            "status": "Failure", "code": code, "message": msg, "reason": "x"})).unwrap();
        kube::Error::Api(Box::new(s))
    }

    #[test]
    fn apiserver_codes_map_to_fleetops_errors() {
        assert!(matches!(map(api(404, "nope")), Error::NotFound(m) if m == "nope"));
        assert!(matches!(map(api(403, "pods is forbidden: cannot list")), Error::Forbidden(m) if m.contains("cannot list")));
        assert!(matches!(map(api(409, "c")), Error::Conflict(_)));
        assert!(matches!(map(api(422, "i")), Error::Invalid(_)));
        assert!(matches!(map(api(500, "boom")), Error::Upstream { status: 502, .. }));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-kube 2>&1 | tail -5`
Expected: compile errors, because the `summaries`, `mask` and `errors` modules don't exist.

- [ ] **Step 3: Implement**

`crates/fleetops-kube/src/lib.rs`:

```rust
//! Kubernetes environments on kube-rs: a client from a kubeconfig or
//! through an agent tunnel, and the `KubeOps` implementation over it.

pub mod errors;
pub mod kinds;
pub mod kubeconfig;
pub mod mask;
pub mod summaries;
pub mod tunnel;
```

`crates/fleetops-kube/src/kinds.rs`:

```rust
//! Each kind's API group, version and resource, for dynamic requests.

use fleetops_core::kube::KubeKind;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{ConfigMap, Namespace, Node, Pod, Secret, Service},
    networking::v1::Ingress,
};
use kube::discovery::ApiResource;

pub fn api_resource(kind: KubeKind) -> ApiResource {
    match kind {
        KubeKind::Namespaces => ApiResource::erase::<Namespace>(&()),
        KubeKind::Nodes => ApiResource::erase::<Node>(&()),
        KubeKind::Deployments => ApiResource::erase::<Deployment>(&()),
        KubeKind::StatefulSets => ApiResource::erase::<StatefulSet>(&()),
        KubeKind::DaemonSets => ApiResource::erase::<DaemonSet>(&()),
        KubeKind::Jobs => ApiResource::erase::<Job>(&()),
        KubeKind::CronJobs => ApiResource::erase::<CronJob>(&()),
        KubeKind::Pods => ApiResource::erase::<Pod>(&()),
        KubeKind::Services => ApiResource::erase::<Service>(&()),
        KubeKind::Ingresses => ApiResource::erase::<Ingress>(&()),
        KubeKind::ConfigMaps => ApiResource::erase::<ConfigMap>(&()),
        KubeKind::Secrets => ApiResource::erase::<Secret>(&()),
    }
}

/// The object as JSON with `apiVersion` and `kind` set: list items arrive
/// without them, and the typed k8s-openapi structs require them.
pub fn typed_json(kind: KubeKind, obj: &kube::api::DynamicObject) -> serde_json::Value {
    let ar = api_resource(kind);
    let mut v = serde_json::to_value(obj).unwrap_or_default();
    v["apiVersion"] = ar.api_version.into();
    v["kind"] = ar.kind.into();
    v
}
```

`crates/fleetops-kube/src/summaries.rs`:

```rust
//! One row per object (spec §2.4): the only kind-specific code. Objects
//! that don't parse as their typed struct still get a row, with health
//! Unknown, so one odd object never fails a whole list.

use crate::kinds::typed_json;
use fleetops_core::kube::{Health, KubeCondition, KubeKind, KubeRef, KubeRow};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{ConfigMap, Namespace, Node, Pod, Secret, Service},
    networking::v1::Ingress,
};
use kube::api::DynamicObject;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

type Cols = BTreeMap<String, String>;

fn typed<K: DeserializeOwned>(kind: KubeKind, obj: &DynamicObject) -> Option<K> {
    serde_json::from_value(typed_json(kind, obj)).ok()
}

fn cols<const N: usize>(pairs: [(&str, String); N]) -> Cols {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `ready` of `want`, with the health that implies.
fn replicas(ready: i32, want: i32) -> (String, Health) {
    let h = if ready >= want {
        Health::Ok
    } else if ready > 0 {
        Health::Warn
    } else {
        Health::Bad
    };
    (format!("{ready}/{want}"), h)
}

pub fn row(kind: KubeKind, obj: &DynamicObject) -> KubeRow {
    let m = &obj.metadata;
    let base = KubeRow {
        name: m.name.clone().unwrap_or_default(),
        namespace: if kind.cluster_scoped() { None } else { m.namespace.clone() },
        created: m.creation_timestamp.as_ref().map(|t| t.0.as_second()).unwrap_or(0),
        status: String::new(),
        health: Health::Unknown,
        columns: Cols::new(),
    };
    let summary = match kind {
        KubeKind::Namespaces => typed::<Namespace>(kind, obj).map(namespace),
        KubeKind::Nodes => typed::<Node>(kind, obj).map(node),
        KubeKind::Deployments => typed::<Deployment>(kind, obj).map(deployment),
        KubeKind::StatefulSets => typed::<StatefulSet>(kind, obj).map(statefulset),
        KubeKind::DaemonSets => typed::<DaemonSet>(kind, obj).map(daemonset),
        KubeKind::Jobs => typed::<Job>(kind, obj).map(job),
        KubeKind::CronJobs => typed::<CronJob>(kind, obj).map(cronjob),
        KubeKind::Pods => typed::<Pod>(kind, obj).map(pod),
        KubeKind::Services => typed::<Service>(kind, obj).map(service),
        KubeKind::Ingresses => typed::<Ingress>(kind, obj).map(ingress),
        KubeKind::ConfigMaps => typed::<ConfigMap>(kind, obj).map(configmap),
        KubeKind::Secrets => typed::<Secret>(kind, obj).map(secret),
    };
    match summary {
        Some((status, health, columns)) => KubeRow { status, health, columns, ..base },
        None => KubeRow { status: "Unknown".into(), ..base },
    }
}

type Summary = (String, Health, Cols);

fn namespace(n: Namespace) -> Summary {
    let phase = n.status.and_then(|s| s.phase).unwrap_or_else(|| "Active".into());
    let h = if phase == "Active" { Health::Ok } else { Health::Warn };
    (phase, h, Cols::new())
}

fn node(n: Node) -> Summary {
    let roles: Vec<String> = n
        .metadata
        .labels
        .unwrap_or_default()
        .keys()
        .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/").map(str::to_string))
        .collect();
    let status = n.status.unwrap_or_default();
    let ready = status
        .conditions
        .unwrap_or_default()
        .into_iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");
    let ip = status
        .addresses
        .unwrap_or_default()
        .into_iter()
        .find(|a| a.type_ == "InternalIP")
        .map(|a| a.address)
        .unwrap_or_default();
    let version = status.node_info.map(|i| i.kubelet_version).unwrap_or_default();
    let (s, h) = if ready { ("Ready", Health::Ok) } else { ("NotReady", Health::Bad) };
    (
        s.into(),
        h,
        cols([("roles", roles.join(",")), ("version", version), ("internal_ip", ip)]),
    )
}

fn deployment(d: Deployment) -> Summary {
    let want = d.spec.and_then(|s| s.replicas).unwrap_or(1);
    let st = d.status.unwrap_or_default();
    let available = st.available_replicas.unwrap_or(0);
    let (ready, h) = replicas(st.ready_replicas.unwrap_or(0), want);
    let h = if want == 0 { Health::Ok } else { replicas(available, want).1.min_with(h) };
    (
        ready.clone(),
        h,
        cols([
            ("ready", ready),
            ("up_to_date", st.updated_replicas.unwrap_or(0).to_string()),
            ("available", available.to_string()),
        ]),
    )
}

fn statefulset(s: StatefulSet) -> Summary {
    let want = s.spec.and_then(|s| s.replicas).unwrap_or(1);
    let (ready, h) = replicas(s.status.and_then(|s| s.ready_replicas).unwrap_or(0), want);
    let h = if want == 0 { Health::Ok } else { h };
    (ready.clone(), h, cols([("ready", ready)]))
}

fn daemonset(d: DaemonSet) -> Summary {
    let st = d.status.unwrap_or_default();
    let (ready, h) = replicas(st.number_ready, st.desired_number_scheduled);
    (
        ready.clone(),
        h,
        cols([
            ("desired", st.desired_number_scheduled.to_string()),
            ("current", st.current_number_scheduled.to_string()),
            ("ready", st.number_ready.to_string()),
            ("available", st.number_available.unwrap_or(0).to_string()),
        ]),
    )
}

fn job(j: Job) -> Summary {
    let want = j.spec.and_then(|s| s.completions).unwrap_or(1);
    let st = j.status.unwrap_or_default();
    let done = |t: &str| {
        st.conditions
            .iter()
            .flatten()
            .any(|c| c.type_ == t && c.status == "True")
    };
    let (s, h) = if done("Complete") {
        ("Complete", Health::Ok)
    } else if done("Failed") {
        ("Failed", Health::Bad)
    } else {
        ("Running", Health::Ok)
    };
    (
        s.into(),
        h,
        cols([("completions", format!("{}/{want}", st.succeeded.unwrap_or(0)))]),
    )
}

fn cronjob(c: CronJob) -> Summary {
    let spec = c.spec.unwrap_or_default();
    let st = c.status.unwrap_or_default();
    let active = st.active.map(|a| a.len()).unwrap_or(0);
    let s = if spec.suspend.unwrap_or(false) {
        "Suspended"
    } else if active > 0 {
        "Active"
    } else {
        "Scheduled"
    };
    (
        s.into(),
        Health::Ok,
        cols([
            ("schedule", spec.schedule),
            ("active", active.to_string()),
            (
                "last_schedule",
                st.last_schedule_time.map(|t| t.0.to_string()).unwrap_or_default(),
            ),
        ]),
    )
}

fn pod(p: Pod) -> Summary {
    let terminating = p.metadata.deletion_timestamp.is_some();
    let node = p.spec.as_ref().and_then(|s| s.node_name.clone()).unwrap_or_default();
    let want = p.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0);
    let st = p.status.unwrap_or_default();
    let cs = st.container_statuses.unwrap_or_default();
    let ready = cs.iter().filter(|c| c.ready).count();
    let restarts: i32 = cs.iter().map(|c| c.restart_count).sum();
    let reason = cs.iter().find_map(|c| {
        let state = c.state.as_ref()?;
        state
            .waiting
            .as_ref()
            .and_then(|w| w.reason.clone())
            .or_else(|| state.terminated.as_ref().and_then(|t| t.reason.clone()))
    });
    let phase = st.phase.unwrap_or_else(|| "Pending".into());
    let (status, health) = if terminating {
        ("Terminating".to_string(), Health::Warn)
    } else if let Some(r) = reason.filter(|r| r != "Completed") {
        (r, Health::Bad)
    } else {
        let h = match phase.as_str() {
            "Running" if ready == want => Health::Ok,
            "Succeeded" => Health::Ok,
            "Failed" => Health::Bad,
            _ => Health::Warn,
        };
        (phase, h)
    };
    (
        status,
        health,
        cols([
            ("ready", format!("{ready}/{want}")),
            ("restarts", restarts.to_string()),
            ("node", node),
            ("ip", st.pod_ip.unwrap_or_default()),
        ]),
    )
}

fn service(s: Service) -> Summary {
    let spec = s.spec.unwrap_or_default();
    let ports: Vec<String> = spec
        .ports
        .unwrap_or_default()
        .into_iter()
        .map(|p| format!("{}/{}", p.port, p.protocol.unwrap_or_else(|| "TCP".into())))
        .collect();
    let external: Vec<String> = s
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|l| l.ingress)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| i.ip.or(i.hostname))
        .collect();
    (
        spec.type_.unwrap_or_else(|| "ClusterIP".into()),
        Health::Ok,
        cols([
            ("cluster_ip", spec.cluster_ip.unwrap_or_default()),
            ("external_ip", external.join(", ")),
            ("ports", ports.join(", ")),
        ]),
    )
}

fn ingress(i: Ingress) -> Summary {
    let spec = i.spec.unwrap_or_default();
    let hosts: Vec<String> = spec
        .rules
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.host)
        .collect();
    let address: Vec<String> = i
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|l| l.ingress)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| i.ip.or(i.hostname))
        .collect();
    let hosts = hosts.join(", ");
    (
        if hosts.is_empty() { "*".into() } else { hosts.clone() },
        Health::Ok,
        cols([
            ("class", spec.ingress_class_name.unwrap_or_default()),
            ("hosts", hosts),
            ("address", address.join(", ")),
        ]),
    )
}

fn configmap(c: ConfigMap) -> Summary {
    let n = c.data.map(|d| d.len()).unwrap_or(0) + c.binary_data.map(|d| d.len()).unwrap_or(0);
    (format!("{n} keys"), Health::Ok, cols([("keys", n.to_string())]))
}

fn secret(s: Secret) -> Summary {
    let n = s.data.map(|d| d.len()).unwrap_or(0);
    (
        s.type_.unwrap_or_else(|| "Opaque".into()),
        Health::Ok,
        cols([("keys", n.to_string())]),
    )
}

trait Worst {
    fn min_with(self, other: Health) -> Health;
}

impl Worst for Health {
    /// The worse of two healths.
    fn min_with(self, other: Health) -> Health {
        let rank = |h: Health| match h {
            Health::Bad => 0,
            Health::Warn => 1,
            Health::Unknown => 2,
            Health::Ok => 3,
        };
        if rank(self) <= rank(other) { self } else { other }
    }
}

pub fn conditions(obj: &DynamicObject) -> Vec<KubeCondition> {
    obj.data["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(KubeCondition {
                type_: c["type"].as_str()?.to_string(),
                status: c["status"].as_str().unwrap_or_default().to_string(),
                reason: c["reason"].as_str().map(str::to_string),
                message: c["message"].as_str().map(str::to_string),
            })
        })
        .collect()
}

pub fn owners(obj: &DynamicObject) -> Vec<KubeRef> {
    obj.metadata
        .owner_references
        .iter()
        .flatten()
        .map(|o| KubeRef {
            kind: o.kind.clone(),
            name: o.name.clone(),
            link: KubeKind::from_kind_name(&o.kind),
        })
        .collect()
}

pub fn containers(obj: &DynamicObject) -> Vec<String> {
    obj.data["spec"]["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .collect()
}
```

`crates/fleetops-kube/src/mask.rs`:

```rust
//! The YAML shown on an object's page: `managedFields` stripped, and every
//! Secret value masked, including the copy kubectl keeps in the
//! last-applied annotation (spec §8, Review Focus 2).

use crate::kinds::typed_json;
use fleetops_core::kube::{KubeKind, MASK, YAML_CAP};
use kube::api::DynamicObject;
use serde_json::Value;

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
const CUT_NOTE: &str = "\n# … cut at 1 MiB\n";

pub fn yaml(kind: KubeKind, obj: &DynamicObject) -> String {
    let mut v = typed_json(kind, obj);
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    if kind == KubeKind::Secrets {
        for field in ["data", "stringData"] {
            if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
                for val in map.values_mut() {
                    *val = MASK.into();
                }
            }
        }
        if let Some(ann) = v["metadata"]
            .get_mut("annotations")
            .and_then(Value::as_object_mut)
        {
            if ann.contains_key(LAST_APPLIED) {
                ann.insert(LAST_APPLIED.into(), MASK.into());
            }
        }
    }
    let mut text = serde_saphyr::to_string(&v).unwrap_or_else(|e| format!("# cannot render: {e}\n"));
    if text.len() > YAML_CAP {
        let mut cut = YAML_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(CUT_NOTE);
    }
    text
}
```

`crates/fleetops-kube/src/errors.rs`:

```rust
//! kube-rs errors as FleetOps errors (spec §7). The apiserver's message
//! is kept: a 403 says which RBAC rule was missing.

use fleetops_core::Error;

pub fn map(e: kube::Error) -> Error {
    match e {
        kube::Error::Api(s) => match s.code {
            404 => Error::NotFound(s.message),
            403 => Error::Forbidden(s.message),
            409 => Error::Conflict(s.message),
            400 | 422 => Error::Invalid(s.message),
            _ => Error::Upstream {
                status: 502,
                message: s.message,
            },
        },
        other => Error::Upstream {
            status: 502,
            message: other.to_string(),
        },
    }
}
```

Add `serde.workspace = true` to `crates/fleetops-kube/Cargo.toml` `[dependencies]`, and `serde_json.workspace = true` to `[dev-dependencies]`.

Add the `use` lines above each `mod tests` that it needs: `use kube::api::DynamicObject; use fleetops_core::kube::KubeKind;` are already in scope through `super::*`.

The timestamp in the first test, `2026-10-01T00:00:00Z`, is `1_790_812_800`. If the assertion disagrees, recompute the value with `date -u -d 2026-10-01 +%s` (Linux) or `date -u -j -f %F 2026-10-01 +%s` (macOS) and fix the test constant, not the code.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-kube 2>&1 | tail -5`
Expected: all pass (the 7 summaries tests, the 4 mask tests, the errors test, and the kubeconfig tests).

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-kube
git commit -m "feat(kube): per-kind rows, masked YAML and error mapping

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 7: `KubeClient`, the kube-rs implementation of `KubeOps`

**Files:**
- Create: `crates/fleetops-kube/src/client.rs`
- Modify: `crates/fleetops-kube/src/lib.rs` (`pub mod client; pub use client::KubeClient;`), `crates/fleetops-kube/Cargo.toml`
- Modify: `crates/fleetops-kube/tests/common/mod.rs` (an `ops()` helper), `crates/fleetops-kube/tests/kube_it.rs` (the suite)
- Modify: `Makefile` (`test-kube` runs the suite twice)

**Interfaces:**
- Consumes:
  - `KubeOps` and the DTOs (Task 2);
  - `api_resource`, `typed_json`, `row`, `conditions`, `owners`, `containers`, `mask::yaml` and `errors::map` (Task 6);
  - `kubeconfig::config` (Task 5);
  - `tunnel::client` (Task 1).
- Produces:
  - `fleetops_kube::KubeClient::new(client: kube::Client) -> KubeClient`;
  - `KubeClient::with_list_cap(self, cap: usize) -> KubeClient` (tests);
  - `impl KubeOps for KubeClient`;
  - `REQUEST_TIMEOUT: Duration = 30s`;
  - `NODE_CACHE: Duration = 30s`.

- [ ] **Step 1: Write the failing cluster tests**

Add to `crates/fleetops-kube/Cargo.toml` `[dependencies]`:

```toml
async-trait.workspace = true
futures-util = { workspace = true, features = ["io", "sink"] }
tokio-util = { workspace = true, features = ["io"] }
```

Add to `crates/fleetops-kube/tests/common/mod.rs`:

```rust
use fleetops_kube::KubeClient;

/// The client the suite runs against: through the tunnel when
/// FLEETOPS_IT_TUNNEL is set, else straight from kind's kubeconfig.
pub fn ops() -> KubeClient {
    if std::env::var_os("FLEETOPS_IT_TUNNEL").is_some() {
        return KubeClient::new(tunnel_client());
    }
    let p = fleetops_kube::kubeconfig::prepare(&read("kubeconfig"), None).unwrap();
    let config = fleetops_kube::kubeconfig::config(&p.yaml).unwrap();
    KubeClient::new(kube::Client::try_from(config).unwrap())
}

/// Polls `f` for up to 30 s.
pub async fn eventually<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..60 {
        if f().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("timed out waiting for {what}");
}
```

Append to `crates/fleetops-kube/tests/kube_it.rs` (the `use` lines go at the top):

```rust
use fleetops_core::kube::{KubeKind, KubeOps, MASK};
use fleetops_core::streams::{ExecOptions, LogOptions};
use fleetops_core::Error;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::PostParams;

#[tokio::test]
async fn every_kind_lists() {
    let ops = common::ops();
    for kind in KubeKind::ALL {
        let ns = if kind.cluster_scoped() { None } else { Some(NS) };
        ops.list(kind, ns).await.unwrap_or_else(|e| panic!("{kind:?}: {e}"));
    }
    let ns = ops.list(KubeKind::Namespaces, None).await.unwrap();
    assert!(ns.rows.iter().any(|r| r.name == NS));
    let nodes = ops.list(KubeKind::Nodes, None).await.unwrap();
    assert!(!nodes.rows.is_empty());
    let deps = ops.list(KubeKind::Deployments, Some(NS)).await.unwrap();
    let web = deps.rows.iter().find(|r| r.name == "web").expect("web");
    assert!(web.columns["ready"].ends_with("/2"), "{web:?}");
    // All namespaces includes ours.
    let all = ops.list(KubeKind::Pods, None).await.unwrap();
    assert!(all.rows.iter().any(|r| r.namespace.as_deref() == Some(NS)));
}

#[tokio::test]
async fn a_capped_list_says_it_is_truncated() {
    let ops = common::ops().with_list_cap(1);
    let pods = ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    assert_eq!(pods.rows.len(), 1);
    assert!(pods.truncated);
}

#[tokio::test]
async fn a_pod_shows_yaml_owners_and_containers() {
    let ops = common::ops();
    let pods = ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    let web = pods.rows.iter().find(|r| r.name.starts_with("web-")).expect("a web pod");
    let o = ops.get(KubeKind::Pods, NS, &web.name).await.unwrap();
    assert!(o.yaml.contains("kind: Pod") && !o.yaml.contains("managedFields"), "{}", o.yaml);
    let kinds: Vec<_> = o.owners.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["ReplicaSet", "Deployment"]);
    assert_eq!(o.owners[1].link, Some(KubeKind::Deployments));
    assert_eq!(o.containers, ["web"]);
    assert!(o.conditions.iter().any(|c| c.type_ == "Ready"));
    let t = ops.get(KubeKind::Pods, NS, "ticker").await.unwrap();
    assert_eq!(t.containers, ["main", "side"]);
}

#[tokio::test]
async fn secret_yaml_never_contains_its_value() {
    let ops = common::ops();
    let s = ops.get(KubeKind::Secrets, NS, "creds").await.unwrap();
    assert!(s.yaml.contains(MASK), "{}", s.yaml);
    // Neither the plain value nor its base64 form, anywhere (the
    // last-applied annotation holds it too).
    assert!(!s.yaml.contains("hunter2-do-not-leak"), "{}", s.yaml);
    assert!(!s.yaml.contains("aHVudGVyMi1kby1ub3QtbGVhaw"), "{}", s.yaml);
}

#[tokio::test]
async fn scale_changes_the_desired_replicas() {
    let ops = common::ops();
    ops.scale(KubeKind::StatefulSets, NS, "db", 2).await.unwrap();
    common::eventually("db wants 2", || async {
        let o = ops.get(KubeKind::StatefulSets, NS, "db").await.unwrap();
        o.row.columns["ready"].ends_with("/2")
    })
    .await;
    ops.scale(KubeKind::StatefulSets, NS, "db", 1).await.unwrap();
    assert!(matches!(ops.scale(KubeKind::Pods, NS, "ticker", 2).await, Err(Error::Invalid(_))));
}

#[tokio::test]
async fn restart_stamps_the_template_and_records_events() {
    let ops = common::ops();
    ops.restart(KubeKind::Deployments, NS, "web").await.unwrap();
    let o = ops.get(KubeKind::Deployments, NS, "web").await.unwrap();
    assert!(o.yaml.contains("kubectl.kubernetes.io/restartedAt"), "{}", o.yaml);
    common::eventually("a ScalingReplicaSet event", || async {
        let ev = ops.events(KubeKind::Deployments, NS, "web").await.unwrap();
        ev.iter().any(|e| e.reason == "ScalingReplicaSet")
    })
    .await;
}

#[tokio::test]
async fn delete_removes_the_object() {
    let ops = common::ops();
    let name = format!("tmp-{}", std::process::id());
    let cms: kube::Api<ConfigMap> = kube::Api::namespaced(common::tunnel_client(), NS);
    let cm: ConfigMap = serde_json::from_value(serde_json::json!({"metadata": {"name": name}})).unwrap();
    cms.create(&PostParams::default(), &cm).await.unwrap();
    ops.delete(KubeKind::ConfigMaps, NS, &name).await.unwrap();
    assert!(matches!(ops.get(KubeKind::ConfigMaps, NS, &name).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn logs_and_exec_default_to_the_first_container() {
    use futures_util::StreamExt;
    let ops = common::ops();
    let opts = LogOptions { follow: false, tail: Some(3), ..LogOptions::default() };
    let lines: Vec<_> = ops.logs(NS, "ticker", &opts).collect().await;
    assert!(!lines.is_empty());
    for l in lines {
        assert!(l.unwrap().text.starts_with("tick"), "side container's logs leaked in");
    }
    let exec = ExecOptions { cmd: Some(vec!["cat".into(), "/proc/1/cmdline".into()]), ..ExecOptions::default() };
    let mut session = ops.exec_start(NS, "ticker", &exec).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = session.output.next().await {
        out.extend(chunk.unwrap());
    }
    assert!(String::from_utf8_lossy(&out).contains("tick"), "{}", String::from_utf8_lossy(&out));
}

#[tokio::test]
async fn exec_resizes_and_reports_the_exit_code() {
    use futures_util::StreamExt;
    let ops = common::ops();
    let opts = ExecOptions { cmd: Some(vec!["sh".into()]), container: Some("main".into()), ..ExecOptions::default() };
    let mut s = ops.exec_start(NS, "ticker", &opts).await.unwrap();
    s.resize.send((123, 45)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.input.send(b"stty size; exit 3\n".to_vec()).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = timeout(WAIT, s.output.next()).await.unwrap() {
        out.extend(chunk.unwrap());
    }
    assert!(String::from_utf8_lossy(&out).contains("45 123"));
    assert_eq!(timeout(WAIT, s.exit_code).await.unwrap().unwrap(), Some(3));
}

#[tokio::test]
async fn a_narrow_token_gets_the_rbac_reason() {
    let ops = fleetops_kube::KubeClient::new(common::tunnel_client_with("narrow-token"));
    ops.list(KubeKind::Pods, Some(NS)).await.unwrap();
    match ops.list(KubeKind::Deployments, Some(NS)).await {
        Err(Error::Forbidden(m)) => assert!(m.contains("deployments"), "{m}"),
        other => panic!("expected forbidden, got {other:?}"),
    }
}
```

In `Makefile`, make `test-kube` run both passes:

```make
test-kube: kind-up
	FLEETOPS_KUBE_IT=$(KUBE_IT) cargo test -p fleetops-kube --features kube-it --test kube_it -- --test-threads=4
	FLEETOPS_KUBE_IT=$(KUBE_IT) FLEETOPS_IT_TUNNEL=1 cargo test -p fleetops-kube --features kube-it --test kube_it -- --test-threads=4
```

- [ ] **Step 2: Run them to verify they fail**

Run: `make test-kube 2>&1 | grep -E "^error|could not compile" | head -3`
Expected: `unresolved import fleetops_kube::KubeClient`.

- [ ] **Step 3: Implement**

`crates/fleetops-kube/src/client.rs`:

```rust
//! `KubeOps` over a kube-rs client, the same code whether the client
//! talks to the apiserver directly or through an agent tunnel.

use crate::{errors, kinds::api_resource, mask, summaries};
use async_trait::async_trait;
use fleetops_core::kube::{
    KubeEvent, KubeExec, KubeKind, KubeList, KubeObject, KubeOps, KubeRef, KubeVersion,
    LIST_CAP, PAGE_SIZE,
};
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{now_unix, Error, Result};
use futures_util::{AsyncBufReadExt, SinkExt, StreamExt, TryStreamExt};
use k8s_openapi::api::{
    apps::v1::ReplicaSet,
    core::v1::{Event, Node, Pod},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::api::{
    Api, AttachParams, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams,
    TerminalSize,
};
use serde_json::json;
use std::{
    future::Future,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

/// Every request but logs and exec gives up after this (spec §5).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Node counts in `version()` are reused for this long.
pub const NODE_CACHE: Duration = Duration::from_secs(30);

pub struct KubeClient {
    client: kube::Client,
    list_cap: usize,
    nodes: Mutex<Option<(Instant, (u32, u32))>>,
}

impl KubeClient {
    pub fn new(client: kube::Client) -> Self {
        Self {
            client,
            list_cap: LIST_CAP,
            nodes: Mutex::new(None),
        }
    }

    /// A lower row cap, so tests can see truncation.
    pub fn with_list_cap(mut self, cap: usize) -> Self {
        self.list_cap = cap;
        self
    }

    fn api(&self, kind: KubeKind, ns: Option<&str>) -> Api<DynamicObject> {
        let ar = api_resource(kind);
        match ns.filter(|_| !kind.cluster_scoped()) {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        }
    }

    fn pods(&self, ns: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), ns)
    }

    async fn node_counts(&self) -> Result<(u32, u32)> {
        if let Some((at, counts)) = *self.nodes.lock().unwrap() {
            if at.elapsed() < NODE_CACHE {
                return Ok(counts);
            }
        }
        let nodes: Api<Node> = Api::all(self.client.clone());
        let list = timed(nodes.list(&ListParams::default())).await?;
        let total = list.items.len() as u32;
        let ready = list
            .items
            .iter()
            .filter(|n| {
                n.status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            })
            .count() as u32;
        *self.nodes.lock().unwrap() = Some((Instant::now(), (total, ready)));
        Ok((total, ready))
    }

    /// The owner chain, nearest first. A ReplicaSet owner is followed one
    /// step so a Deployment's pods link to the Deployment.
    async fn owner_chain(&self, ns: &str, obj: &DynamicObject) -> Vec<KubeRef> {
        let mut chain = summaries::owners(obj);
        if let Some(rs) = chain.iter().find(|o| o.kind == "ReplicaSet").map(|o| o.name.clone()) {
            let rss: Api<ReplicaSet> = Api::namespaced(self.client.clone(), ns);
            if let Ok(Ok(Some(rs))) = tokio::time::timeout(REQUEST_TIMEOUT, rss.get_opt(&rs)).await {
                chain.extend(rs.metadata.owner_references.into_iter().flatten().map(|o| KubeRef {
                    link: KubeKind::from_kind_name(&o.kind),
                    kind: o.kind,
                    name: o.name,
                }));
            }
        }
        chain
    }
}

/// Runs one apiserver call with the request timeout and error mapping.
async fn timed<T>(f: impl Future<Output = kube::Result<T>>) -> Result<T> {
    match tokio::time::timeout(REQUEST_TIMEOUT, f).await {
        Ok(r) => r.map_err(errors::map),
        Err(_) => Err(Error::Upstream {
            status: 504,
            message: format!("the cluster did not answer within {}s", REQUEST_TIMEOUT.as_secs()),
        }),
    }
}

async fn first_container(pods: &Api<Pod>, pod: &str) -> Result<String> {
    let p = timed(pods.get(pod)).await?;
    p.spec
        .and_then(|s| s.containers.into_iter().next())
        .map(|c| c.name)
        .ok_or_else(|| Error::NotFound(format!("pod '{pod}' has no containers")))
}

/// A finished exec's code: 0 on Success, else the ExitCode cause.
fn exit_code(s: &Status) -> Option<i64> {
    if s.status.as_deref() == Some("Success") {
        return Some(0);
    }
    s.details
        .as_ref()?
        .causes
        .as_ref()?
        .iter()
        .find(|c| c.reason.as_deref() == Some("ExitCode"))
        .and_then(|c| c.message.as_deref()?.parse().ok())
}

fn stream_err(e: std::io::Error) -> Error {
    Error::Upstream {
        status: 502,
        message: e.to_string(),
    }
}

#[async_trait]
impl KubeOps for KubeClient {
    async fn version(&self) -> Result<KubeVersion> {
        let info = timed(self.client.apiserver_version()).await?;
        let (nodes, nodes_ready) = self.node_counts().await?;
        Ok(KubeVersion {
            git_version: info.git_version,
            platform: info.platform,
            nodes,
            nodes_ready,
        })
    }

    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList> {
        let api = self.api(kind, ns);
        let page = (self.list_cap as u32).clamp(1, PAGE_SIZE);
        let mut rows = Vec::new();
        let mut cont: Option<String> = None;
        let mut truncated = false;
        loop {
            let mut lp = ListParams::default().limit(page);
            if let Some(c) = &cont {
                lp = lp.continue_token(c);
            }
            let list = timed(api.list(&lp)).await?;
            rows.extend(list.items.iter().map(|o| summaries::row(kind, o)));
            cont = list.metadata.continue_.filter(|c| !c.is_empty());
            if rows.len() >= self.list_cap {
                truncated = rows.len() > self.list_cap || cont.is_some();
                rows.truncate(self.list_cap);
                break;
            }
            if cont.is_none() {
                break;
            }
        }
        rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
        Ok(KubeList { rows, truncated })
    }

    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        let obj = timed(self.api(kind, Some(ns)).get(name)).await?;
        let owners = self.owner_chain(ns, &obj).await;
        Ok(KubeObject {
            row: summaries::row(kind, &obj),
            yaml: mask::yaml(kind, &obj),
            conditions: summaries::conditions(&obj),
            owners,
            containers: if kind == KubeKind::Pods { summaries::containers(&obj) } else { vec![] },
        })
    }

    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        let events: Api<Event> = if kind.cluster_scoped() {
            Api::all(self.client.clone())
        } else {
            Api::namespaced(self.client.clone(), ns)
        };
        let fields = format!("involvedObject.name={name},involvedObject.kind={}", kind.kind_name());
        let list = timed(events.list(&ListParams::default().fields(&fields))).await?;
        let mut out: Vec<KubeEvent> = list
            .items
            .into_iter()
            .map(|e| KubeEvent {
                type_: e.type_.unwrap_or_else(|| "Normal".into()),
                reason: e.reason.unwrap_or_default(),
                message: e.message.unwrap_or_default(),
                count: e.count.unwrap_or(1),
                last_seen: e
                    .last_timestamp
                    .map(|t| t.0.as_second())
                    .or_else(|| e.event_time.map(|t| t.0.as_second())),
                source: e.source.and_then(|s| s.component),
            })
            .collect();
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        Ok(out)
    }

    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        timed(self.api(kind, Some(ns)).delete(name, &DeleteParams::background())).await?;
        Ok(())
    }

    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()> {
        if !kind.scalable() {
            return Err(Error::Invalid(format!("{} cannot be scaled", kind.as_str())));
        }
        let patch = Patch::Merge(json!({ "spec": { "replicas": replicas } }));
        timed(self.api(kind, Some(ns)).patch_subresource("scale", name, &PatchParams::default(), &patch)).await?;
        Ok(())
    }

    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        if !kind.restartable() {
            return Err(Error::Invalid(format!("{} cannot be restarted", kind.as_str())));
        }
        let now = k8s_openapi::jiff::Timestamp::now().to_string();
        let patch = Patch::Merge(json!({ "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": now } } } } }));
        timed(self.api(kind, Some(ns)).patch(name, &PatchParams::default(), &patch)).await?;
        Ok(())
    }

    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        let pods = self.pods(ns);
        let (pod, o) = (pod.to_string(), opts.clone());
        let lines = futures_util::stream::once(async move {
            let container = match o.container.clone() {
                Some(c) => c,
                None => first_container(&pods, &pod).await?,
            };
            let lp = LogParams {
                container: Some(container),
                follow: o.follow,
                tail_lines: o.tail.map(i64::from),
                since_seconds: o.since.map(|s| (now_unix() - s).max(1)),
                timestamps: o.timestamps,
                ..LogParams::default()
            };
            let read = pods.log_stream(&pod, &lp).await.map_err(errors::map)?;
            Ok::<_, Error>(read.lines().map(|l| {
                l.map(|text| LogLine { stream: LogStream::Stdout, text }).map_err(stream_err)
            }))
        })
        .try_flatten();
        Box::pin(lines)
    }

    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec> {
        let pods = self.pods(ns);
        let container = match &opts.container {
            Some(c) => c.clone(),
            None => first_container(&pods, pod).await?,
        };
        let ap = AttachParams::interactive_tty().container(container);
        let mut proc = timed(pods.exec(pod, opts.command(), &ap)).await?;
        let mut size = proc.terminal_size().expect("tty exec has a size channel");
        let _ = size
            .send(TerminalSize { width: opts.cols, height: opts.rows })
            .await;
        let stdout = proc.stdout().expect("tty exec has stdout");
        let mut stdin = proc.stdin().expect("tty exec has stdin");
        let status = proc.take_status().expect("exec has a status");

        let (input, mut input_rx) = mpsc::channel::<Vec<u8>>(64);
        tokio::spawn(async move {
            while let Some(bytes) = input_rx.recv().await {
                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let (resize, mut resize_rx) = mpsc::channel::<(u16, u16)>(8);
        tokio::spawn(async move {
            while let Some((cols, rows)) = resize_rx.recv().await {
                if size.send(TerminalSize { width: cols, height: rows }).await.is_err() {
                    break;
                }
            }
        });
        let (exit_tx, exit_rx) = oneshot::channel();
        tokio::spawn(async move {
            let code = status.await.as_ref().and_then(exit_code);
            let _ = exit_tx.send(code);
            let _ = proc.join().await;
        });
        let output = tokio_util::io::ReaderStream::new(stdout)
            .map(|r| r.map(|b| b.to_vec()).map_err(stream_err));
        Ok(KubeExec {
            output: Box::pin(output),
            input,
            resize,
            exit_code: exit_rx,
        })
    }
}
```

Add `pub mod client;` and `pub use client::KubeClient;` to `lib.rs`.

If the API names differ in kube 4.2 or k8s-openapi 0.28, follow the compiler and keep the behaviour:
- `continue_token`, `fields`, `get_opt`, `patch_subresource`;
- `Event.last_timestamp` and `event_time`;
- `take_status` returning a future of `Option<Status>`.

Record each rename in the ledger.

- [ ] **Step 4: Run them to verify they pass**

Run: `make test-kube 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: `test result: ok` twice (the kubeconfig pass and the tunnel pass), with 11 tests each.

- [ ] **Step 5: Lint, unit suite, commit**

Run: `make lint && cargo test -p fleetops-kube 2>&1 | tail -3`
Expected: clean; unit tests pass.

```bash
git add Makefile crates/fleetops-kube
git commit -m "feat(kube): KubeClient implements KubeOps over kube-rs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 8: Kubernetes environments and the server's factory

**Files:**
- Create: `crates/fleetops-kube/src/env.rs`; Modify: `crates/fleetops-kube/src/lib.rs`, `crates/fleetops-kube/Cargo.toml`
- Create: `crates/fleetops-server/src/factory.rs`; Modify: `crates/fleetops-server/src/lib.rs`, `crates/fleetops-server/src/config.rs`, `crates/fleetops-server/Cargo.toml`
- Test: unit tests in `crates/fleetops-kube/src/env.rs`; `crates/fleetops-kube/tests/kube_it.rs` (a status test); `crates/fleetops-server/tests/secret_key.rs`

**Interfaces:**
- Consumes: `EnvSpec`, `Credential`, `EnvVersion`, `Environment::kube` (Task 4); `KubeClient` (Task 7); `kubeconfig::config` (Task 5); `tunnel::client` (Task 1); `fleetops_tunnel::registry::Registry` (sub-project 2).
- Produces:
  - `fleetops_kube::env::{KubeConfigEnv, KubeAgentEnv, KubeEnvFactory}` and `KubeEnvFactory::new(agents: Arc<Registry>)`;
  - `STATUS_TIMEOUT: Duration = 5s`;
  - `fleetops_server::factory::Factory::new(agents: Arc<Registry>)`;
  - `fleetops_server::config::Config.secret_key: Option<String>` (`--secret-key` / `FLEETOPS_SECRET_KEY`);
  - `fleetops_server::open_store(config: &Config) -> anyhow::Result<Store>`.

- [ ] **Step 1: Write the failing tests**

Unit tests at the bottom of `crates/fleetops-kube/src/env.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus};

    fn spec(kind: EnvKind, credential: Option<Credential>) -> EnvSpec {
        EnvSpec { id: 1, name: "k".into(), kind, url: "https://127.0.0.1:1".into(), credential }
    }

    const UNREACHABLE: &str = "apiVersion: v1\nkind: Config\ncurrent-context: c\nclusters:\n- name: c\n  cluster: {server: 'https://127.0.0.1:1'}\ncontexts:\n- name: c\n  context: {cluster: c, user: u}\nusers:\n- name: u\n  user: {token: t}\n";

    #[tokio::test]
    async fn an_unreachable_cluster_is_down_without_blocking() {
        let f = KubeEnvFactory::new(Default::default());
        let started = std::time::Instant::now();
        let env = f
            .build(&spec(EnvKind::KubeConfig, Some(Credential::Kubeconfig(UNREACHABLE.into()))))
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "build must not connect");
        assert!(env.kube().is_some());
        assert!(env.docker().is_none());
        match tokio::time::timeout(STATUS_TIMEOUT * 2, env.status()).await.unwrap() {
            EnvStatus::Down { reason } => assert!(reason.contains("cannot reach cluster"), "{reason}"),
            up => panic!("expected down, got {up:?}"),
        }
    }

    #[tokio::test]
    async fn an_unreadable_credential_says_to_re_enter_it() {
        let f = KubeEnvFactory::new(Default::default());
        let env = f.build(&spec(EnvKind::KubeConfig, Some(Credential::Unreadable("x".into())))).unwrap();
        assert!(env.kube().is_none());
        assert_eq!(
            env.status().await,
            EnvStatus::Down { reason: "credential cannot be decrypted; re-enter the kubeconfig".into() }
        );
    }

    #[tokio::test]
    async fn an_agent_cluster_waits_for_its_agent() {
        let f = KubeEnvFactory::new(Default::default());
        let env = f.build(&spec(EnvKind::KubeAgent, None)).unwrap();
        assert_eq!(env.kind(), EnvKind::KubeAgent);
        assert!(env.kube().is_none());
        assert!(matches!(env.status().await, EnvStatus::Down { reason } if reason.contains("waiting for the agent")));
    }

    #[test]
    fn docker_kinds_are_not_built_here() {
        let f = KubeEnvFactory::new(Default::default());
        assert!(f.build(&spec(EnvKind::DockerLocal, None)).is_err());
    }
}
```

Append to `crates/fleetops-kube/tests/kube_it.rs`:

```rust
#[tokio::test]
async fn a_kubeconfig_environment_is_up_with_its_nodes() {
    use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion};
    let p = fleetops_kube::kubeconfig::prepare(&common::read("kubeconfig"), None).unwrap();
    let f = fleetops_kube::env::KubeEnvFactory::new(Default::default());
    let env = f
        .build(&EnvSpec {
            id: 9,
            name: "kind".into(),
            kind: EnvKind::KubeConfig,
            url: p.summary.server.clone(),
            credential: Some(Credential::Kubeconfig(p.yaml)),
        })
        .unwrap();
    match env.status().await {
        EnvStatus::Up { version: EnvVersion::Kube(v) } => {
            assert!(v.nodes >= 1 && v.nodes_ready >= 1, "{v:?}");
            assert!(v.git_version.starts_with('v'));
        }
        other => panic!("{other:?}"),
    }
}
```

`crates/fleetops-server/tests/secret_key.rs`:

```rust
use clap::Parser;
use fleetops_server::config::Config;

#[tokio::test]
async fn the_key_file_is_created_once_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from(["fleetops", "--data", dir.path().to_str().unwrap()]);
    let a = fleetops_server::open_store(&config).await.unwrap();
    let rec = a
        .create_environment_with_credential("k", "kube-config", "https://k", "kubeconfig", "tok", &serde_json::json!({}), 1)
        .await
        .unwrap();
    drop(a);
    assert!(dir.path().join("secret.key").exists());
    let b = fleetops_server::open_store(&config).await.unwrap();
    assert_eq!(
        b.env_credential(rec.id).await.unwrap(),
        Some(fleetops_core::env::Credential::Kubeconfig("tok".into()))
    );
}

#[tokio::test]
async fn a_malformed_secret_key_stops_startup() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from(["fleetops", "--data", dir.path().to_str().unwrap(), "--secret-key", "short"]);
    let e = fleetops_server::open_store(&config).await.unwrap_err().to_string();
    assert!(e.contains("FLEETOPS_SECRET_KEY must be 32 bytes"), "{e}");
}
```

Add `fleetops-core`, `serde_json`, `tempfile` and `clap` to `crates/fleetops-server/Cargo.toml` `[dev-dependencies]` if they are missing. Also add `fleetops-kube.workspace = true` and `fleetops-core.workspace = true` to its `[dependencies]` if missing.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-kube --lib env 2>&1 | tail -3; cargo test -p fleetops-server --test secret_key 2>&1 | tail -3`
Expected: compile errors, because `KubeEnvFactory` and `open_store` are not defined.

- [ ] **Step 3: Implement the environments**

Add `fleetops-tunnel` (already present) and `tracing.workspace = true` to `crates/fleetops-kube/Cargo.toml`.

`crates/fleetops-kube/src/env.rs` (above the tests):

```rust
//! The two Kubernetes environment kinds (spec §2.4).

use crate::{kubeconfig, tunnel, KubeClient};
use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::{Credential, EnvFactory, EnvKind, EnvSpec, EnvStatus, EnvVersion, Environment};
use fleetops_core::kube::KubeOps;
use fleetops_core::{Error, Result};
use fleetops_tunnel::registry::Registry;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// How long `status()` waits for the cluster.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const UNREADABLE: &str = "credential cannot be decrypted; re-enter the kubeconfig";

async fn kube_status(ops: &dyn KubeOps, unreachable: &str) -> EnvStatus {
    match tokio::time::timeout(STATUS_TIMEOUT, ops.version()).await {
        Ok(Ok(v)) => EnvStatus::Up { version: EnvVersion::Kube(v) },
        Ok(Err(e)) => EnvStatus::Down { reason: format!("{unreachable}: {e}") },
        Err(_) => EnvStatus::Down {
            reason: format!("{unreachable}: no answer within {}s", STATUS_TIMEOUT.as_secs()),
        },
    }
}

/// A cluster reached directly with its stored kubeconfig. The client is
/// built up front (no I/O), so a dead cluster costs nothing until used.
pub struct KubeConfigEnv {
    id: i64,
    name: String,
    ops: std::result::Result<Arc<KubeClient>, String>,
}

impl KubeConfigEnv {
    pub fn new(id: i64, name: &str, credential: Option<&Credential>) -> Self {
        let ops = match credential {
            None => Err("no kubeconfig stored".to_string()),
            Some(Credential::Unreadable(_)) => Err(UNREADABLE.to_string()),
            Some(Credential::Kubeconfig(yaml)) => kubeconfig::config(yaml)
                .and_then(|c| kube::Client::try_from(c).map_err(|e| Error::Invalid(e.to_string())))
                .map(|c| Arc::new(KubeClient::new(c)))
                .map_err(|e| format!("cannot reach cluster: {e}")),
        };
        Self { id, name: name.to_string(), ops }
    }
}

#[async_trait]
impl Environment for KubeConfigEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        EnvKind::KubeConfig
    }
    async fn status(&self) -> EnvStatus {
        match &self.ops {
            Err(reason) => EnvStatus::Down { reason: reason.clone() },
            Ok(ops) => kube_status(ops.as_ref(), "cannot reach cluster").await,
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        self.ops.as_ref().ok().map(|c| c.clone() as Arc<dyn KubeOps>)
    }
}

/// A cluster behind an in-cluster agent: a client per live session.
pub struct KubeAgentEnv {
    id: i64,
    name: String,
    agents: Arc<Registry>,
    client: Mutex<Option<(u64, Arc<KubeClient>)>>,
}

impl KubeAgentEnv {
    pub fn new(id: i64, name: &str, agents: Arc<Registry>) -> Self {
        Self { id, name: name.to_string(), agents, client: Mutex::new(None) }
    }

    fn client(&self) -> Option<Arc<KubeClient>> {
        let live = self.agents.get(self.id)?;
        let mut slot = self.client.lock().unwrap();
        if let Some((id, c)) = slot.as_ref() {
            if *id == live.id {
                return Some(c.clone());
            }
        }
        let c = Arc::new(KubeClient::new(tunnel::client(Arc::new(live.session))));
        *slot = Some((live.id, c.clone()));
        Some(c)
    }
}

#[async_trait]
impl Environment for KubeAgentEnv {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> EnvKind {
        EnvKind::KubeAgent
    }
    async fn status(&self) -> EnvStatus {
        match self.client() {
            Some(c) => kube_status(c.as_ref(), "agent connected, cluster unreachable").await,
            None => EnvStatus::Down {
                reason: match self.agents.offline_reason(self.id) {
                    Some(r) => format!("agent offline: {r}"),
                    None => "waiting for the agent to connect".into(),
                },
            },
        }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> {
        None
    }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        self.client().map(|c| c as Arc<dyn KubeOps>)
    }
    async fn wait_change(&self) {
        self.agents.wait_change().await
    }
}

/// Builds the Kubernetes kinds; the server combines it with Docker's.
pub struct KubeEnvFactory {
    agents: Arc<Registry>,
}

impl KubeEnvFactory {
    pub fn new(agents: Arc<Registry>) -> Self {
        Self { agents }
    }
}

impl EnvFactory for KubeEnvFactory {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        match spec.kind {
            EnvKind::KubeConfig => Ok(Arc::new(KubeConfigEnv::new(spec.id, &spec.name, spec.credential.as_ref()))),
            EnvKind::KubeAgent => Ok(Arc::new(KubeAgentEnv::new(spec.id, &spec.name, self.agents.clone()))),
            other => Err(Error::Invalid(format!("{} is not a Kubernetes environment kind", other.as_str()))),
        }
    }
}
```

Match `AgentDockerEnv` (`crates/fleetops-docker/src/agent_env.rs`) for:
- the registry's `offline_reason` return type;
- the shape of `Live` (`.id`, `.session`);
- `Registry: Default`.

Add `pub mod env;` to `lib.rs`.

- [ ] **Step 4: Implement the server factory and key loading**

`crates/fleetops-server/src/factory.rs`:

```rust
//! The server's environment factory: Docker kinds from fleetops-docker,
//! Kubernetes kinds from fleetops-kube, one shared agent registry.

use fleetops_core::env::{EnvFactory, EnvSpec, Environment};
use fleetops_core::Result;
use fleetops_docker::DockerEnvFactory;
use fleetops_kube::env::KubeEnvFactory;
use fleetops_tunnel::registry::Registry;
use std::sync::Arc;

pub struct Factory {
    docker: DockerEnvFactory,
    kube: KubeEnvFactory,
}

impl Factory {
    pub fn new(agents: Arc<Registry>) -> Self {
        Self {
            docker: DockerEnvFactory::new(agents.clone()),
            kube: KubeEnvFactory::new(agents),
        }
    }
}

impl EnvFactory for Factory {
    fn build(&self, spec: &EnvSpec) -> Result<Arc<dyn Environment>> {
        if spec.kind.is_kube() {
            self.kube.build(spec)
        } else {
            self.docker.build(spec)
        }
    }
}
```

In `crates/fleetops-server/src/config.rs`, add:

```rust
    /// 32 bytes, base64: seals stored kubeconfigs. Unset: <data>/secret.key,
    /// created on first start. Back it up with the database.
    #[arg(long, env = "FLEETOPS_SECRET_KEY", hide_env_values = true)]
    pub secret_key: Option<String>,
```

In `crates/fleetops-server/src/lib.rs`:
- add `pub mod factory;`;
- make `build_state` use `Arc::new(factory::Factory::new(agents.clone()))`;
- replace the `Store::open` line in `run` with `let store = open_store(&config).await?;`;
- add:

```rust
/// Opens the database with the secret key from FLEETOPS_SECRET_KEY, or
/// from <data>/secret.key (created on first start).
pub async fn open_store(config: &Config) -> anyhow::Result<Store> {
    std::fs::create_dir_all(&config.data)?;
    let key = match &config.secret_key {
        Some(text) => fleetops_store::secrets::SecretKey::from_base64(text)?,
        None => {
            let path = config.data.join("secret.key");
            let (key, created) = fleetops_store::secrets::SecretKey::load_or_create(&path)?;
            if created {
                tracing::info!(path = %path.display(), "created the secret key; back it up with the database");
            }
            key
        }
    };
    Ok(Store::open(&config.data.join("fleetops.db")).await?.with_secret_key(&key))
}
```

- [ ] **Step 5: Run them to verify they pass**

Run: `cargo test -p fleetops-kube --lib env 2>&1 | tail -3 && cargo test -p fleetops-server 2>&1 | tail -3 && make test-kube 2>&1 | grep "test result"`
Expected: 4 unit tests pass; the server tests pass (2 new); both cluster passes are ok, with 12 tests each.

- [ ] **Step 6: Full suite, lint, commit**

Run: `make lint && make test 2>&1 | tail -3`
Expected: green.

```bash
git add crates/fleetops-kube crates/fleetops-server
git commit -m "feat(kube): kubeconfig and agent cluster environments; server factory and secret key

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 9: Agents say what they serve; the server refuses a mismatch

**Files:**
- Modify: `crates/fleetops-tunnel/src/control.rs` (`Serves`, `HostInfo.serves`)
- Modify: `crates/fleetops-store/src/agents.rs` (`create_agent_environment` takes the kind), and its callers: `crates/fleetops-api/src/routes/envs.rs`, `crates/fleetops-agent/tests/agent.rs`, and store tests
- Modify: `crates/fleetops-api/src/agents/connect.rs` (the mismatch check), `crates/fleetops-api/tests/common/agent.rs` (`hello_serving`)
- Test: `crates/fleetops-tunnel/tests/control.rs` (append), `crates/fleetops-api/tests/agents_connect.rs` (append)

**Interfaces:**
- Consumes: `EnvKind::KubeAgent` (Task 4).
- Produces:
  - `fleetops_tunnel::control::Serves { Docker, Kube }` (serde lowercase; default `Docker`);
  - `HostInfo.serves: Serves` (`#[serde(default)]`);
  - `Store::create_agent_environment(&self, name: &str, kind: EnvKind, join_hash: &str, join_expires_at: i64, now: i64)`;
  - `TestAgent::hello_serving(&mut self, proto: u16, serves: Serves)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/fleetops-tunnel/tests/control.rs`:

```rust
#[test]
fn a_hello_from_an_older_agent_serves_docker() {
    use fleetops_tunnel::control::{HostInfo, Serves};
    let old = r#"{"hostname":"h","os":"linux","arch":"x86_64","docker":"unix:///var/run/docker.sock"}"#;
    let h: HostInfo = serde_json::from_str(old).unwrap();
    assert_eq!(h.serves, Serves::Docker);
    let kube = HostInfo { serves: Serves::Kube, ..HostInfo::default() };
    assert_eq!(serde_json::to_value(&kube).unwrap()["serves"], "kube");
}
```

Append to `crates/fleetops-api/tests/agents_connect.rs`:

```rust
/// A kube-agent environment made directly in the store; returns its join token.
async fn kube_agent_env(t: &TestApp, name: &str) -> String {
    let token = fleetops_api::agents::new_join_token();
    let now = fleetops_core::now_unix();
    let rec = t
        .store
        .create_agent_environment(name, fleetops_core::env::EnvKind::KubeAgent, &fleetops_api::auth_token_hash(&token), now + 900, now)
        .await
        .unwrap();
    t.state.envs.insert(&rec, None).unwrap();
    token
}

#[tokio::test]
async fn a_docker_agent_on_a_kube_environment_is_refused_with_the_reason() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let token = kube_agent_env(&t, "k8s-01").await;
    let mut a = TestAgent::connect(&base(&t).await, &token).await.unwrap();
    a.hello(PROTO_MAJOR).await;
    assert_eq!(a.next().await, Some(Msg::Bye { reason: ByeReason::Incompatible }));
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let env = list.json.as_array().unwrap().iter().find(|e| e["name"] == "k8s-01").unwrap();
    assert_eq!(env["agent"]["last_error"], "agent serves docker; this environment expects kube");
}

#[tokio::test]
async fn a_kube_agent_on_a_kube_environment_is_welcomed() {
    let t = app().await;
    let _admin = t.setup_admin().await;
    let token = kube_agent_env(&t, "k8s-01").await;
    let mut a = TestAgent::connect(&base(&t).await, &token).await.unwrap();
    a.hello_serving(PROTO_MAJOR, fleetops_tunnel::control::Serves::Kube).await;
    match a.welcome().await {
        Msg::Welcome { agent_key: Some(_), env_name, .. } => assert_eq!(env_name, "k8s-01"),
        m => panic!("{m:?}"),
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-tunnel --test control 2>&1 | tail -3; cargo test -p fleetops-api --test agents_connect 2>&1 | tail -3`
Expected: compile errors, because `Serves`, `hello_serving` and the 5-argument `create_agent_environment` don't exist yet.

- [ ] **Step 3: Implement**

In `crates/fleetops-tunnel/src/control.rs`:

```rust
/// What the agent forwards streams to. Agents before sub-project 3 send
/// no value and serve Docker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Serves {
    #[default]
    Docker,
    Kube,
}

impl Serves {
    pub fn as_str(self) -> &'static str {
        match self {
            Serves::Docker => "docker",
            Serves::Kube => "kube",
        }
    }
}
```

Add `#[serde(default)] pub serves: Serves,` to `HostInfo`, after `docker`. Change `HostInfo.docker`'s doc comment to `/// What the agent forwards to: a Docker URL, or "kubernetes".`

In `crates/fleetops-store/src/agents.rs`, `create_agent_environment` takes `kind: fleetops_core::env::EnvKind` after `name`. Its `INSERT` becomes `VALUES (?, ?, '', ?, ?)` with `.bind(name).bind(kind.as_str())`. Update every caller (`rg -n "create_agent_environment\(" crates`):
- `routes/envs.rs` passes `EnvKind::DockerAgent` (Task 11 adds the kube branch);
- the agent tests and store tests pass `EnvKind::DockerAgent`.

In `crates/fleetops-api/tests/common/agent.rs`, turn `hello` into a wrapper:

```rust
    pub async fn hello(&mut self, proto: u16) {
        self.hello_serving(proto, fleetops_tunnel::control::Serves::Docker).await
    }

    pub async fn hello_serving(&mut self, proto: u16, serves: fleetops_tunnel::control::Serves) {
        let host = HostInfo {
            hostname: "test-host".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            docker: "unix:///var/run/docker.sock".into(),
            serves,
        };
        // …the existing write_msg of Msg::Hello { proto, agent_version, host }
    }
```

In `crates/fleetops-api/src/agents/connect.rs` `serve()`, right after the `if proto != PROTO_MAJOR { … }` block, add:

```rust
    let expects = match env.kind.parse::<EnvKind>() {
        Ok(EnvKind::KubeAgent) => Serves::Kube,
        _ => Serves::Docker,
    };
    if info.host.serves != expects {
        let reason = format!(
            "agent serves {}; this environment expects {}",
            info.host.serves.as_str(),
            expects.as_str()
        );
        say_bye(&mut c, session, ByeReason::Incompatible).await;
        let _ = s
            .store
            .record_agent_seen(env_id, now_unix(), Some(&info.agent_version), host_json.as_deref(), Some(&reason))
            .await;
        s.agents.set_offline_reason(env_id, &reason);
        audit(s, None, "agent.reject", "environment", &env.name, "failure").await;
        return reason;
    }
```

Import `fleetops_core::env::EnvKind` and `fleetops_tunnel::control::Serves`.

Agents built from this tree must also send `serves`. In `crates/fleetops-agent/src/session.rs` `host_info`, add `serves: fleetops_tunnel::control::Serves::Docker,`. Task 10 makes it depend on the mode.

- [ ] **Step 4: Run them to verify they pass**

Run: `make test 2>&1 | tail -3`
Expected: all pass, including the 3 new tests.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(tunnel): agents declare what they serve; a mismatch is refused with a reason

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 10: `fleetops-agent --kube`

The agent's in-cluster mode:
- inbound streams go to the kube proxy;
- the token is re-read periodically;
- the agent key can live in a Secret (`--key-secret`) instead of a data directory, because the pod runs with a read-only root filesystem.

**Files:**
- Modify: `crates/fleetops-agent/src/config.rs`, `crates/fleetops-agent/src/lib.rs`, `crates/fleetops-agent/src/session.rs`, `crates/fleetops-agent/src/creds.rs`, `crates/fleetops-agent/src/kube_proxy.rs` (adds `api`), `crates/fleetops-agent/Cargo.toml`
- Modify: `crates/fleetops-agent/tests/agent.rs` (config literal gains the new fields)
- Modify: `Cargo.toml` (workspace dep `tokio-rustls`)
- Create: `crates/fleetops-agent/tests/common/fake_apiserver.rs`, `crates/fleetops-agent/tests/kube_mode.rs`

**Interfaces:**
- Consumes: `KubeProxy` and `KubeUpstream` (Task 1); `Serves` (Task 9); `fleetops_server::build_state` and the kube factory (Task 8).
- Produces:
  - `Config.kube: bool` (`--kube`, `FLEETOPS_AGENT_KUBE`), conflicting with `--docker`;
  - `Config.key_secret: Option<String>` (`--key-secret`, `FLEETOPS_AGENT_KEY_SECRET`; requires `--kube`);
  - `Config.kube_upstream: Option<KubeUpstream>` (`#[arg(skip)]`, tests only);
  - `creds::Keys::{File(PathBuf), Secret(SecretKeys)}` with `async load`, `async save`, `async probe` and `describe`;
  - `KubeProxy::api(&self, method: Method, path: &str, content_type: Option<&str>, body: Vec<u8>) -> io::Result<(StatusCode, Bytes)>`.

- [ ] **Step 1: Write the fake apiserver and the failing tests**

Add to the workspace `Cargo.toml`: `tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12", "logging"] }`.

In `crates/fleetops-agent/Cargo.toml`:
- add to `[dependencies]`: `base64.workspace = true` and `serde_json.workspace = true`;
- add to `[dev-dependencies]`: `fleetops-server = { path = "../fleetops-server" }`, `rcgen.workspace = true`, `tokio-rustls.workspace = true`, `hyper.workspace = true`, `hyper-util.workspace = true`, `http-body-util.workspace = true`, `bytes.workspace = true`, `serde_json.workspace = true` and `base64.workspace = true`.

`crates/fleetops-agent/tests/common/fake_apiserver.rs`:

```rust
//! A TLS apiserver stand-in: answers /version and /api/v1/nodes, keeps
//! Secrets in memory, upgrades `.../exec` to an echo pipe, and records the
//! path and Authorization of every request.
#![allow(dead_code)]

use bytes::Bytes;
use fleetops_agent::kube_proxy::KubeUpstream;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct Seen {
    pub requests: Vec<(String, String)>,
    pub secrets: HashMap<String, Value>,
}

pub struct FakeApi {
    pub dir: tempfile::TempDir,
    pub upstream: KubeUpstream,
    pub seen: Arc<Mutex<Seen>>,
}

impl FakeApi {
    pub fn set_token(&self, t: &str) {
        std::fs::write(self.dir.path().join("token"), t).unwrap();
    }
    pub fn auth_for(&self, path: &str) -> Vec<String> {
        self.seen.lock().unwrap().requests.iter().filter(|(p, _)| p == path).map(|(_, a)| a.clone()).collect()
    }
}

fn json_resp(status: StatusCode, v: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(v.to_string())))
        .unwrap()
}

async fn handle(seen: Arc<Mutex<Seen>>, mut req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_string();
    let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    seen.lock().unwrap().requests.push((path.clone(), auth.clone()));
    if path.ends_with("/exec") && req.headers().contains_key("upgrade") {
        let up = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            if let Ok(up) = up.await {
                let (mut r, mut w) = tokio::io::split(TokioIo::new(up));
                let _ = tokio::io::copy(&mut r, &mut w).await;
            }
        });
        return Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header("connection", "upgrade")
            .header("upgrade", "echo")
            .body(Full::new(Bytes::new()))
            .unwrap();
    }
    if path == "/version" {
        return json_resp(StatusCode::OK, json!({"major": "1", "minor": "32", "gitVersion": "v1.32.0",
            "gitCommit": "", "gitTreeState": "", "buildDate": "", "goVersion": "", "compiler": "", "platform": "linux/amd64"}));
    }
    if path == "/api/v1/nodes" {
        return json_resp(StatusCode::OK, json!({"kind": "NodeList", "apiVersion": "v1", "metadata": {}, "items": []}));
    }
    if let Some(rest) = path.strip_prefix("/api/v1/namespaces/fleetops/secrets") {
        let method = req.method().clone();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let mut s = seen.lock().unwrap();
        let name = rest.trim_start_matches('/').to_string();
        return match (method.as_str(), name.is_empty()) {
            ("GET", false) => match s.secrets.get(&name) {
                Some(v) => json_resp(StatusCode::OK, v.clone()),
                None => json_resp(StatusCode::NOT_FOUND, json!({"kind": "Status", "code": 404})),
            },
            ("POST", true) => {
                let v: Value = serde_json::from_slice(&body).unwrap();
                let n = v["metadata"]["name"].as_str().unwrap().to_string();
                s.secrets.insert(n, v.clone());
                json_resp(StatusCode::CREATED, v)
            }
            ("PATCH", false) => match s.secrets.get_mut(&name) {
                Some(v) => {
                    let patch: Value = serde_json::from_slice(&body).unwrap();
                    v["data"] = patch["data"].clone();
                    json_resp(StatusCode::OK, v.clone())
                }
                None => json_resp(StatusCode::NOT_FOUND, json!({"kind": "Status", "code": 404})),
            },
            _ => json_resp(StatusCode::METHOD_NOT_ALLOWED, json!({})),
        };
    }
    json_resp(StatusCode::OK, json!({"path": path, "authorization": auth}))
}

pub async fn start() -> FakeApi {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()]).unwrap();
    std::fs::write(dir.path().join("ca.crt"), cert.cert.pem()).unwrap();
    std::fs::write(dir.path().join("token"), "good-token").unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (acceptor, seen) = (acceptor.clone(), s2.clone());
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let svc = hyper::service::service_fn(move |req| {
                    let seen = seen.clone();
                    async move { Ok::<_, Infallible>(handle(seen, req).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), svc)
                    .with_upgrades()
                    .await;
            });
        }
    });
    let upstream = KubeUpstream {
        base: format!("https://127.0.0.1:{port}").parse().unwrap(),
        ca: dir.path().join("ca.crt"),
        token: dir.path().join("token"),
    };
    FakeApi { dir, upstream, seen }
}
```

`crates/fleetops-agent/tests/kube_mode.rs`:

```rust
mod common {
    pub mod fake_apiserver;
}

use bytes::Bytes;
use common::fake_apiserver;
use fleetops_agent::creds::{Keys, SecretKeys};
use fleetops_agent::kube_proxy::KubeProxy;
use fleetops_tunnel::http::TunnelConnector;
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode};
use hyper_util::{client::legacy::Client, rt::{TokioExecutor, TokioIo}};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A plain-HTTP client whose connections are tunnel streams served by `proxy`.
fn through(proxy: Arc<KubeProxy>) -> Client<TunnelConnector, Empty<Bytes>> {
    let session = fleetops_tunnel::testing::loopback_with(move |s| {
        let p = proxy.clone();
        tokio::spawn(async move {
            let _ = p.serve(s).await;
        });
    });
    Client::builder(TokioExecutor::new()).build(TunnelConnector::new(Arc::new(session)))
}

async fn get(c: &Client<TunnelConnector, Empty<Bytes>>, path: &str, auth: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut b = Request::get(format!("http://kube-agent{path}"));
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    let r = c.request(b.body(Empty::new()).unwrap()).await.unwrap();
    let status = r.status();
    let body = r.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn the_agents_token_replaces_whatever_the_server_sent() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let (status, body) = get(&c, "/api/v1/pods", Some("Bearer evil")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authorization"], "Bearer good-token");
}

#[tokio::test]
async fn paths_outside_the_api_never_reach_the_apiserver() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let (status, body) = get(&c, "/metrics", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["message"].as_str().unwrap().contains("forwards only Kubernetes API paths"));
    assert!(api.auth_for("/metrics").is_empty());
}

#[tokio::test]
async fn an_upgrade_is_spliced_end_to_end() {
    let api = fake_apiserver::start().await;
    let c = through(KubeProxy::new(api.upstream.clone()).unwrap());
    let req = Request::get("http://kube-agent/api/v1/namespaces/a/pods/p/exec")
        .header("connection", "upgrade")
        .header("upgrade", "echo")
        .body(Empty::new())
        .unwrap();
    let resp = c.request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut io = TokioIo::new(hyper::upgrade::on(resp).await.unwrap());
    io.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    io.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
}

#[tokio::test]
async fn a_rotated_token_file_is_picked_up() {
    let api = fake_apiserver::start().await;
    let proxy = KubeProxy::new(api.upstream.clone()).unwrap();
    let c = through(proxy.clone());
    api.set_token("newer-token");
    proxy.refresh_token().unwrap();
    let (_, body) = get(&c, "/api/v1/pods", None).await;
    assert_eq!(body["authorization"], "Bearer newer-token");
}

#[tokio::test]
async fn the_secret_key_store_creates_then_updates_its_secret() {
    let api = fake_apiserver::start().await;
    let keys = Keys::Secret(SecretKeys::new(
        KubeProxy::new(api.upstream.clone()).unwrap(),
        "fleetops".into(),
        "fleetops-agent-key".into(),
    ));
    keys.probe().await.unwrap();
    assert_eq!(keys.load().await.unwrap(), None);
    keys.save("fk_one").await.unwrap();
    assert_eq!(keys.load().await.unwrap().as_deref(), Some("fk_one"));
    keys.save("fk_two").await.unwrap();
    assert_eq!(keys.load().await.unwrap().as_deref(), Some("fk_two"));
}

#[tokio::test]
async fn a_kube_agent_brings_its_cluster_up_and_keeps_its_key_in_a_secret() {
    use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion};
    std::env::set_var("POD_NAMESPACE", "fleetops");
    let api = fake_apiserver::start().await;
    let store = fleetops_store::Store::in_memory().await.unwrap();
    let state = fleetops_server::build_state(store.clone(), fleetops_api::ApiConfig::default());
    let token = fleetops_api::agents::new_join_token();
    let now = fleetops_core::now_unix();
    store
        .create_agent_environment("k8s-01", EnvKind::KubeAgent, &fleetops_api::auth_token_hash(&token), now + 900, now)
        .await
        .unwrap();
    state.envs.load(&store).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = fleetops_api::app(state.clone(), axum::Router::new());
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });

    let data = tempfile::tempdir().unwrap();
    let config = fleetops_agent::Config {
        server: base,
        join_token: Some(token),
        data: data.path().to_path_buf(),
        docker: "unix:///var/run/docker.sock".into(),
        ca: None,
        log: "warn".into(),
        server_silence: std::time::Duration::from_secs(45),
        kube: true,
        key_secret: Some("fleetops-agent-key".into()),
        kube_upstream: Some(api.upstream.clone()),
    };
    tokio::spawn(fleetops_agent::run(config));

    for _ in 0..100 {
        if state.agents.connected() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(state.agents.connected(), 1);
    let env = state.envs.get("k8s-01").unwrap();
    match env.status().await {
        EnvStatus::Up { version: EnvVersion::Kube(v) } => assert_eq!(v.git_version, "v1.32.0"),
        other => panic!("{other:?}"),
    }
    assert!(api.auth_for("/version").iter().all(|a| a == "Bearer good-token"));
    let secrets = api.seen.lock().unwrap().secrets.clone();
    assert!(secrets.contains_key("fleetops-agent-key"), "the key went to the Secret");
    assert!(std::fs::read_dir(data.path()).unwrap().next().is_none(), "nothing written to --data");
}
```

Match the existing `Config` literal in `tests/agent.rs::config()` for the field list. Add `kube: false, key_secret: None, kube_upstream: None` there.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-agent --test kube_mode 2>&1 | grep -E "^error" | head -5`
Expected: compile errors, because `Keys`, `SecretKeys` and the `Config.kube` fields are not defined.

- [ ] **Step 3: Implement**

`crates/fleetops-agent/src/config.rs`: add the three fields to `Config`:

```rust
    /// Serve the in-cluster Kubernetes API instead of Docker; run as a pod.
    #[arg(long, env = "FLEETOPS_AGENT_KUBE", conflicts_with = "docker")]
    pub kube: bool,
    /// Keep the agent key in this Secret (in $POD_NAMESPACE), not in --data.
    #[arg(long, env = "FLEETOPS_AGENT_KEY_SECRET", requires = "kube")]
    pub key_secret: Option<String>,
    /// Tests: the apiserver to use instead of the in-cluster one.
    #[arg(skip)]
    pub kube_upstream: Option<crate::kube_proxy::KubeUpstream>,
```

In `crates/fleetops-agent/src/kube_proxy.rs`, add to `impl KubeProxy`:

```rust
    /// One request of the agent's own (its key Secret), with its token.
    pub async fn api(
        &self,
        method: hyper::Method,
        path: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> io::Result<(StatusCode, Bytes)> {
        let other = |e: &dyn std::fmt::Display| io::Error::other(e.to_string());
        let uri = format!("{}{path}", self.up.base.to_string().trim_end_matches('/'));
        let token = self.token.read().unwrap().clone();
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json");
        if let Some(ct) = content_type {
            b = b.header(header::CONTENT_TYPE, ct);
        }
        let req = b
            .body(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed())
            .map_err(|e| other(&e))?;
        let resp = tokio::time::timeout(Duration::from_secs(10), self.client.request(req))
            .await
            .map_err(|_| other(&"the kube-apiserver did not answer within 10s"))?
            .map_err(|e| other(&e))?;
        let status = resp.status();
        let bytes = resp.into_body().collect().await.map_err(|e| other(&e))?.to_bytes();
        Ok((status, bytes))
    }
```

In `crates/fleetops-agent/src/creds.rs`, keep the file functions and add:

```rust
use crate::kube_proxy::KubeProxy;
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::{Method, StatusCode};
use std::{path::PathBuf, sync::Arc};

/// Where the agent keeps its key: a file in --data, or a Secret.
pub enum Keys {
    File(PathBuf),
    Secret(SecretKeys),
}

impl Keys {
    pub async fn load(&self) -> io::Result<Option<String>> {
        match self {
            Keys::File(dir) => load_key(dir),
            Keys::Secret(s) => s.load().await,
        }
    }

    pub async fn save(&self, key: &str) -> io::Result<()> {
        match self {
            Keys::File(dir) => save_key(dir, key),
            Keys::Secret(s) => s.save(key).await,
        }
    }

    /// Fails unless a key could be saved, so a join token is never spent
    /// by an agent that would lose the key it gets for it.
    pub async fn probe(&self) -> io::Result<()> {
        match self {
            Keys::File(dir) => probe_writable(dir),
            Keys::Secret(s) => s.probe().await,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Keys::File(dir) => format!("the data directory {}", dir.display()),
            Keys::Secret(s) => format!("the Secret {}/{}", s.namespace, s.name),
        }
    }
}

/// The key in Secret `name`, field `key`, through the apiserver.
pub struct SecretKeys {
    proxy: Arc<KubeProxy>,
    namespace: String,
    name: String,
}

const FIELD: &str = "key";

impl SecretKeys {
    pub fn new(proxy: Arc<KubeProxy>, namespace: String, name: String) -> Self {
        Self { proxy, namespace, name }
    }

    fn collection(&self) -> String {
        format!("/api/v1/namespaces/{}/secrets", self.namespace)
    }

    fn path(&self) -> String {
        format!("{}/{}", self.collection(), self.name)
    }

    fn fail(&self, what: &str, status: StatusCode, body: &[u8]) -> io::Error {
        io::Error::other(format!(
            "{what} Secret {}/{}: HTTP {status}: {}",
            self.namespace,
            self.name,
            String::from_utf8_lossy(body)
        ))
    }

    pub async fn load(&self) -> io::Result<Option<String>> {
        let (status, body) = self.proxy.api(Method::GET, &self.path(), None, vec![]).await?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::OK => {
                let v: serde_json::Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
                let Some(b64) = v["data"][FIELD].as_str() else { return Ok(None) };
                let bytes = STANDARD.decode(b64).map_err(io::Error::other)?;
                Ok(Some(String::from_utf8_lossy(&bytes).trim().to_string()).filter(|k| !k.is_empty()))
            }
            s => Err(self.fail("reading", s, &body)),
        }
    }

    pub async fn save(&self, key: &str) -> io::Result<()> {
        let data = serde_json::json!({ "data": { FIELD: STANDARD.encode(key) } });
        let (status, body) = self
            .proxy
            .api(Method::PATCH, &self.path(), Some("application/merge-patch+json"), data.to_string().into_bytes())
            .await?;
        match status {
            StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => {
                let secret = serde_json::json!({
                    "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
                    "metadata": { "name": self.name },
                    "data": { FIELD: STANDARD.encode(key) }
                });
                let (status, body) = self
                    .proxy
                    .api(Method::POST, &self.collection(), Some("application/json"), secret.to_string().into_bytes())
                    .await?;
                if status == StatusCode::CREATED || status == StatusCode::OK {
                    Ok(())
                } else {
                    Err(self.fail("creating", status, &body))
                }
            }
            s => Err(self.fail("updating", s, &body)),
        }
    }

    /// The Secret must be readable (present or not): checks RBAC up front.
    pub async fn probe(&self) -> io::Result<()> {
        let (status, body) = self.proxy.api(Method::GET, &self.path(), None, vec![]).await?;
        match status {
            StatusCode::OK | StatusCode::NOT_FOUND => Ok(()),
            s => Err(self.fail("reading", s, &body)),
        }
    }
}
```

Make `namespace` and `name` `pub(crate)` so `describe` can read them.

`crates/fleetops-agent/src/session.rs`:
- add `#[derive(Clone)] pub enum Inbound { Docker(Target), Kube(Arc<KubeProxy>) }`;
- `run(ws, inbound: Inbound, target_desc: &str, keys: &Keys, silence: Duration)` spawns per stream:

```rust
    let serves = match &inbound {
        Inbound::Docker(_) => Serves::Docker,
        Inbound::Kube(_) => Serves::Kube,
    };
    let session = Session::spawn(tungstenite_io(ws), Role::Client, move |stream| match inbound.clone() {
        Inbound::Docker(target) => {
            tokio::spawn(async move {
                if let Err(e) = forward(stream, &target).await {
                    tracing::debug!(error = %e, "stream to Docker ended");
                }
            });
        }
        Inbound::Kube(proxy) => {
            tokio::spawn(async move {
                if let Err(e) = proxy.serve(stream).await {
                    tracing::debug!(error = %e, "stream to the apiserver ended");
                }
            });
        }
    });
```

- `host_info(target_desc, serves)` sets `docker: target_desc.into(), serves`;
- `control(...)` saves with `keys.save(&key).await`. The error text becomes `"enrolled, but could not save the key in {}: {e}; rotate the agent in the UI and start again"` with `keys.describe()`.

`crates/fleetops-agent/src/lib.rs` `run`:

```rust
pub async fn run(config: Config) -> anyhow::Result<()> {
    let (inbound, desc, proxy) = if config.kube {
        let up = match &config.kube_upstream {
            Some(u) => u.clone(),
            None => kube_proxy::KubeUpstream::in_cluster()?,
        };
        let proxy = kube_proxy::KubeProxy::new(up)?;
        proxy.spawn_refresh(kube_proxy::TOKEN_REFRESH);
        (session::Inbound::Kube(proxy.clone()), "kubernetes".to_string(), Some(proxy))
    } else {
        let target = fleetops_tunnel::Target::parse(&config.docker)?;
        (session::Inbound::Docker(target), config.docker.clone(), None)
    };
    let keys = match (&config.key_secret, proxy) {
        (Some(name), Some(proxy)) => {
            let ns = std::env::var("POD_NAMESPACE")
                .context("POD_NAMESPACE must be set to keep the key in a Secret")?;
            creds::Keys::Secret(creds::SecretKeys::new(proxy, ns, name.clone()))
        }
        (Some(_), None) => return Err(anyhow!("--key-secret needs --kube")),
        (None, _) => creds::Keys::File(config.data.clone()),
    };
    keys.probe()
        .await
        .with_context(|| format!("{} must be writable", keys.describe()))?;
    let tls = Arc::new(dial::tls_config(config.ca.as_deref())?);
    // …the existing loop, with `keys.load().await?` in place of
    // `creds::load_key(&config.data)?`, and
    // `session::run(ws, inbound.clone(), &desc, &keys, config.server_silence)`.
}
```

The existing test `unwritable_data_dir_fails_before_connecting` checks the error text. The text is now `the data directory <dir> must be writable`, which keeps it passing.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-agent 2>&1 | tail -5`
Expected: all pass, including the 6 new `kube_mode` tests.

- [ ] **Step 5: Lint, full suite, commit**

Run: `make lint && make test 2>&1 | tail -3`
Expected: green.

```bash
git add Cargo.toml Cargo.lock crates/fleetops-agent
git commit -m "feat(agent): --kube mode with token injection, upgrade splicing and a Secret key store

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 11: Creating and changing Kubernetes environments

**Files:**
- Create: `crates/fleetops-api/src/agents/manifest.rs`
- Modify: `crates/fleetops-api/src/agents/mod.rs` (`pub mod manifest;`, `command_for`, `EnvView.kube`), `crates/fleetops-api/src/routes/envs.rs`, `crates/fleetops-api/src/routes/mod.rs` (register `kube_contexts`), `crates/fleetops-api/src/openapi.rs` (schemas), `crates/fleetops-api/Cargo.toml` (`fleetops-kube`)
- Modify: `crates/fleetops-api/tests/openapi.rs` (`OPERATIONS` 61 → 62), `crates/fleetops-api/openapi.json`, `web/src/api/gen/*`
- Test: `crates/fleetops-api/tests/kube_envs.rs`, plus unit tests in `manifest.rs`

**Interfaces:**
- Consumes:
  - `kubeconfig::{prepare, check, contexts}` (Task 5);
  - `Store::create_environment_with_credential`, `put_env_credential`, `env_credential` and `env_credential_summaries` (Task 3);
  - `EnvRegistry::insert` and `replace` with a credential (Task 4);
  - `create_agent_environment(name, kind, …)` (Task 9).
- Produces:
  - `CreateEnv { name, kind, url?, kubeconfig?, context?, skip_check }` and `UpdateEnv { name?, url?, kubeconfig?, context?, skip_check }`;
  - `POST /api/v1/kube/contexts` with `{kubeconfig}`, returning `KubeContexts { contexts, current }` (operation `kubeContexts`);
  - `EnvView.kube: Option<KubeconfigSummary>`;
  - `agents::command_for(kind: EnvKind, server_url: &str, join_token: &str) -> String`;
  - `agents::manifest::{command, manifest, NAMESPACE, KEY_SECRET}`.

The contexts endpoint lives at `/kube/contexts`, not `/envs/kube/contexts` as the spec says. An environment named `kube` would otherwise collide with `/envs/{env}/…` routes. Record this as a ruling and update spec §4.1 and §5 in Task 17.

- [ ] **Step 1: Write the failing tests**

Unit tests at the bottom of a new `crates/fleetops-api/src/agents/manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_installs_a_namespaced_agent_with_its_join_token() {
        let m = manifest("https://fleet.example", "fj_abc");
        let docs: Vec<&str> = m.split("\n---\n").collect();
        let kinds: Vec<&str> = docs
            .iter()
            .map(|d| d.lines().find_map(|l| l.strip_prefix("kind: ")).unwrap())
            .collect();
        assert_eq!(
            kinds,
            ["Namespace", "ServiceAccount", "ClusterRole", "ClusterRoleBinding", "Role", "RoleBinding", "Secret", "Deployment"]
        );
        let with_token: Vec<&str> = docs.iter().filter(|d| d.contains("fj_abc")).map(|d| *d).collect();
        assert_eq!(with_token.len(), 1, "the token is only in the join Secret");
        assert!(with_token[0].contains("kind: Secret"));
        assert!(m.contains("- --kube") && m.contains("- https://fleet.example") && m.contains("- --key-secret"));
        assert!(m.contains("readOnlyRootFilesystem: true") && m.contains("type: Recreate"));
        assert!(!m.contains("docker.sock"));
    }

    #[test]
    fn a_new_token_changes_the_pod_template_so_reapplying_restarts_the_agent() {
        let a = manifest("https://f", "fj_one");
        let b = manifest("https://f", "fj_two");
        let ann = |m: &str| m.lines().find(|l| l.contains("fleetops.io/join:")).unwrap().to_string();
        assert_ne!(ann(&a), ann(&b));
    }

    #[test]
    fn the_command_pipes_the_manifest_to_kubectl() {
        let c = command("https://f", "fj_x");
        assert!(c.starts_with("kubectl apply -f - <<'EOF'\n") && c.ends_with("EOF\n"), "{c}");
    }
}
```

`crates/fleetops-api/tests/kube_envs.rs`:

```rust
mod common;

use common::*;
use fleetops_core::env::{Credential, EnvKind};
use serde_json::json;

/// A token kubeconfig for a cluster nobody listens on.
pub const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\ncurrent-context: dev\nclusters:\n- name: dev\n  cluster: {server: 'https://127.0.0.1:1'}\ncontexts:\n- name: dev\n  context: {cluster: dev, user: dev}\n- name: other\n  context: {cluster: dev, user: dev}\nusers:\n- name: dev\n  user: {token: very-secret-token}\n";

fn post(path: &str, body: serde_json::Value, s: &Session) -> axum::http::Request<axum::body::Body> {
    let mut r = req("POST", path, Some(body), Some(s));
    r.headers_mut().insert("origin", "https://fleet.example".parse().unwrap());
    r
}

#[tokio::test]
async fn a_kubeconfig_environment_is_stored_encrypted_and_never_returned() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t
        .send(post("/api/v1/envs", json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true}), &admin))
        .await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    assert_eq!(r.json["url"], "https://127.0.0.1:1");
    assert!(!r.json.to_string().contains("very-secret-token"));
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    assert!(!list.json.to_string().contains("very-secret-token"));
    let k = list.json.as_array().unwrap().iter().find(|e| e["name"] == "k").unwrap();
    assert_eq!(k["kube"], json!({"server": "https://127.0.0.1:1", "context": "dev", "user_kind": "token"}));
    let spec = t.fake.built().into_iter().rev().find(|s| s.name == "k").unwrap();
    assert_eq!(spec.kind, EnvKind::KubeConfig);
    assert!(matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("very-secret-token") && !y.contains("other")));
}

#[tokio::test]
async fn an_unreachable_cluster_is_refused_unless_the_check_is_skipped() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t.send(post("/api/v1/envs", json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG}), &admin)).await;
    assert_eq!(r.status, 400);
    assert!(r.json["message"].as_str().unwrap().contains("cannot reach cluster"), "{:?}", r.json);
    assert!(t.store.list_environments().await.unwrap().is_empty(), "nothing stored");
}

#[tokio::test]
async fn bad_kubeconfigs_are_refused_with_the_reason() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let exec = KUBECONFIG.replace("{token: very-secret-token}", "{exec: {apiVersion: v1, command: aws}}");
    for (body, needle) in [
        (json!({"name": "k", "kind": "kube-config", "kubeconfig": exec, "skip_check": true}), "exec and auth-provider users are not supported"),
        (json!({"name": "k", "kind": "kube-config", "skip_check": true}), "a kubeconfig is required"),
        (json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "url": "https://x", "skip_check": true}), "takes no URL"),
        (json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "context": "nope", "skip_check": true}), "unknown context 'nope'"),
    ] {
        let r = t.send(post("/api/v1/envs", body, &admin)).await;
        assert_eq!(r.status, 400);
        assert!(r.json["message"].as_str().unwrap().contains(needle), "{needle}: {:?}", r.json);
    }
}

#[tokio::test]
async fn contexts_lists_a_kubeconfigs_contexts_for_admins_only() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t.send(post("/api/v1/kube/contexts", json!({"kubeconfig": KUBECONFIG}), &admin)).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json, json!({"contexts": ["dev", "other"], "current": "dev"}));
    let user = t.create_user(&admin, "bob", "user").await;
    let r = t.send(post("/api/v1/kube/contexts", json!({"kubeconfig": KUBECONFIG}), &user)).await;
    assert_eq!(r.status, 403);
}

#[tokio::test]
async fn a_kube_agent_environment_gets_a_kubectl_manifest() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t.send(post("/api/v1/envs", json!({"name": "k8s-01", "kind": "kube-agent"}), &admin)).await;
    assert_eq!(r.status, 201, "{:?}", r.json);
    let token = r.json["enrollment"]["join_token"].as_str().unwrap();
    let cmd = r.json["enrollment"]["command"].as_str().unwrap();
    assert!(cmd.starts_with("kubectl apply -f - <<'EOF'"), "{cmd}");
    assert!(cmd.contains(token) && cmd.contains("- https://fleet.example"));
    assert_eq!(r.json["kind"], "kube-agent");
    let rot = t.send(post("/api/v1/envs/k8s-01/agent/rotate", json!({}), &admin)).await;
    assert_eq!(rot.status, 200, "{:?}", rot.json);
    assert!(rot.json["command"].as_str().unwrap().starts_with("kubectl apply"));
}

#[tokio::test]
async fn updating_a_kubeconfig_replaces_it_and_rebuilds_the_environment() {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.send(post("/api/v1/envs", json!({"name": "k", "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true}), &admin)).await;
    let newer = KUBECONFIG.replace("127.0.0.1:1", "127.0.0.1:2").replace("very-secret-token", "newer-token");
    let r = t
        .send(req("PATCH", "/api/v1/envs/k", Some(json!({"kubeconfig": newer, "skip_check": true})), Some(&admin)))
        .await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["url"], "https://127.0.0.1:2");
    let spec = t.fake.built().into_iter().rev().find(|s| s.name == "k").unwrap();
    assert!(matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("newer-token")));
    // A rename alone keeps the stored kubeconfig.
    t.send(req("PATCH", "/api/v1/envs/k", Some(json!({"name": "k2"})), Some(&admin))).await;
    let spec = t.fake.built().into_iter().rev().find(|s| s.name == "k2").unwrap();
    assert!(matches!(spec.credential, Some(Credential::Kubeconfig(y)) if y.contains("newer-token")));
    let bad = t.send(req("PATCH", "/api/v1/envs/k2", Some(json!({"url": "https://x"})), Some(&admin))).await;
    assert_eq!(bad.status, 400);
}
```

Use the signature of `create_user` in `tests/common/mod.rs`. It returns a logged-in `Session` for a user with role `user`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test kube_envs 2>&1 | tail -5`
Expected: the tests fail, either from a missing module or with 400 "Kubernetes environments are not available yet".

- [ ] **Step 3: Implement the manifest**

Above the tests in `crates/fleetops-api/src/agents/manifest.rs`:

```rust
//! The `kubectl apply` command that installs the agent in a cluster
//! (spec §4.2). One replica with the Recreate strategy: two agents with
//! one key would replace each other. The pod template carries a hash of
//! the join token, so re-applying after a rotate restarts the agent.

use super::AGENT_IMAGE;
use sha2::{Digest, Sha256};

pub const NAMESPACE: &str = "fleetops";
pub const KEY_SECRET: &str = "fleetops-agent-key";

pub fn command(server_url: &str, join_token: &str) -> String {
    format!("kubectl apply -f - <<'EOF'\n{}\nEOF\n", manifest(server_url, join_token))
}

pub fn manifest(server_url: &str, join_token: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let join_hash = &hex::encode(Sha256::digest(join_token.as_bytes()))[..12];
    format!(
        r#"apiVersion: v1
kind: Namespace
metadata:
  name: {NAMESPACE}
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: fleetops-agent
  namespace: {NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: fleetops-agent
rules:
  - apiGroups: [""]
    resources: [namespaces, nodes]
    verbs: [get, list, watch]
  - apiGroups: [""]
    resources: [pods, services, configmaps, secrets, events]
    verbs: [get, list, watch, delete]
  - apiGroups: [""]
    resources: [pods/log]
    verbs: [get]
  - apiGroups: [""]
    resources: [pods/exec]
    verbs: [get, create]
  - apiGroups: [apps]
    resources: [deployments, statefulsets, daemonsets]
    verbs: [get, list, watch, delete, patch]
  - apiGroups: [apps]
    resources: [deployments/scale, statefulsets/scale]
    verbs: [get, patch]
  - apiGroups: [apps]
    resources: [replicasets]
    verbs: [get]
  - apiGroups: [batch]
    resources: [jobs, cronjobs]
    verbs: [get, list, watch, delete]
  - apiGroups: [networking.k8s.io]
    resources: [ingresses]
    verbs: [get, list, watch, delete]
  - nonResourceURLs: [/version]
    verbs: [get]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: fleetops-agent
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: fleetops-agent
subjects:
  - kind: ServiceAccount
    name: fleetops-agent
    namespace: {NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: fleetops-agent-key
  namespace: {NAMESPACE}
rules:
  - apiGroups: [""]
    resources: [secrets]
    resourceNames: [{KEY_SECRET}]
    verbs: [get, patch]
  - apiGroups: [""]
    resources: [secrets]
    verbs: [create]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: fleetops-agent-key
  namespace: {NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: fleetops-agent-key
subjects:
  - kind: ServiceAccount
    name: fleetops-agent
    namespace: {NAMESPACE}
---
apiVersion: v1
kind: Secret
metadata:
  name: fleetops-agent-join
  namespace: {NAMESPACE}
type: Opaque
stringData:
  token: {join_token}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: fleetops-agent
  namespace: {NAMESPACE}
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels: {{app: fleetops-agent}}
  template:
    metadata:
      labels: {{app: fleetops-agent}}
      annotations:
        fleetops.io/join: "{join_hash}"
    spec:
      serviceAccountName: fleetops-agent
      securityContext:
        runAsNonRoot: true
        seccompProfile: {{type: RuntimeDefault}}
      containers:
        - name: agent
          image: {AGENT_IMAGE}:{version}
          args:
            - --kube
            - --server
            - {server_url}
            - --key-secret
            - {KEY_SECRET}
          env:
            - name: FLEETOPS_AGENT_JOIN_TOKEN
              valueFrom: {{secretKeyRef: {{name: fleetops-agent-join, key: token}}}}
            - name: POD_NAMESPACE
              valueFrom: {{fieldRef: {{fieldPath: metadata.namespace}}}}
          securityContext:
            readOnlyRootFilesystem: true
            allowPrivilegeEscalation: false
            capabilities: {{drop: [ALL]}}
          resources:
            requests: {{cpu: 10m, memory: 16Mi}}
            limits: {{memory: 64Mi}}"#
    )
}
```

If `hex` or `sha2` is missing from `crates/fleetops-api/Cargo.toml`, add it (`hash_token` already uses SHA-256, so they are probably there).

- [ ] **Step 4: Implement the routes**

In `crates/fleetops-api/src/agents/mod.rs`:
- add `pub mod manifest;`;
- add `#[serde(default)] pub kube: Option<fleetops_core::env::KubeconfigSummary>,` to `EnvView`, with doc comment `/// What may be shown of a stored kubeconfig (kube-config environments).`;
- add:

```rust
/// The enrollment command for an agent environment of `kind`.
pub fn command_for(kind: EnvKind, server_url: &str, join_token: &str) -> String {
    match kind {
        EnvKind::KubeAgent => manifest::command(server_url, join_token),
        _ => enrollment_command(server_url, join_token),
    }
}
```

In `crates/fleetops-api/src/routes/envs.rs`:

```rust
#[derive(Deserialize, ToSchema)]
pub struct CreateEnv {
    pub name: String,
    pub kind: EnvKind,
    /// The Docker URL of a local environment; other kinds take none.
    #[serde(default)]
    pub url: Option<String>,
    /// kube-config: the kubeconfig, stored encrypted and never returned.
    #[serde(default)]
    pub kubeconfig: Option<String>,
    /// kube-config: the context to use; default the current one.
    #[serde(default)]
    pub context: Option<String>,
    /// kube-config: save without checking that the cluster answers.
    #[serde(default)]
    pub skip_check: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateEnv {
    pub name: Option<String>,
    pub url: Option<String>,
    /// kube-config: a replacement kubeconfig.
    #[serde(default)]
    pub kubeconfig: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub skip_check: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct KubeconfigText {
    pub kubeconfig: String,
}

#[derive(Serialize, ToSchema)]
pub struct KubeContexts {
    pub contexts: Vec<String>,
    pub current: Option<String>,
}

/// Validates and minifies a kubeconfig, then checks the cluster answers
/// unless told not to. Runs before the mutation lock: the check can take
/// seconds.
async fn prepare_kubeconfig(
    text: &str,
    context: Option<&str>,
    skip_check: bool,
) -> ApiResult<kubeconfig::Prepared> {
    let p = kubeconfig::prepare(text, context)?;
    if !skip_check {
        kubeconfig::check(&p.yaml).await?;
    }
    Ok(p)
}

fn summary_json(p: &kubeconfig::Prepared) -> serde_json::Value {
    serde_json::to_value(&p.summary).expect("summary serialises")
}
```

with `use fleetops_kube::kubeconfig;`, `use fleetops_core::env::Credential;` and `use serde::Serialize;`.

`create`:
1. Before `let _guard`, add:
   ```rust
   let prepared = match (body.kind, &body.kubeconfig) {
       (EnvKind::KubeConfig, Some(text)) => Some(prepare_kubeconfig(text, body.context.as_deref(), body.skip_check).await?),
       (EnvKind::KubeConfig, None) => return Err(Error::Invalid("a kubeconfig is required".into()).into()),
       (_, Some(_)) => return Err(Error::Invalid("only a kube-config environment takes a kubeconfig".into()).into()),
       (_, None) => None,
   };
   ```
   Also refuse a URL on the kube kinds up front:
   ```rust
   if body.kind.is_kube() && body.url.is_some() {
       return Err(Error::Invalid("a Kubernetes environment takes no URL; paste a kubeconfig".into()).into());
   }
   ```
   Put the URL check first, so a request with both a URL and a kubeconfig gets the URL message.
2. The `match body.kind` returns `(rec, enrollment, credential)`:
   - `DockerLocal`: as before, with `None` for the credential.
   - `DockerAgent | KubeAgent`: the existing agent arm, with `create_agent_environment(&body.name, body.kind, …)` and `agents::command_for(body.kind, &server, &token)`.
   - `KubeConfig`:
     ```rust
     EnvKind::KubeConfig => {
         let p = prepared.expect("checked above");
         let rec = s
             .store
             .create_environment_with_credential(&body.name, body.kind.as_str(), &p.summary.server, "kubeconfig", &p.yaml, &summary_json(&p), now_unix())
             .await?;
         (rec, None, Some(Credential::Kubeconfig(p.yaml)))
     }
     ```
3. Then call `s.envs.insert(&rec, credential)?;`.

`update`:
1. Parse `let kind: EnvKind = current.kind.parse()?;`.
2. Refuse a URL on `DockerAgent` or `is_kube()` kinds with the existing message for agents, or `"a Kubernetes environment's URL comes from its kubeconfig"` for kube kinds.
3. Refuse `kubeconfig` on any kind but `KubeConfig` with `"only a kube-config environment takes a kubeconfig"`.
4. Prepare a replacement kubeconfig. The lock is taken earlier in this handler, so first move `let _guard = s.envs.lock_mutations().await;` below this preparation step, and call `find(&s, &name)` twice: once before preparing, to learn the kind, and once again under the lock.
   ```rust
   let prepared = match &body.kubeconfig {
       Some(t) => Some(prepare_kubeconfig(t, body.context.as_deref(), body.skip_check).await?),
       None => None,
   };
   ```
5. `new_url` is `prepared.summary.server` when `prepared` is set, otherwise as before.
6. When `prepared` is set, call `s.store.put_env_credential(current.id, "kubeconfig", &p.yaml, &summary_json(&p), now_unix()).await?` before `update_environment`.
7. Pass the credential to `replace`: `Some(Credential::Kubeconfig(p.yaml))` when there is a replacement, otherwise `s.store.env_credential(rec.id).await?`.

`rotate_agent`: accept `docker-agent` and `kube-agent`. `new_enrollment` gains a `kind: EnvKind` parameter and builds its command with `agents::command_for(kind, …)`.

`list`: read `let summaries = s.store.env_credential_summaries().await?;`, and set `kube: summaries.get(&env.id).and_then(|v| serde_json::from_value(v.clone()).ok())` in each `EnvView`.

Add the contexts handler:

```rust
#[utoipa::path(
    post,
    path = "/kube/contexts",
    operation_id = "kubeContexts",
    tag = "environments",
    summary = "List a kubeconfig's contexts without saving it",
    request_body = KubeconfigText,
    responses((status = 200, description = "Context names and the current one", body = KubeContexts))
)]
pub async fn kube_contexts(
    State(_s): State<AppState>,
    cu: CurrentUser,
    ApiJson(body): ApiJson<KubeconfigText>,
) -> ApiResult<Json<KubeContexts>> {
    authorize(&cu.user, None, Action::EnvManage)?;
    let c = kubeconfig::contexts(&body.kubeconfig)?;
    Ok(Json(KubeContexts { contexts: c.contexts, current: c.current }))
}
```

Register it in `crates/fleetops-api/src/routes/mod.rs` `api()` with `.routes(routes!(envs::kube_contexts))`. Add `KubeconfigSummary`, `UserKind`, `KubeContexts` and `KubeconfigText` to the `components(schemas(...))` in `openapi.rs` if utoipa doesn't pick them up through the handlers.

Add `fleetops-kube.workspace = true` to `crates/fleetops-api/Cargo.toml`.

- [ ] **Step 5: Run them to verify they pass, regenerate the document**

Run: `cargo test -p fleetops-api --test kube_envs 2>&1 | tail -3 && cargo test -p fleetops-api --lib manifest 2>&1 | tail -3`
Expected: 6 and 3 passed.

Then set `const OPERATIONS: usize = 62;` in `crates/fleetops-api/tests/openapi.rs`, and run `make openapi && make lint && make test 2>&1 | tail -3 && make test-web 2>&1 | tail -3`.
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add -A crates web/src/api/gen
git commit -m "feat(api): kubeconfig and agent cluster environments, contexts, kubectl enrollment

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 12: The `/envs/{env}/k8s` REST routes

**Files:**
- Create: `crates/fleetops-api/src/routes/kube.rs`
- Modify: `crates/fleetops-api/src/routes/mod.rs` (`mod kube;`, register), `crates/fleetops-api/src/routes/docker/mod.rs` (409 for kube envs), `crates/fleetops-api/src/openapi.rs`
- Create: `crates/fleetops-api/tests/common/fake_kube.rs`; Modify: `crates/fleetops-api/tests/common/mod.rs` (`pub mod fake_kube;`, `add_kube_env`), `crates/fleetops-api/tests/common/fake_docker.rs` (`FakeDocker.kube`, `FakeFactory` builds `FakeKubeEnv`)
- Modify: `crates/fleetops-api/tests/openapi.rs` (`OPERATIONS` 62 → 68), `crates/fleetops-api/openapi.json`, `web/src/api/gen/*`
- Test: `crates/fleetops-api/tests/kube_routes.rs`

**Interfaces:**
- Consumes: `KubeOps`, `KubeKind` and the DTOs (Task 2); `Action::KubeRead`/`KubeWrite` (Task 4); `kube_envs.rs`'s `KUBECONFIG` pattern (Task 11).
- Produces:
  - routes (operation ids):
    - `GET /envs/{env}/k8s/{kind}?namespace=` (`listKube`);
    - `GET /envs/{env}/k8s/{kind}/{ns}/{name}` (`getKube`);
    - `DELETE` on the same path (`deleteKube`);
    - `GET …/events` (`kubeEvents`);
    - `POST …/scale` with `{replicas}` (`scaleKube`);
    - `POST …/restart` (`restartKube`).
  - `pub(crate) fn routes::kube::kube_for(s, cu, env_name, action) -> Result<(Arc<dyn Environment>, Arc<dyn KubeOps>)>`, also used by the WS topics in Task 13.
  - `pub(crate) fn routes::kube::wrong_kind(name: &str, kind: EnvKind) -> Error`.
  - Test helpers: `FakeKube` (`object`, `set_forbidden`, `set_truncated`, `calls`, `set_logs`, `take_exec`), `FakeKubeEnv`, `ExecHandle`, and `TestApp::add_kube_env(&Session, name)`.

The spec's names-only `GET /envs/{env}/k8s/namespaces` collides with the list route for the `namespaces` kind. Dropping it means the namespace picker uses `GET …/k8s/namespaces` and reads the row names. Record this as a ruling and update spec §5 in Task 17.

- [ ] **Step 1: Write the fake and the failing tests**

`crates/fleetops-api/tests/common/fake_kube.rs`:

```rust
//! A scripted cluster for API tests.
#![allow(dead_code)]

use async_trait::async_trait;
use fleetops_core::docker::DockerOps;
use fleetops_core::env::{EnvKind, EnvStatus, EnvVersion, Environment};
use fleetops_core::kube::*;
use fleetops_core::streams::{BoxStream, ExecOptions, LogLine, LogOptions, LogStream};
use fleetops_core::{Error, Result};
use std::collections::{BTreeMap, HashSet};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

/// The test's side of a fake exec.
pub struct ExecHandle {
    pub target: String,
    pub opts: ExecOptions,
    pub output: mpsc::Sender<Result<Vec<u8>>>,
    pub input: mpsc::Receiver<Vec<u8>>,
    pub resize: mpsc::Receiver<(u16, u16)>,
    pub exit: oneshot::Sender<Option<i64>>,
}

#[derive(Default)]
pub struct FakeKube {
    objects: Mutex<BTreeMap<(KubeKind, String, String), KubeObject>>,
    forbidden: Mutex<HashSet<KubeKind>>,
    truncated: AtomicBool,
    pub unreachable: AtomicBool,
    calls: Mutex<Vec<String>>,
    logs: Mutex<Vec<String>>,
    pub log_opts: Mutex<Vec<LogOptions>>,
    execs: Mutex<Vec<ExecHandle>>,
}

pub fn row(name: &str, ns: Option<&str>) -> KubeRow {
    KubeRow {
        name: name.into(),
        namespace: ns.map(str::to_string),
        created: 1,
        status: "Running".into(),
        health: Health::Ok,
        columns: BTreeMap::from([("ready".to_string(), "1/1".to_string())]),
    }
}

impl FakeKube {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn object(&self, kind: KubeKind, ns: &str, name: &str) {
        let ns_opt = (!kind.cluster_scoped()).then_some(ns);
        let obj = KubeObject {
            row: row(name, ns_opt),
            yaml: format!("kind: {}\nmetadata:\n  name: {name}\n", kind.kind_name()),
            conditions: vec![],
            owners: vec![],
            containers: if kind == KubeKind::Pods { vec!["main".into()] } else { vec![] },
        };
        self.objects.lock().unwrap().insert((kind, ns.into(), name.into()), obj);
    }

    pub fn set_forbidden(&self, kind: KubeKind) {
        self.forbidden.lock().unwrap().insert(kind);
    }
    pub fn set_truncated(&self) {
        self.truncated.store(true, Ordering::SeqCst);
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn set_logs(&self, lines: &[&str]) {
        *self.logs.lock().unwrap() = lines.iter().map(|s| s.to_string()).collect();
    }
    pub fn take_exec(&self) -> Option<ExecHandle> {
        self.execs.lock().unwrap().pop()
    }
    fn call(&self, c: String) {
        self.calls.lock().unwrap().push(c);
    }
    fn gate(&self, kind: KubeKind) -> Result<()> {
        if self.forbidden.lock().unwrap().contains(&kind) {
            return Err(Error::Forbidden(format!("{} is forbidden: cannot list resource \"{}\"", kind.as_str(), kind.as_str())));
        }
        Ok(())
    }
    fn find(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        self.objects
            .lock()
            .unwrap()
            .get(&(kind, ns.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{} \"{name}\" not found", kind.as_str())))
    }
}

#[async_trait]
impl KubeOps for FakeKube {
    async fn version(&self) -> Result<KubeVersion> {
        Ok(KubeVersion { git_version: "v1.32.0".into(), platform: "linux/amd64".into(), nodes: 3, nodes_ready: 3 })
    }
    async fn list(&self, kind: KubeKind, ns: Option<&str>) -> Result<KubeList> {
        self.gate(kind)?;
        self.call(format!("list {} {}", kind.as_str(), ns.unwrap_or("*")));
        let rows = self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter(|((k, n, _), _)| *k == kind && (kind.cluster_scoped() || ns.is_none_or(|ns| ns == n)))
            .map(|(_, o)| o.row.clone())
            .collect();
        Ok(KubeList { rows, truncated: self.truncated.load(Ordering::SeqCst) })
    }
    async fn get(&self, kind: KubeKind, ns: &str, name: &str) -> Result<KubeObject> {
        self.gate(kind)?;
        self.find(kind, ns, name)
    }
    async fn events(&self, kind: KubeKind, ns: &str, name: &str) -> Result<Vec<KubeEvent>> {
        self.find(kind, ns, name)?;
        Ok(vec![KubeEvent { type_: "Normal".into(), reason: "Started".into(), message: "started".into(), count: 1, last_seen: Some(1), source: Some("kubelet".into()) }])
    }
    async fn delete(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        self.find(kind, ns, name)?;
        self.objects.lock().unwrap().remove(&(kind, ns.into(), name.into()));
        self.call(format!("delete {}/{ns}/{name}", kind.as_str()));
        Ok(())
    }
    async fn scale(&self, kind: KubeKind, ns: &str, name: &str, replicas: u32) -> Result<()> {
        self.find(kind, ns, name)?;
        self.call(format!("scale {}/{ns}/{name} {replicas}", kind.as_str()));
        Ok(())
    }
    async fn restart(&self, kind: KubeKind, ns: &str, name: &str) -> Result<()> {
        self.find(kind, ns, name)?;
        self.call(format!("restart {}/{ns}/{name}", kind.as_str()));
        Ok(())
    }
    fn logs(&self, ns: &str, pod: &str, opts: &LogOptions) -> BoxStream<LogLine> {
        self.log_opts.lock().unwrap().push(opts.clone());
        self.call(format!("logs {ns}/{pod}"));
        let lines: Vec<Result<LogLine>> = self
            .logs
            .lock()
            .unwrap()
            .iter()
            .map(|t| Ok(LogLine { stream: LogStream::Stdout, text: t.clone() }))
            .collect();
        Box::pin(futures_util::stream::iter(lines))
    }
    async fn exec_start(&self, ns: &str, pod: &str, opts: &ExecOptions) -> Result<KubeExec> {
        self.find(KubeKind::Pods, ns, pod)?;
        let (out_tx, out_rx) = mpsc::channel(16);
        let (in_tx, in_rx) = mpsc::channel(16);
        let (rs_tx, rs_rx) = mpsc::channel(16);
        let (exit_tx, exit_rx) = oneshot::channel();
        self.execs.lock().unwrap().push(ExecHandle {
            target: format!("{ns}/{pod}"),
            opts: opts.clone(),
            output: out_tx,
            input: in_rx,
            resize: rs_rx,
            exit: exit_tx,
        });
        let output = futures_util::stream::unfold(out_rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) });
        Ok(KubeExec { output: Box::pin(output), input: in_tx, resize: rs_tx, exit_code: exit_rx })
    }
}

pub struct FakeKubeEnv {
    pub id: i64,
    pub name: String,
    pub kind: EnvKind,
    pub kube: Arc<FakeKube>,
}

#[async_trait]
impl Environment for FakeKubeEnv {
    fn id(&self) -> i64 { self.id }
    fn name(&self) -> &str { &self.name }
    fn kind(&self) -> EnvKind { self.kind }
    async fn status(&self) -> EnvStatus {
        if self.kube.unreachable.load(Ordering::SeqCst) {
            return EnvStatus::Down { reason: "cannot reach cluster: test".into() };
        }
        EnvStatus::Up { version: EnvVersion::Kube(self.kube.version().await.unwrap()) }
    }
    fn docker(&self) -> Option<Arc<dyn DockerOps>> { None }
    fn kube(&self) -> Option<Arc<dyn KubeOps>> {
        (!self.kube.unreachable.load(Ordering::SeqCst)).then(|| self.kube.clone() as Arc<dyn KubeOps>)
    }
}
```

In `fake_docker.rs`:
- add `pub kube: Arc<super::fake_kube::FakeKube>` to `FakeDocker`, initialised with `FakeKube::new()`;
- `FakeFactory::build` returns `Arc::new(FakeKubeEnv { id: spec.id, name: spec.name.clone(), kind: spec.kind, kube: self.0.kube.clone() })` when `spec.kind.is_kube()`, and keeps recording the spec.

In `common/mod.rs`:
- add `pub mod fake_kube;`;
- move the `KUBECONFIG` constant from `kube_envs.rs` into this module as `pub const KUBECONFIG`, and import it there;
- add:

```rust
    /// A kube-config environment backed by `self.fake.kube`.
    pub async fn add_kube_env(&self, admin: &Session, name: &str) {
        let r = self
            .send(req("POST", "/api/v1/envs", Some(serde_json::json!({
                "name": name, "kind": "kube-config", "kubeconfig": KUBECONFIG, "skip_check": true})), Some(admin)))
            .await;
        assert_eq!(r.status, 201, "{:?}", r.json);
    }
```

`crates/fleetops-api/tests/kube_routes.rs`:

```rust
mod common;

use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::json;

async fn kube_app() -> (TestApp, Session) {
    let t = app().await;
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    let k = &t.fake.kube;
    k.object(KubeKind::Deployments, "apps", "web");
    k.object(KubeKind::Deployments, "other", "api");
    k.object(KubeKind::Pods, "apps", "web-1");
    k.object(KubeKind::Nodes, "-", "n1");
    (t, admin)
}

#[tokio::test]
async fn a_list_is_scoped_to_the_namespace_asked_for() {
    let (t, admin) = kube_app().await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/deployments?namespace=apps", None, Some(&admin))).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["rows"].as_array().unwrap().len(), 1);
    assert_eq!(r.json["rows"][0]["name"], "web");
    assert_eq!(r.json["truncated"], false);
    let all = t.send(req("GET", "/api/v1/envs/k/k8s/deployments", None, Some(&admin))).await;
    assert_eq!(all.json["rows"].as_array().unwrap().len(), 2);
    let nodes = t.send(req("GET", "/api/v1/envs/k/k8s/nodes?namespace=apps", None, Some(&admin))).await;
    assert_eq!(nodes.json["rows"][0]["name"], "n1");
}

#[tokio::test]
async fn an_unknown_kind_is_404() {
    let (t, admin) = kube_app().await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/replicasets", None, Some(&admin))).await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn kube_and_docker_routes_refuse_the_other_kind() {
    let (t, admin) = kube_app().await;
    t.add_env(&admin, "local").await;
    let r = t.send(req("GET", "/api/v1/envs/local/k8s/pods", None, Some(&admin))).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json["message"], "conflict: wrong environment kind: 'local' is docker-local");
    let r = t.send(req("GET", "/api/v1/envs/k/containers", None, Some(&admin))).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json["message"], "conflict: wrong environment kind: 'k' is kube-config");
}

#[tokio::test]
async fn get_and_events_return_the_object() {
    let (t, admin) = kube_app().await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/pods/apps/web-1", None, Some(&admin))).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["row"]["name"], "web-1");
    assert_eq!(r.json["containers"], json!(["main"]));
    let e = t.send(req("GET", "/api/v1/envs/k/k8s/pods/apps/web-1/events", None, Some(&admin))).await;
    assert_eq!(e.json[0]["reason"], "Started");
    let n = t.send(req("GET", "/api/v1/envs/k/k8s/nodes/-/n1", None, Some(&admin))).await;
    assert_eq!(n.status, 200);
    let missing = t.send(req("GET", "/api/v1/envs/k/k8s/pods/apps/nope", None, Some(&admin))).await;
    assert_eq!(missing.status, 404);
}

#[tokio::test]
async fn scale_restart_and_delete_act_and_are_audited() {
    let (t, admin) = kube_app().await;
    let s = t.send(req("POST", "/api/v1/envs/k/k8s/deployments/apps/web/scale", Some(json!({"replicas": 3})), Some(&admin))).await;
    assert_eq!(s.status, 204, "{:?}", s.json);
    let r = t.send(req("POST", "/api/v1/envs/k/k8s/deployments/apps/web/restart", None, Some(&admin))).await;
    assert_eq!(r.status, 204);
    let d = t.send(req("DELETE", "/api/v1/envs/k/k8s/pods/apps/web-1", None, Some(&admin))).await;
    assert_eq!(d.status, 204);
    let calls = t.fake.kube.calls();
    assert!(calls.contains(&"scale deployments/apps/web 3".into()), "{calls:?}");
    assert!(calls.contains(&"restart deployments/apps/web".into()));
    assert!(calls.contains(&"delete pods/apps/web-1".into()));
    let audit = t.send(req("GET", "/api/v1/audit", None, Some(&admin))).await;
    let text = audit.json.to_string();
    for (action, target) in [("k8s.scale", "deployments/apps/web"), ("k8s.restart", "deployments/apps/web"), ("k8s.delete", "pods/apps/web-1")] {
        assert!(text.contains(action) && text.contains(target), "{action} {target}: {text}");
    }
}

#[tokio::test]
async fn kinds_that_cannot_scale_or_restart_are_refused_before_the_cluster() {
    let (t, admin) = kube_app().await;
    let s = t.send(req("POST", "/api/v1/envs/k/k8s/pods/apps/web-1/scale", Some(json!({"replicas": 2})), Some(&admin))).await;
    assert_eq!(s.status, 400);
    let r = t.send(req("POST", "/api/v1/envs/k/k8s/pods/apps/web-1/restart", None, Some(&admin))).await;
    assert_eq!(r.status, 400);
    assert!(t.fake.kube.calls().iter().all(|c| !c.starts_with("scale") && !c.starts_with("restart")));
}

#[tokio::test]
async fn a_cluster_403_passes_through_with_its_reason() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_forbidden(KubeKind::Secrets);
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/secrets", None, Some(&admin))).await;
    assert_eq!(r.status, 403);
    assert!(r.json["message"].as_str().unwrap().contains("cannot list resource"));
}

#[tokio::test]
async fn an_unreachable_cluster_is_503_and_truncation_is_reported() {
    let (t, admin) = kube_app().await;
    t.fake.kube.set_truncated();
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/pods", None, Some(&admin))).await;
    assert_eq!(r.json["truncated"], true);
    t.fake.kube.unreachable.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/pods", None, Some(&admin))).await;
    assert_eq!(r.status, 503);
}

#[tokio::test]
async fn ordinary_users_can_read_and_act() {
    let (t, admin) = kube_app().await;
    let user = t.create_user(&admin, "bob", "user").await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/deployments", None, Some(&user))).await;
    assert_eq!(r.status, 200);
    let s = t.send(req("POST", "/api/v1/envs/k/k8s/deployments/apps/web/restart", None, Some(&user))).await;
    assert_eq!(s.status, 204);
}
```

Use the audit list route that `tests/audit_log.rs` uses (`/api/v1/audit`, or `/api/v1/audit-log` if that is the path), and the `add_env` helper's real signature.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test kube_routes 2>&1 | tail -5`
Expected: failures with 404s on the `/k8s/` paths.

- [ ] **Step 3: Implement**

`crates/fleetops-api/src/routes/kube.rs`:

```rust
//! `/envs/{env}/k8s/…`: Kubernetes objects in one environment (spec §5).

use crate::{
    auth::extract::CurrentUser,
    authz::{authorize, Action},
    extract::{ApiJson, ApiPath},
    ApiResult, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use fleetops_core::env::{EnvKind, Environment};
use fleetops_core::kube::{KubeEvent, KubeKind, KubeList, KubeObject, KubeOps};
use fleetops_core::{now_unix, Error, Result};
use fleetops_store::NewAudit;
use serde::Deserialize;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

pub(crate) fn wrong_kind(name: &str, kind: EnvKind) -> Error {
    Error::Conflict(format!("wrong environment kind: '{name}' is {}", kind.as_str()))
}

/// Resolves the environment, authorises, and returns its Kubernetes ops.
pub(crate) fn kube_for(
    s: &AppState,
    cu: &CurrentUser,
    name: &str,
    action: Action,
) -> Result<(Arc<dyn Environment>, Arc<dyn KubeOps>)> {
    let env = s.envs.get(name)?;
    authorize(&cu.user, Some(env.id()), action)?;
    if !env.kind().is_kube() {
        return Err(wrong_kind(name, env.kind()));
    }
    let ops = env
        .kube()
        .ok_or_else(|| Error::EnvUnavailable(format!("environment '{name}' is unreachable")))?;
    Ok((env, ops))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct KindPath {
    /// Environment name.
    pub env: String,
    /// Plural lowercase kind, e.g. `deployments`.
    pub kind: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ObjectPath {
    pub env: String,
    pub kind: String,
    /// Namespace; `-` for cluster-scoped kinds.
    pub ns: String,
    pub name: String,
}

#[derive(Deserialize, IntoParams)]
pub struct ListQuery {
    /// Omit for all namespaces.
    pub namespace: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ScaleBody {
    pub replicas: u32,
}

fn kind(s: &str) -> Result<KubeKind> {
    s.parse()
}

async fn audit(s: &AppState, cu: &CurrentUser, env_id: i64, action: &str, target: &str, outcome: &Result<()>) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action,
        target_type: Some("k8s"),
        target_id: Some(target),
        result: if outcome.is_ok() { "success" } else { "failure" },
        detail: outcome.as_ref().err().map(|e| serde_json::json!({ "error": e.to_string() })),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}", operation_id = "listKube", tag = "kubernetes",
    summary = "List objects of one kind", params(KindPath, ListQuery),
    responses((status = 200, description = "Rows", body = KubeList)))]
pub async fn list(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<KindPath>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<KubeList>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.list(k, q.namespace.as_deref().filter(|n| !n.is_empty())).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "getKube", tag = "kubernetes",
    summary = "One object: summary, masked YAML, conditions, owners", params(ObjectPath),
    responses((status = 200, description = "The object", body = KubeObject)))]
pub async fn get(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>) -> ApiResult<Json<KubeObject>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.get(k, &p.ns, &p.name).await?))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/events", operation_id = "kubeEvents", tag = "kubernetes",
    summary = "An object's events, newest first", params(ObjectPath),
    responses((status = 200, description = "Events", body = [KubeEvent])))]
pub async fn events(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>) -> ApiResult<Json<Vec<KubeEvent>>> {
    let k = kind(&p.kind)?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.events(k, &p.ns, &p.name).await?))
}

fn target(k: KubeKind, p: &ObjectPath) -> String {
    format!("{}/{}/{}", k.as_str(), p.ns, p.name)
}

#[utoipa::path(delete, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "deleteKube", tag = "kubernetes",
    summary = "Delete an object (background propagation)", params(ObjectPath),
    responses((status = 204, description = "Deleted")))]
pub async fn delete(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.delete(k, &p.ns, &p.name).await;
    audit(&s, &cu, env.id(), "k8s.delete", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/scale", operation_id = "scaleKube", tag = "kubernetes",
    summary = "Set a Deployment's or StatefulSet's replicas", params(ObjectPath), request_body = ScaleBody,
    responses((status = 204, description = "Scaled")))]
pub async fn scale(
    State(s): State<AppState>,
    cu: CurrentUser,
    ApiPath(p): ApiPath<ObjectPath>,
    ApiJson(body): ApiJson<ScaleBody>,
) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    if !k.scalable() {
        return Err(Error::Invalid(format!("{} cannot be scaled", k.as_str())).into());
    }
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.scale(k, &p.ns, &p.name, body.replicas).await;
    audit(&s, &cu, env.id(), "k8s.scale", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/restart", operation_id = "restartKube", tag = "kubernetes",
    summary = "Rollout restart a Deployment, StatefulSet or DaemonSet", params(ObjectPath),
    responses((status = 204, description = "Restarted")))]
pub async fn restart(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>) -> ApiResult<StatusCode> {
    let k = kind(&p.kind)?;
    if !k.restartable() {
        return Err(Error::Invalid(format!("{} cannot be restarted", k.as_str())).into());
    }
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops.restart(k, &p.ns, &p.name).await;
    audit(&s, &cu, env.id(), "k8s.restart", &target(k, &p), &r).await;
    r?;
    Ok(StatusCode::NO_CONTENT)
}
```

Registration in `routes/mod.rs` `env_routes()`, following its existing grouping by path:

```rust
        .routes(routes!(kube::list))
        .routes(routes!(kube::get, kube::delete))
        .routes(routes!(kube::events))
        .routes(routes!(kube::scale))
        .routes(routes!(kube::restart))
```

Add `mod kube;` (or `pub(crate) mod kube;`, because Task 13 imports `kube_for`). Add a `"kubernetes"` tag to `ApiDoc` in `openapi.rs` next to the existing tags.

Use `ApiPath` the way the existing handlers use it. If `ApiPath` doesn't support structs, use axum's `Path<ObjectPath>`. If `ApiJson` rejects an absent body on `restart`, use no body extractor at all, as written above.

In `crates/fleetops-api/src/routes/docker/mod.rs` `ctx`, add this right after `s.envs.get`:

```rust
    if env.kind().is_kube() {
        return Err(crate::routes::kube::wrong_kind(env_name, env.kind()).into());
    }
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-api --test kube_routes 2>&1 | tail -3`
Expected: 9 passed.

- [ ] **Step 5: Contract checks, document, suite, commit**

Add a contract check to `a_list_is_scoped_to_the_namespace_asked_for`, following an existing route test in `tests/` that calls `common::contract::check`. Then set `OPERATIONS` to 68.

Run: `make openapi && make lint && make test 2>&1 | tail -3 && make test-web 2>&1 | tail -3`
Expected: green.

```bash
git add -A crates web/src/api/gen
git commit -m "feat(api): /envs/{env}/k8s list, get, events, delete, scale and restart

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 13: Pod logs and exec over the shared WebSocket

**Files:**
- Modify: `crates/fleetops-api/src/ws/topics.rs` (`logs`/`exec` dispatch on kind; `docker_for` returns 409 for kube; shared log pump), `crates/fleetops-api/src/ws/conn.rs` (`ExecCtl.resize`)
- Test: `crates/fleetops-api/tests/ws_kube.rs`

**Interfaces:**
- Consumes: `kube_for` and `wrong_kind` (Task 12); `KubeOps::logs` and `exec_start`, `KubeExec` (Task 2); `FakeKube::{set_logs, take_exec, log_opts}` (Task 12).
- Produces:
  - WS topic `logs` with `id = "<ns>/<pod>"` on kube envs (KubeRead), plus `opts.container`;
  - WS topic `exec` with the same id (KubeWrite, audited as `k8s.exec` with target `pods/<ns>/<pod>`);
  - `ExecCtl { input, resize: Resize }` with `enum Resize { Docker { ops, exec_id }, Kube(mpsc::Sender<(u16, u16)>) }`.

- [ ] **Step 1: Write the failing tests**

Model these on `crates/fleetops-api/tests/ws_logs.rs` and `ws_exec.rs`: the same `serve()` / `ws_connect()` / `Ws` helpers, and the same login cookie. `crates/fleetops-api/tests/ws_kube.rs`:

```rust
mod common;

use common::*;
use fleetops_core::kube::KubeKind;
use serde_json::json;
use std::time::Duration;

async fn kube_ws() -> (TestApp, Ws) {
    let t = ws_app().await; // the helper ws_logs.rs uses: an app with ws_config(), served
    let admin = t.setup_admin().await;
    t.add_kube_env(&admin, "k").await;
    t.fake.kube.object(KubeKind::Pods, "apps", "web-1");
    let ws = Ws::open(&t, &admin).await;
    (t, ws)
}

#[tokio::test]
async fn kube_logs_stream_over_the_shared_socket() {
    let (t, mut ws) = kube_ws().await;
    t.fake.kube.set_logs(&["one", "two"]);
    ws.send(json!({"op": "sub", "sub": 1, "topic": "logs", "env": "k", "id": "apps/web-1",
        "opts": {"follow": false, "container": "main"}})).await;
    let data = ws.until(|m| m["type"] == "data").await;
    let texts: Vec<_> = data["data"].as_array().unwrap().iter().map(|l| l["text"].clone()).collect();
    assert_eq!(texts, [json!("one"), json!("two")]);
    ws.until(|m| m["type"] == "end").await;
    assert_eq!(t.fake.kube.log_opts.lock().unwrap()[0].container.as_deref(), Some("main"));
}

#[tokio::test]
async fn a_kube_pod_id_must_be_namespace_slash_pod() {
    let (_t, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "logs", "env": "k", "id": "web-1"})).await;
    let e = ws.until(|m| m["type"] == "error").await;
    assert_eq!(e["code"], "invalid");
}

#[tokio::test]
async fn kube_exec_carries_input_resize_output_and_the_exit_code() {
    let (t, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 7, "topic": "exec", "env": "k", "id": "apps/web-1",
        "opts": {"cmd": ["sh"], "cols": 80, "rows": 24}})).await;
    let mut h = None;
    for _ in 0..100 {
        h = t.fake.kube.take_exec();
        if h.is_some() { break }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut h = h.expect("exec started");
    assert_eq!(h.target, "apps/web-1");
    ws.send_binary(7, b"ls\n").await;
    assert_eq!(h.input.recv().await.unwrap(), b"ls\n");
    ws.send(json!({"op": "resize", "sub": 7, "cols": 100, "rows": 40})).await;
    assert_eq!(h.resize.recv().await.unwrap(), (100, 40));
    h.output.send(Ok(b"bin etc".to_vec())).await.unwrap();
    let (sub, bytes) = ws.binary().await;
    assert_eq!((sub, bytes.as_slice()), (7, &b"bin etc"[..]));
    drop(h.output);
    h.exit.send(Some(0)).unwrap();
    let end = ws.until(|m| m["type"] == "end").await;
    assert_eq!((end["reason"].clone(), end["exit_code"].clone()), (json!("exited"), json!(0)));
    let audit = t.store.list_audit(Default::default()).await.unwrap();
    assert!(audit.iter().any(|a| a.action == "k8s.exec" && a.target_id.as_deref() == Some("pods/apps/web-1")));
}

#[tokio::test]
async fn a_kube_exec_whose_pod_disappears_ends_promptly() {
    let (t, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 3, "topic": "exec", "env": "k", "id": "apps/web-1"})).await;
    let mut h = None;
    for _ in 0..100 {
        h = t.fake.kube.take_exec();
        if h.is_some() { break }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(h.expect("exec started")); // output and exit both dropped: the pod is gone
    let end = tokio::time::timeout(Duration::from_secs(2), ws.until(|m| m["type"] == "end")).await.expect("prompt end");
    assert_eq!(end["exit_code"], serde_json::Value::Null);
}

#[tokio::test]
async fn docker_topics_on_a_kube_environment_are_refused() {
    let (_t, mut ws) = kube_ws().await;
    ws.send(json!({"op": "sub", "sub": 1, "topic": "events", "env": "k"})).await;
    let e = ws.until(|m| m["type"] == "error").await;
    assert_eq!(e["code"], "conflict");
}
```

Adapt these helper names to what `tests/common/mod.rs` really provides:
- `ws_app`, `Ws::open`, `ws.until`, `ws.send_binary`, `ws.binary`;
- the audit list call: use whatever `ws_exec.rs` uses to check the `container.exec` audit row.

If a helper is missing (for example `binary()` returning `(u32, Vec<u8>)`), add it to `common/mod.rs` next to the others.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p fleetops-api --test ws_kube 2>&1 | tail -5`
Expected: failures. Logs and exec on `k` get `env_unavailable` (because `docker()` is `None`), and events does not get `conflict`.

- [ ] **Step 3: Implement**

In `crates/fleetops-api/src/ws/conn.rs`:

```rust
/// How a terminal resize reaches the process.
pub(super) enum Resize {
    Docker { ops: Arc<dyn DockerOps>, exec_id: String },
    Kube(mpsc::Sender<(u16, u16)>),
}

pub(super) struct ExecCtl {
    pub input: mpsc::Sender<Vec<u8>>,
    pub resize: Resize,
}
```

and in the `ClientMsg::Resize` arm:

```rust
        ClientMsg::Resize { sub, cols, rows } => {
            enum Do {
                Docker(Arc<dyn DockerOps>, String),
                Kube(mpsc::Sender<(u16, u16)>),
            }
            let target = shared.subs.lock().unwrap().get(&sub).and_then(|s| {
                s.exec.as_ref().map(|c| match &c.resize {
                    Resize::Docker { ops, exec_id } => Do::Docker(ops.clone(), exec_id.clone()),
                    Resize::Kube(tx) => Do::Kube(tx.clone()),
                })
            });
            match target {
                Some(Do::Docker(ops, exec_id)) => {
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ops.exec_resize(&exec_id, cols, rows).await {
                            shared.fail(Some(sub), &e);
                        }
                    });
                }
                // A full channel means resizes are arriving faster than the
                // process takes them; the next one supersedes this one.
                Some(Do::Kube(tx)) => {
                    let _ = tx.try_send((cols, rows));
                }
                None => shared.fail(Some(sub), &Error::NotFound(format!("no exec subscription {sub}"))),
            }
        }
```

In `crates/fleetops-api/src/ws/topics.rs`:

1. `docker_for` returns `Err(crate::routes::kube::wrong_kind(name, env.kind()))` when `env.kind().is_kube()`, checked before `env.docker()`.
2. Add:
   ```rust
   /// `<namespace>/<pod>`, the id of a pod on the logs and exec topics.
   fn pod_id(id: &Option<String>) -> Result<(String, String)> {
       let id = required(id, "id")?;
       match id.split_once('/') {
           Some((ns, pod)) if !ns.is_empty() && !pod.is_empty() && !pod.contains('/') => Ok((ns.into(), pod.into())),
           _ => Err(Error::Invalid("a Kubernetes pod id is <namespace>/<pod>".into())),
       }
   }

   fn is_kube_env(s: &AppState, env: &Option<String>) -> bool {
       env.as_deref().and_then(|n| s.envs.get(n).ok()).is_some_and(|e| e.kind().is_kube())
   }
   ```
3. Move the body of the `tokio::spawn` in `logs` (the batching loop) into `fn pump_logs(q: Arc<SubQueue>, sub: u32, mut lines: BoxStream<LogLine>) -> tokio::task::JoinHandle<()>`, and use it in both branches:
   ```rust
   pub(super) async fn logs(c: &Ctx<'_>, env: Option<String>, id: Option<String>, opts_v: Value) -> Result<Sub> {
       let (s, cu, sub) = (c.s, c.cu, c.sub);
       let o: LogOptions = opts(opts_v)?;
       let lines = if is_kube_env(s, &env) {
           let (_, ops) = crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeRead)?;
           let (ns, pod) = pod_id(&id)?;
           ops.logs(&ns, &pod, &o)
       } else {
           let (_, ops) = docker_for(s, cu, &env, Action::DockerRead)?;
           ops.logs(required(&id, "id")?, &o)
       };
       let q = c.queue(Policy::Gap);
       let task = pump_logs(q.clone(), sub, lines);
       Ok(Sub { queue: q, task: task.abort_handle(), exec: None })
   }
   ```
4. In `exec`, at the top:
   ```rust
   if is_kube_env(c.s, &env) {
       return kube_exec(c, env, id, opts_v).await;
   }
   ```
   and build the Docker `ExecCtl` as `ExecCtl { input: session.input, resize: Resize::Docker { ops, exec_id: session.exec_id } }`.
5. Add:

```rust
/// A TTY in a pod's container. Ends with `exited` when the output ends;
/// the exit code is whatever the process reported within a second.
async fn kube_exec(c: &Ctx<'_>, env: Option<String>, id: Option<String>, opts_v: Value) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, ops) = crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeWrite)?;
    let (ns, pod) = pod_id(&id)?;
    let o: ExecOptions = opts(opts_v)?;
    let session = ops.exec_start(&ns, &pod, &o).await;
    let failure = session.as_ref().err().map(|e| e.to_string());
    audit_kube_exec(s, cu, env.id(), &format!("pods/{ns}/{pod}"), &o, failure).await;
    let session = session?;
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let mut output = session.output;
    let exit = session.exit_code;
    let task = tokio::spawn(async move {
        while let Some(chunk) = output.next().await {
            let Ok(bytes) = chunk else { break };
            if !qq.push_wait(Out::Binary(encode_binary(sub, &bytes))).await {
                return;
            }
        }
        let code = tokio::time::timeout(Duration::from_secs(1), exit).await.ok().and_then(|r| r.ok()).flatten();
        qq.finish(protocol::end(sub, "exited", json!({ "exit_code": code })));
    });
    Ok(Sub {
        queue: q,
        task: task.abort_handle(),
        exec: Some(ExecCtl { input: session.input, resize: Resize::Kube(session.resize) }),
    })
}

async fn audit_kube_exec(s: &AppState, cu: &CurrentUser, env_id: i64, target: &str, o: &ExecOptions, failure: Option<String>) {
    let (result, mut detail) = match failure {
        None => ("success", json!({})),
        Some(e) => ("failure", json!({ "error": e })),
    };
    detail["cmd"] = json!(o.command());
    detail["container"] = json!(o.container);
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action: "k8s.exec",
        target_type: Some("k8s"),
        target_id: Some(target),
        result,
        detail: Some(detail),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}
```

Import `super::conn::Resize`, `fleetops_core::streams::{BoxStream, LogLine}` and `SubQueue`.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p fleetops-api --test ws_kube 2>&1 | tail -3 && cargo test -p fleetops-api --test ws_exec --test ws_logs 2>&1 | tail -3`
Expected: 5 passed; the existing Docker WS tests still pass.

- [ ] **Step 5: Full suite, commit**

Run: `make lint && make test 2>&1 | tail -3`
Expected: green.

```bash
git add crates/fleetops-api
git commit -m "feat(api): pod logs and exec over the shared websocket topics

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 14: Web, part 1: kind-aware shell, routes, namespace picker, lists and overview

**Files:**
- Modify: `web/src/lib/env.ts` (`isKube`), `web/src/components/nav.ts` (`KUBE_NAV`), `web/src/components/Shell.tsx` (rail, go-keys, breadcrumb, EnvLive, switcher icon), `web/src/router.tsx` (k8s routes, env home, kind redirect), `web/src/api/queries.ts` (kube keys and queries, status polling)
- Create: `web/src/pages/EnvHome.tsx`, `web/src/pages/kube/kinds.ts`, `web/src/pages/kube/NamespacePicker.tsx`, `web/src/pages/kube/KubeListPage.tsx`, `web/src/pages/kube/KubeOverviewPage.tsx`, `web/src/pages/kube/KubeHealth.tsx`
- Modify: `web/src/test/app.tsx` (a kube env and its routes)
- Test: `web/src/pages/kube/kube.test.tsx`

**Interfaces:**
- Consumes the generated SDK from Tasks 11–12:
  - `listKube({ path: { env, kind }, query: { namespace } })` → `KubeList`;
  - the types `KubeRow`, `KubeKind`, `EnvView.kube` and `EnvVersion` (`kind: "kube"`).
- Produces:
  - `isKube(kind: string): boolean`;
  - `KUBE_KINDS`, `WORKLOADS`, `KIND_LABEL` and `KIND_COLUMNS` in `pages/kube/kinds.ts`;
  - `keys.kube(env, kind, ns?)` and `keys.kubeObject(env, kind, ns, name)`;
  - `kubeListQuery(env, kind, ns?)` (polls every 5 s);
  - `envStatusQuery(env, kube?: boolean)` (polls every 15 s when `kube`);
  - `useNamespace(env): [string | undefined, (ns?: string) => void]`;
  - the routes `/envs/$env/k8s/$kind` and `/envs/$env/k8s/$kind/$ns/$name` (the latter's page comes in Task 15);
  - `KubeHealth({ health })`, the status dot for a row.

- [ ] **Step 1: Write the failing tests**

In `web/src/test/app.tsx`:
- add a second environment to `ENVS`: `{ id: 2, name: "k", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, agent: null, kube: { server: "https://k:6443", context: "dev", user_kind: "token" } }`;
- export a `kubeRoutes` map with:
  - `"GET /api/v1/envs/k/status"` → `{ state: "up", version: { kind: "kube", git_version: "v1.32.0", platform: "linux/amd64", nodes: 3, nodes_ready: 3 } }`;
  - `"GET /api/v1/envs/k/k8s/namespaces"` → `{ rows: [row("apps"), row("default")], truncated: false }`;
  - `"GET /api/v1/envs/k/k8s/deployments"` → `{ rows: [row("web", "apps", { ready: "2/3", up_to_date: "3", available: "2" }, "warn")], truncated: false }`;
  - `"GET /api/v1/envs/k/k8s/pods"` → `{ rows: [row("web-1", "apps", { ready: "1/1", restarts: "0", node: "n1", ip: "" })], truncated: false }`.

  `row(name, ns?, columns = {}, health = "ok")` builds a `KubeRow`.

`web/src/pages/kube/kube.test.tsx`:

```tsx
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

describe("Kubernetes environments", () => {
  it("shows the Kubernetes pages in the sidebar, not Docker's", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    for (const label of ["Overview", "Workloads", "Pods", "Services", "Ingresses", "ConfigMaps", "Secrets", "Namespaces", "Nodes"]) {
      expect(within(nav).getByRole("link", { name: new RegExp(label) })).toBeInTheDocument();
    }
    expect(within(nav).queryByRole("link", { name: /Containers/ })).not.toBeInTheDocument();
    expect(await screen.findByText("v1.32.0")).toBeInTheDocument();
    expect(screen.getByText("3 of 3 nodes ready")).toBeInTheDocument();
  });

  it("lists a kind with its own columns", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    for (const h of ["Name", "Namespace", "Ready", "Up-to-date", "Available", "Age"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(h) })).toBeInTheDocument();
    }
    expect(within(table).getByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/k/k8s/deployments/apps/web");
    expect(within(table).getByText("2/3")).toBeInTheDocument();
    // Workload kinds share tabs.
    expect(screen.getByRole("link", { name: "StatefulSets" })).toHaveAttribute("href", "/envs/k/k8s/statefulsets");
  });

  it("asks for one namespace once it is picked, and remembers it", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods": { json: { rows: [], truncated: false } } });
    renderApp("/envs/k/k8s/pods");
    await userEvent.selectOptions(await screen.findByRole("combobox", { name: "Namespace" }), "apps");
    await screen.findByRole("table", { name: "Pods" });
    expect(calls.some((c) => c.path === "/api/v1/envs/k/k8s/pods" && c.query?.namespace === "apps")).toBe(true);
    expect(localStorage.getItem("fleetops.ns.k")).toBe("apps");
  });

  it("says when the cluster forbids a list", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/secrets": { status: 403, json: { code: "forbidden", message: "secrets is forbidden", request_id: null } },
    });
    renderApp("/envs/k/k8s/secrets");
    expect(await screen.findByText("No permission to list Secrets in all namespaces")).toBeInTheDocument();
  });

  it("says when a list is truncated", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods": { json: { rows: [], truncated: true } } });
    renderApp("/envs/k/k8s/pods");
    expect(await screen.findByText(/Showing the first 5,000/)).toBeInTheDocument();
  });

  it("docker_routes_under_a_kube_env_redirect_to_its_overview", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes });
    const { history } = renderApp("/envs/k/containers");
    expect(await screen.findByText("v1.32.0")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/envs/k");
    expect(calls.some((c) => c.path === "/api/v1/envs/k/containers")).toBe(false);
  });

  it("does not subscribe to Docker events for a kube environment", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k");
    await screen.findByText("v1.32.0");
    const { FakeWebSocket } = await import("@/test/fake-ws");
    const subs = FakeWebSocket.latest()?.sent.filter((m) => m.includes('"topic":"events"') && m.includes('"env":"k"')) ?? [];
    expect(subs).toEqual([]);
  });
});
```

Match `mockFetch`'s call record. If it doesn't parse the query string into `c.query`, check `c.path`/URL the way existing tests do, and extend `src/test/fetch.ts` to record `query` if needed. Match `FakeWebSocket`'s real API too (`sent`, `latest()`).

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm -C web exec vitest run src/pages/kube 2>&1 | tail -15`
Expected: failures. There are no Kubernetes nav links, the routes are missing, and a kube env gets the Docker dashboard.

- [ ] **Step 3: Implement**

`web/src/lib/env.ts`, append:

```ts
/** Kubernetes environment kinds start with "kube-". */
export function isKube(kind: string | undefined): boolean {
  return !!kind && kind.startsWith("kube-");
}
```

`web/src/pages/kube/kinds.ts`:

```ts
import type { KubeKind } from "@/api/gen";

export const KUBE_KINDS: readonly KubeKind[] = [
  "namespaces", "nodes", "deployments", "statefulsets", "daemonsets", "jobs", "cronjobs",
  "pods", "services", "ingresses", "configmaps", "secrets",
];
export const WORKLOADS: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets", "jobs", "cronjobs"];
export const CLUSTER_SCOPED: readonly KubeKind[] = ["namespaces", "nodes"];
export const SCALABLE: readonly KubeKind[] = ["deployments", "statefulsets"];
export const RESTARTABLE: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets"];

export const KIND_LABEL: Record<KubeKind, string> = {
  namespaces: "Namespaces", nodes: "Nodes", deployments: "Deployments", statefulsets: "StatefulSets",
  daemonsets: "DaemonSets", jobs: "Jobs", cronjobs: "CronJobs", pods: "Pods", services: "Services",
  ingresses: "Ingresses", configmaps: "ConfigMaps", secrets: "Secrets",
};

/** Kind-specific columns: [key in KubeRow.columns, header]. */
export const KIND_COLUMNS: Record<KubeKind, [string, string][]> = {
  namespaces: [],
  nodes: [["roles", "Roles"], ["version", "Version"], ["internal_ip", "Internal IP"]],
  deployments: [["ready", "Ready"], ["up_to_date", "Up-to-date"], ["available", "Available"]],
  statefulsets: [["ready", "Ready"]],
  daemonsets: [["desired", "Desired"], ["current", "Current"], ["ready", "Ready"], ["available", "Available"]],
  jobs: [["completions", "Completions"]],
  cronjobs: [["schedule", "Schedule"], ["active", "Active"], ["last_schedule", "Last schedule"]],
  pods: [["ready", "Ready"], ["restarts", "Restarts"], ["node", "Node"], ["ip", "IP"]],
  services: [["cluster_ip", "Cluster IP"], ["external_ip", "External IP"], ["ports", "Ports"]],
  ingresses: [["class", "Class"], ["hosts", "Hosts"], ["address", "Address"]],
  configmaps: [["keys", "Keys"]],
  secrets: [["keys", "Keys"]],
};

/** What the status column shows, by kind. */
export const STATUS_HEADER: Partial<Record<KubeKind, string>> = { services: "Type", secrets: "Type", configmaps: "Data" };

export const asKind = (s: string): KubeKind | undefined => KUBE_KINDS.find((k) => k === s);
```

`web/src/components/nav.ts`, append (add `Box`, `Boxes`, `Globe`, `FileCog`, `KeyRound`, `FolderTree` and `Cpu` to the lucide import):

```ts
/** A Kubernetes environment's pages, in sidebar order. `kind` items link to a list. */
export const KUBE_NAV = [
  { to: "/envs/$env", label: "Overview", icon: Gauge, key: "d" },
  { to: "/envs/$env/k8s/$kind", kind: "deployments", label: "Workloads", icon: Boxes, key: "w" },
  { to: "/envs/$env/k8s/$kind", kind: "pods", label: "Pods", icon: Box, key: "o" },
  { to: "/envs/$env/k8s/$kind", kind: "services", label: "Services", icon: Waypoints, key: "s" },
  { to: "/envs/$env/k8s/$kind", kind: "ingresses", label: "Ingresses", icon: Globe, key: "i" },
  { to: "/envs/$env/k8s/$kind", kind: "configmaps", label: "ConfigMaps", icon: FileCog, key: "m" },
  { to: "/envs/$env/k8s/$kind", kind: "secrets", label: "Secrets", icon: KeyRound, key: "x" },
  { to: "/envs/$env/k8s/$kind", kind: "namespaces", label: "Namespaces", icon: FolderTree, key: "n" },
  { to: "/envs/$env/k8s/$kind", kind: "nodes", label: "Nodes", icon: Cpu, key: "e" },
] as const;

export type NavItem = (typeof NAV)[number] | (typeof KUBE_NAV)[number];

/** The href a nav item points at in `env`. */
export function navHref(item: NavItem, env: string): string {
  const base = item.to.replace("$env", encodeURIComponent(env));
  return "kind" in item ? base.replace("$kind", item.kind) : base;
}
```

`web/src/components/Shell.tsx`:
- `Shell` reads `const { data: envs = [] } = useQuery(envsQuery); const kube = isKube(envs.find((e) => e.name === env)?.kind);` and passes `kube` to `Rail`, `TopBar` and `useGoKeys`.
- `Rail` uses `const items = kube ? KUBE_NAV : NAV`. For each item it renders `<Link to={item.to} params={"kind" in item ? { env, kind: item.kind } : { env }} activeOptions={{ exact: item.to === "/envs/$env" }} …>`. Cast `params` to `never` if TanStack's typing refuses the union. For the Workloads item, mark it active when the current pathname is any workload list: compare `useLocation().pathname` against `navHref` for each of `WORKLOADS`, and add `data-status="active"`.
- `useGoKeys(env, kube)` looks keys up in `kube ? KUBE_NAV : NAV` and navigates with the same params.
- `TopBar` finds `section` among `kube ? KUBE_NAV : NAV` by `pathname.startsWith(navHref(n, env))`, keeping the reversed search order.
- `{env && <EnvLive env={env} />}` becomes `{env && !kube && <EnvLive env={env} />}`. Kubernetes environments have no Docker event stream.
- `EnvStatusBadge({ env, kube })` uses `envStatusQuery(env, kube)`. The switcher passes `kube` for the current env.
- In the switcher list, show a small `Boxes` icon (aria-label "Kubernetes") after the name of kube envs.

`web/src/api/queries.ts`:

```ts
/** How often a Kubernetes environment's status is read; nothing pushes it. */
export const KUBE_STATUS_POLL_MS = 15000;
/** How often a visible Kubernetes list refreshes. */
export const KUBE_LIST_POLL_MS = 5000;

export const envStatusQuery = (env: string, kube = false) =>
  queryOptions({
    queryKey: keys.envStatus(env),
    queryFn: async () => (await getEnvStatus(path(env))).data,
    refetchInterval: (q) =>
      q.state.data?.state === "down" ? ENV_DOWN_POLL_MS : kube ? KUBE_STATUS_POLL_MS : false,
  });

export const kubeListQuery = (env: string, kind: KubeKind, ns?: string) =>
  queryOptions({
    queryKey: keys.kube(env, kind, ns),
    queryFn: async () => (await listKube({ path: { env, kind }, query: ns ? { namespace: ns } : {} })).data,
    refetchInterval: KUBE_LIST_POLL_MS,
    retry: (failures, err) => !(err instanceof ApiError && err.status === 403) && failures < 1,
  });
```

and in `keys`:

```ts
  kube: (env: string, kind: string, ns?: string) => ["env", env, "k8s", kind, ns ?? "*"] as const,
  kubeObject: (env: string, kind: string, ns: string, name: string) => ["env", env, "k8s", kind, ns, name] as const,
```

TanStack Query stops interval refetches while the document is hidden (`refetchIntervalInBackground` defaults to false), which gives "only while visible".

`web/src/pages/kube/NamespacePicker.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { useCallback, useState } from "react";
import { kubeListQuery } from "@/api/queries";

const storageKey = (env: string) => `fleetops.ns.${env}`;

function read(env: string): string | undefined {
  try {
    return localStorage.getItem(storageKey(env)) || undefined;
  } catch {
    return undefined;
  }
}

/** The namespace a Kubernetes environment's lists show, remembered per environment. `undefined` is all. */
export function useNamespace(env: string): [string | undefined, (ns?: string) => void] {
  const [ns, setNs] = useState(() => read(env));
  const set = useCallback(
    (next?: string) => {
      setNs(next);
      try {
        if (next) localStorage.setItem(storageKey(env), next);
        else localStorage.removeItem(storageKey(env));
      } catch {
        /* storage blocked: the choice lasts this page only */
      }
    },
    [env],
  );
  return [ns, set];
}

export function NamespacePicker({ env, value, onChange }: { env: string; value?: string; onChange: (ns?: string) => void }) {
  const { data } = useQuery(kubeListQuery(env, "namespaces"));
  const names = (data?.rows ?? []).map((r) => r.name);
  return (
    <label className="flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Namespace</span>
      <select
        aria-label="Namespace"
        className="h-8 rounded-md border bg-background px-2"
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value || undefined)}
      >
        <option value="">All namespaces</option>
        {names.map((n) => (
          <option key={n} value={n}>{n}</option>
        ))}
      </select>
    </label>
  );
}
```

`web/src/pages/kube/KubeHealth.tsx`:

```tsx
import type { Health } from "@/api/gen";
import { cn } from "@/lib/utils";

const TONE: Record<Health, string> = { ok: "bg-live", warn: "bg-state-wait", bad: "bg-state-fail", unknown: "bg-muted-foreground/50" };

export function KubeHealth({ health, label }: { health: Health; label: string }) {
  return (
    <span className="flex items-center gap-2">
      <span aria-hidden className={cn("size-2 shrink-0 rounded-full", TONE[health])} />
      {label}
    </span>
  );
}
```

`web/src/pages/kube/KubeListPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { useMemo } from "react";
import type { KubeKind, KubeRow } from "@/api/gen";
import { ApiError } from "@/api/client";
import { kubeListQuery } from "@/api/queries";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { KubeHealth } from "./KubeHealth";
import { CLUSTER_SCOPED, KIND_COLUMNS, KIND_LABEL, STATUS_HEADER, WORKLOADS, asKind } from "./kinds";
import { NamespacePicker, useNamespace } from "./NamespacePicker";

const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind");

function kindColumns(env: string, kind: KubeKind) {
  const col = columns<KubeRow>();
  const scoped = !CLUSTER_SCOPED.includes(kind);
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/k8s/$kind/$ns/$name"
          params={{ env, kind, ns: c.row.original.namespace ?? "-", name: c.getValue() }}
          className="font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    ...(scoped ? [col.accessor((r) => r.namespace ?? "", { id: "namespace", header: "Namespace" })] : []),
    col.accessor("status", {
      header: STATUS_HEADER[kind] ?? "Status",
      cell: (c) => <KubeHealth health={c.row.original.health} label={c.getValue()} />,
    }),
    ...KIND_COLUMNS[kind].map(([key, header]) => col.accessor((r) => r.columns[key] ?? "", { id: key, header })),
    col.accessor("created", { header: "Age", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

export function KubeListPage() {
  const { env, kind: raw } = route.useParams();
  const kind = asKind(raw) ?? "pods";
  const [ns, setNs] = useNamespace(env);
  const scoped = !CLUSTER_SCOPED.includes(kind);
  const list = useQuery(kubeListQuery(env, kind, scoped ? ns : undefined));
  const cols = useMemo(() => kindColumns(env, kind), [env, kind]);
  const forbidden = list.error instanceof ApiError && list.error.status === 403;
  const where = ns ? `namespace ${ns}` : "all namespaces";
  return (
    <Page
      title={KIND_LABEL[kind]}
      actions={scoped ? <NamespacePicker env={env} value={ns} onChange={setNs} /> : undefined}
    >
      {WORKLOADS.includes(kind) && (
        <nav aria-label="Workload kinds" className="mb-3 flex flex-wrap gap-1">
          {WORKLOADS.map((k) => (
            <Link
              key={k}
              to="/envs/$env/k8s/$kind"
              params={{ env, kind: k }}
              className="rounded-md px-2.5 py-1 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium"
            >
              {KIND_LABEL[k]}
            </Link>
          ))}
        </nav>
      )}
      {list.data?.truncated && (
        <p role="status" className="mb-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
          Showing the first 5,000. Pick a namespace to narrow the list.
        </p>
      )}
      <DataTable
        label={KIND_LABEL[kind]}
        data={forbidden ? [] : list.data?.rows}
        columns={cols}
        getRowId={(r) => `${r.namespace ?? "-"}/${r.name}`}
        error={forbidden ? undefined : list.error}
        empty={forbidden ? `No permission to list ${KIND_LABEL[kind]} in ${scoped ? where : "this cluster"}` : `No ${KIND_LABEL[kind].toLowerCase()} in ${scoped ? where : "this cluster"}`}
      />
    </Page>
  );
}
```

`web/src/pages/kube/KubeOverviewPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { envStatusQuery, kubeListQuery } from "@/api/queries";
import { Page } from "@/components/Page";

export function KubeOverviewPage({ env }: { env: string }) {
  const status = useQuery(envStatusQuery(env, true));
  const pods = useQuery(kubeListQuery(env, "pods"));
  const s = status.data;
  const version = s?.state === "up" && s.version.kind === "kube" ? s.version : undefined;
  const phases = new Map<string, number>();
  for (const p of pods.data?.rows ?? []) phases.set(p.status, (phases.get(p.status) ?? 0) + 1);
  return (
    <Page title="Overview">
      {s?.state === "down" && (
        <p role="alert" className="mb-4 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          {s.reason}
        </p>
      )}
      {version && (
        <dl className="grid gap-4 sm:grid-cols-3">
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Kubernetes</dt>
            <dd className="font-mono text-lg">{version.git_version}</dd>
            <dd className="text-xs text-muted-foreground">{version.platform}</dd>
          </div>
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Nodes</dt>
            <dd className="text-lg">{version.nodes_ready} of {version.nodes} nodes ready</dd>
          </div>
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Pods</dt>
            <dd className="grid gap-0.5 text-sm">
              {[...phases].map(([phase, n]) => (
                <span key={phase}>{n} {phase}</span>
              ))}
            </dd>
          </div>
        </dl>
      )}
    </Page>
  );
}
```

`web/src/pages/EnvHome.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { lazy, Suspense } from "react";
import { envsQuery } from "@/api/queries";
import { isKube } from "@/lib/env";

const DashboardPage = lazy(() => import("@/pages/DashboardPage").then((m) => ({ default: m.DashboardPage })));
const KubeOverviewPage = lazy(() => import("@/pages/kube/KubeOverviewPage").then((m) => ({ default: m.KubeOverviewPage })));
const route = getRouteApi("/authed/shell/envs/$env/");

/** `/envs/$env`: the Docker dashboard or the Kubernetes overview. */
export function EnvHome() {
  const { env } = route.useParams();
  const { data: envs } = useQuery(envsQuery);
  if (!envs) return null;
  const kube = isKube(envs.find((e) => e.name === env)?.kind);
  return <Suspense fallback={null}>{kube ? <KubeOverviewPage env={env} /> : <DashboardPage />}</Suspense>;
}
```

`web/src/router.tsx`:
- `envRoute` gains:

```ts
  // A Kubernetes environment has no Docker pages and a Docker one no
  // Kubernetes pages: an old link or a renamed kind lands on the home page.
  beforeLoad: async ({ context, params, location }) => {
    const envs = await context.queryClient.ensureQueryData(envsQuery);
    const env = envs.find((e) => e.name === params.env);
    if (!env) return;
    const rest = location.pathname.slice(`/envs/${encodeURIComponent(params.env)}`.length);
    const home = rest === "" || rest === "/";
    const kubePage = rest.startsWith("/k8s/");
    if (!home && kubePage !== isKube(env.kind)) throw redirect({ to: "/envs/$env", params: { env: params.env } });
  },
```

- `dashboardRoute.component` becomes `lazyRouteComponent(() => import("@/pages/EnvHome"), "EnvHome")`;
- add:

```ts
const kubeListRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/$kind",
  component: lazyRouteComponent(() => import("@/pages/kube/KubeListPage"), "KubeListPage"),
});
```

- add both new routes to `envRoute.addChildren([...])`. The object route comes in Task 15. Until then, add it as a route whose component is `KubeListPage`, so its links resolve. Task 15 replaces that component.

If the `DashboardPage` test reads params with `getRouteApi("/authed/shell/envs/$env/")`, nothing changes for it, because the route id is unchanged.

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm -C web exec vitest run src/pages/kube 2>&1 | tail -5 && make test-web 2>&1 | tail -5`
Expected: 7 passed; the whole web suite passes (lint, types and tests).

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): Kubernetes sidebar, overview, kind lists and namespace picker

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 15: Web, part 2: the object page (summary, YAML, events, logs, terminal, actions)

**Files:**
- Create: `web/src/pages/kube/KubeObjectPage.tsx`, `web/src/pages/kube/tabs.ts`, `web/src/pages/kube/KubeActions.tsx`
- Modify: `web/src/router.tsx` (`kubeObjectRoute` uses the page, validates `?tab=`), `web/src/api/queries.ts` (`kubeObjectQuery`, `kubeEventsQuery`)
- Modify: `web/src/pages/containers/logs/useLogStream.ts` and `LogsTab.tsx`, and `web/src/pages/containers/terminal/TerminalView.tsx` (accept `container`)
- Test: `web/src/pages/kube/object.test.tsx`

**Interfaces:**
- Consumes:
  - `getKube`, `kubeEvents`, `deleteKube`, `scaleKube` and `restartKube` from the generated SDK (Task 12);
  - `LogsTab` and `TerminalView` (existing);
  - `KIND_LABEL`, `SCALABLE` and `RESTARTABLE` (Task 14).
- Produces:
  - `KUBE_TABS = ["summary", "yaml", "events", "logs", "terminal"] as const`;
  - `kubeObjectQuery(env, kind, ns, name)` and `kubeEventsQuery(env, kind, ns, name)`;
  - `LogsTab` gains an optional `container?: string`, passed into the subscription's `opts`;
  - `TerminalView`'s `opts` accepts `container`.

- [ ] **Step 1: Write the failing tests**

`web/src/pages/kube/object.test.tsx`:

```tsx
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";

const deployment = {
  row: { name: "web", namespace: "apps", created: 1, status: "2/3", health: "warn", columns: { ready: "2/3" } },
  yaml: "kind: Deployment\nmetadata:\n  name: web\n",
  conditions: [{ type: "Available", status: "True", reason: "MinimumReplicasAvailable", message: null }],
  owners: [],
  containers: [],
};
const pod = {
  row: { name: "web-1", namespace: "apps", created: 1, status: "Running", health: "ok", columns: {} },
  yaml: "kind: Pod\n",
  conditions: [],
  owners: [{ kind: "ReplicaSet", name: "web-7d9", link: null }, { kind: "Deployment", name: "web", link: "deployments" }],
  containers: ["main", "side"],
};
const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment },
  "GET /api/v1/envs/k/k8s/deployments/apps/web/events": { json: [{ type: "Normal", reason: "ScalingReplicaSet", message: "Scaled up", count: 1, last_seen: 1, source: "deployment-controller" }] },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: pod },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1/events": { json: [] },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/scale": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/restart": { status: 204 },
  "DELETE /api/v1/envs/k/k8s/deployments/apps/web": { status: 204 },
};

describe("a Kubernetes object's page", () => {
  it("shows the summary, YAML and events", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/web");
    expect(await screen.findByRole("heading", { name: "web" })).toBeInTheDocument();
    expect(screen.getByText("MinimumReplicasAvailable")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /YAML/ }));
    expect(await screen.findByText(/kind: Deployment/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /Events/ }));
    expect(await screen.findByText("Scaled up")).toBeInTheDocument();
  });

  it("scales, restarts and deletes", async () => {
    const calls = mockFetch(routes);
    const { history } = renderApp("/envs/k/k8s/deployments/apps/web");
    await userEvent.click(await screen.findByRole("button", { name: "Scale" }));
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText("Replicas");
    await userEvent.clear(input);
    await userEvent.type(input, "5");
    await userEvent.click(within(dialog).getByRole("button", { name: "Scale" }));
    expect(calls.find((c) => c.method === "POST" && c.path.endsWith("/scale"))?.body).toEqual({ replicas: 5 });
    await userEvent.click(screen.getByRole("button", { name: "Restart" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restart" }));
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/restart"))).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
    await screen.findByRole("table", { name: "Deployments" });
    expect(history.location.pathname).toBe("/envs/k/k8s/deployments");
  });

  it("a pod links its owners and streams logs from the chosen container", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/pods/apps/web-1?tab=logs");
    expect(await screen.findByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/k/k8s/deployments/apps/web");
    expect(screen.queryByRole("button", { name: "Scale" })).not.toBeInTheDocument();
    const ws = FakeWebSocket.latest()!;
    ws.open();
    await userEvent.selectOptions(await screen.findByRole("combobox", { name: "Container" }), "side");
    const subs = ws.frames().filter((m) => m.op === "sub" && m.topic === "logs");
    const last = subs[subs.length - 1];
    expect(last).toMatchObject({ env: "k", id: "apps/web-1", opts: { container: "side" } });
  });
});
```

Match `FakeWebSocket`'s real method names (`frames()` returns parsed client messages, per `src/test/fake-ws.ts`). Match the `ConfirmDialog` role too (`alertdialog`, or `dialog` if that's what it renders).

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm -C web exec vitest run src/pages/kube/object 2>&1 | tail -10`
Expected: failures; the object route still renders the list page.

- [ ] **Step 3: Implement**

`web/src/pages/kube/tabs.ts`:

```ts
export const KUBE_TABS = ["summary", "yaml", "events", "logs", "terminal"] as const;
export type KubeTab = (typeof KUBE_TABS)[number];
```

`web/src/api/queries.ts`:

```ts
export const kubeObjectQuery = (env: string, kind: KubeKind, ns: string, name: string) =>
  queryOptions({
    queryKey: keys.kubeObject(env, kind, ns, name),
    queryFn: async () => (await getKube({ path: { env, kind, ns, name } })).data,
    retry: retryUnlessGone,
  });

export const kubeEventsQuery = (env: string, kind: KubeKind, ns: string, name: string) =>
  queryOptions({
    queryKey: [...keys.kubeObject(env, kind, ns, name), "events"] as const,
    queryFn: async () => (await kubeEvents({ path: { env, kind, ns, name } })).data,
    refetchInterval: KUBE_LIST_POLL_MS,
  });
```

`web/src/router.tsx`:

```ts
const kubeObjectRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/$kind/$ns/$name",
  validateSearch: (s: Record<string, unknown>): { tab?: KubeTab } => ({ tab: KUBE_TABS.find((t) => t === s.tab) }),
  component: lazyRouteComponent(() => import("@/pages/kube/KubeObjectPage"), "KubeObjectPage"),
});
```

Logs and terminal accept a container:
- `useLogStream(env, id, query, startedAt)`: add `container?: string` to its `LogQuery` type, and spread it into `opts` with the rest of the query.
- `LogsTab` gains `container?: string` and passes it in the query.
- `TerminalView`'s `opts` type becomes `Pick<ExecOptions, "cmd" | "user" | "container">`. It already spreads `opts` into the subscription.
- `TerminalTab` gains `container?: string` and passes it into `opts`.

The ids are used only as the topic `id`. A pod's id is `"<ns>/<name>"`.

`web/src/pages/kube/KubeActions.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import type { KubeKind } from "@/api/gen";
import { deleteKube, restartKube, scaleKube } from "@/api/gen";
import { keys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { FormDialog } from "@/pages/admin/FormDialog";
import { KIND_LABEL, RESTARTABLE, SCALABLE } from "./kinds";

export function KubeActions({ env, kind, ns, name, replicas }: { env: string; kind: KubeKind; ns: string; name: string; replicas?: number }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [open, setOpen] = useState<"scale" | "restart" | "delete" | null>(null);
  const [count, setCount] = useState(String(replicas ?? 1));
  const path = { env, kind, ns, name };
  const refresh = () => qc.invalidateQueries({ queryKey: keys.kube(env, kind).slice(0, 4) });
  const restart = useMutation({ mutationFn: () => restartKube({ path, throwOnError: true }), onSuccess: refresh });
  const remove = useMutation({
    mutationFn: () => deleteKube({ path, throwOnError: true }),
    onSuccess: async () => {
      await refresh();
      void navigate({ to: "/envs/$env/k8s/$kind", params: { env, kind } });
    },
  });
  const label = KIND_LABEL[kind].replace(/s$/, "");
  return (
    <div className="flex gap-2">
      {SCALABLE.includes(kind) && <Button variant="outline" size="sm" onClick={() => setOpen("scale")}>Scale</Button>}
      {RESTARTABLE.includes(kind) && <Button variant="outline" size="sm" onClick={() => setOpen("restart")}>Restart</Button>}
      <Button variant="outline" size="sm" onClick={() => setOpen("delete")}>Delete</Button>
      {open === "scale" && (
        <FormDialog
          title={`Scale ${name}`}
          submitLabel="Scale"
          onClose={() => setOpen(null)}
          onSubmit={async () => {
            await scaleKube({ path, body: { replicas: Number(count) }, throwOnError: true });
            await refresh();
          }}
        >
          <label className="grid gap-1 text-sm">
            Replicas
            <input type="number" min={0} className="h-9 rounded-md border px-2" value={count} onChange={(e) => setCount(e.target.value)} />
          </label>
        </FormDialog>
      )}
      {open === "restart" && (
        <ConfirmDialog
          title={`Restart ${name}?`}
          description={`Its pods are replaced one by one, as kubectl rollout restart does.`}
          confirmLabel="Restart"
          onConfirm={() => restart.mutateAsync()}
          onClose={() => setOpen(null)}
        />
      )}
      {open === "delete" && (
        <ConfirmDialog
          title={`Delete ${label} ${name}?`}
          description={kind === "namespaces" ? "Everything in the namespace is deleted with it." : "This cannot be undone."}
          confirmLabel="Delete"
          destructive
          onConfirm={() => remove.mutateAsync()}
          onClose={() => setOpen(null)}
        />
      )}
    </div>
  );
}
```

Match `ConfirmDialog`'s and `FormDialog`'s real props (see `components/ConfirmDialog.tsx` and `pages/admin/FormDialog.tsx`), keeping the labels the tests click.

`web/src/pages/kube/KubeObjectPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileText, Info, ListTree, ScrollText, TerminalSquare } from "lucide-react";
import { Fragment, lazy, Suspense, useState } from "react";
import { kubeEventsQuery, kubeObjectQuery } from "@/api/queries";
import { DetailError } from "@/components/Details";
import { Page } from "@/components/Page";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatAgo } from "@/lib/format";
import { KubeActions } from "./KubeActions";
import { KubeHealth } from "./KubeHealth";
import { asKind, KIND_LABEL } from "./kinds";
import type { KubeTab } from "./tabs";

const LogsTab = lazy(() => import("@/pages/containers/logs/LogsTab").then((m) => ({ default: m.LogsTab })));
const TerminalTab = lazy(() => import("@/pages/containers/terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind/$ns/$name");

export function KubeObjectPage() {
  const { env, kind: raw, ns, name } = route.useParams();
  const { tab = "summary" } = route.useSearch();
  const navigate = useNavigate();
  const kind = asKind(raw) ?? "pods";
  const obj = useQuery(kubeObjectQuery(env, kind, ns, name));
  const events = useQuery({ ...kubeEventsQuery(env, kind, ns, name), enabled: tab === "events" });
  const [container, setContainer] = useState<string | undefined>();
  if (obj.error) return <DetailError error={obj.error} what={KIND_LABEL[kind].replace(/s$/, "").toLowerCase()} back={{ to: "/envs/$env/k8s/$kind", params: { env, kind } }} />;
  const o = obj.data;
  const isPod = kind === "pods";
  const chosen = container ?? o?.containers[0];
  const replicas = Number(o?.row.columns.ready?.split("/")[1] ?? 1);
  const tabs: [KubeTab, string, typeof Info][] = [
    ["summary", "Summary", Info],
    ["yaml", "YAML", FileText],
    ["events", "Events", ListTree],
    ...(isPod ? ([["logs", "Logs", ScrollText], ["terminal", "Terminal", TerminalSquare]] as [KubeTab, string, typeof Info][]) : []),
  ];
  return (
    <Page title={name} meta={o && <KubeHealth health={o.row.health} label={o.row.status} />} actions={o && <KubeActions env={env} kind={kind} ns={ns} name={name} replicas={replicas} />}>
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: { tab: t as KubeTab }, replace: true })}>
        <TabsList>
          {tabs.map(([value, label, Icon]) => (
            <TabsTrigger key={value} value={value}>
              <Icon aria-hidden strokeWidth={1.75} />
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="summary">
          {o && (
            <div className="grid gap-4">
              <dl className="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                <dt className="text-muted-foreground">Kind</dt><dd>{KIND_LABEL[kind].replace(/s$/, "")}</dd>
                {o.row.namespace && (<><dt className="text-muted-foreground">Namespace</dt><dd>{o.row.namespace}</dd></>)}
                <dt className="text-muted-foreground">Age</dt><dd>{formatAgo(o.row.created)}</dd>
                {Object.entries(o.row.columns).map(([k, v]) => (
                  <Fragment key={k}>
                    <dt className="text-muted-foreground">{k.replace(/_/g, " ")}</dt>
                    <dd>{v || "—"}</dd>
                  </Fragment>
                ))}
              </dl>
              {o.owners.length > 0 && (
                <p className="text-sm">
                  Owned by{" "}
                  {o.owners.map((r, i) => (
                    <span key={`${r.kind}/${r.name}`}>
                      {i > 0 && " → "}
                      {r.link ? (
                        <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: r.link, ns, name: r.name }} className="hover:underline">{r.name}</Link>
                      ) : (
                        <span>{r.name}</span>
                      )}{" "}
                      <span className="text-muted-foreground">({r.kind})</span>
                    </span>
                  ))}
                </p>
              )}
              {o.conditions.length > 0 && (
                <table className="text-sm" aria-label="Conditions">
                  <thead><tr><th className="text-left">Condition</th><th className="text-left">Status</th><th className="text-left">Reason</th><th className="text-left">Message</th></tr></thead>
                  <tbody>{o.conditions.map((c) => (<tr key={c.type}><td>{c.type}</td><td>{c.status}</td><td>{c.reason}</td><td>{c.message}</td></tr>))}</tbody>
                </table>
              )}
            </div>
          )}
        </TabsContent>
        <TabsContent value="yaml">
          {o && <pre aria-label="Object YAML" className="overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">{o.yaml}</pre>}
        </TabsContent>
        <TabsContent value="events">
          <ul className="grid gap-2 text-sm">
            {(events.data ?? []).map((e, i) => (
              <li key={i} className={e.type === "Warning" ? "text-state-fail" : undefined}>
                <span className="font-medium">{e.reason}</span> {e.message}{" "}
                <span className="text-muted-foreground">{e.last_seen ? formatAgo(e.last_seen) : ""} ×{e.count}</span>
              </li>
            ))}
            {events.data?.length === 0 && <li className="text-muted-foreground">No recent events.</li>}
          </ul>
        </TabsContent>
        {isPod && o && (
          <>
            <ContainerPicker containers={o.containers} value={chosen} onChange={setContainer} hidden={tab !== "logs" && tab !== "terminal"} />
            <TabsContent value="logs">
              <Suspense fallback={null}>
                <LogsTab key={chosen} env={env} id={`${ns}/${name}`} name={name} running={o.row.status === "Running"} container={chosen} />
              </Suspense>
            </TabsContent>
            <TabsContent value="terminal">
              <Suspense fallback={null}>
                <TerminalTab key={chosen} env={env} id={`${ns}/${name}`} running={o.row.status === "Running"} container={chosen} />
              </Suspense>
            </TabsContent>
          </>
        )}
      </Tabs>
    </Page>
  );
}

function ContainerPicker({ containers, value, onChange, hidden }: { containers: string[]; value?: string; onChange: (c: string) => void; hidden: boolean }) {
  if (hidden || containers.length < 2) return null;
  return (
    <label className="my-2 flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Container</span>
      <select aria-label="Container" className="h-8 rounded-md border bg-background px-2" value={value} onChange={(e) => onChange(e.target.value)}>
        {containers.map((c) => (<option key={c} value={c}>{c}</option>))}
      </select>
    </label>
  );
}
```

Match `DetailError`'s `back` prop shape.

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm -C web exec vitest run src/pages/kube 2>&1 | tail -5 && make test-web 2>&1 | tail -5`
Expected: 10 passed in `src/pages/kube`; the whole web suite passes, including the existing logs and terminal tests.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): Kubernetes object page with YAML, events, logs, terminal and actions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

### Task 16: Web, part 3: adding Kubernetes environments in Administration

**Files:**
- Modify: `web/src/pages/admin/EnvironmentsPage.tsx` (four kinds, kubeconfig form, context picker, edit), `web/src/pages/admin/AgentEnrollment.tsx` (title and help by kind)
- Test: `web/src/pages/admin/admin.test.tsx` (append)

**Interfaces:**
- Consumes: `createEnv` with `kubeconfig`, `context` and `skip_check`; `updateEnv` with `kubeconfig`; `kubeContexts({ body: { kubeconfig } })`; `EnvView.kube` (Task 11).
- Produces: the form's kind radios, labelled:
  - "Local socket or Docker API URL";
  - "Agent on a remote host";
  - "Kubernetes cluster (kubeconfig)";
  - "Kubernetes cluster (in-cluster agent)".

- [ ] **Step 1: Write the failing tests**

Append to `web/src/pages/admin/admin.test.tsx`, reusing its `adminRoutes`/`agentRoutes` setup and its `row()` helper:

```tsx
describe("Kubernetes environments", () => {
  const KUBECONFIG = "apiVersion: v1\nkind: Config\ncontexts: [{name: dev}, {name: prod}]\n";

  it("adds a kubeconfig cluster after picking its context", async () => {
    const calls = mockFetch({
      ...agentRoutes,
      "POST /api/v1/kube/contexts": { json: { contexts: ["dev", "prod"], current: "dev" } },
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "k", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, enrollment: null } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /kubeconfig/i }));
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "k");
    await userEvent.click(within(dialog).getByLabelText("Kubeconfig"));
    await userEvent.paste(KUBECONFIG);
    await userEvent.tab();
    const ctx = await within(dialog).findByRole("combobox", { name: "Context" });
    expect(ctx).toHaveValue("dev");
    await userEvent.selectOptions(ctx, "prod");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs")?.body).toEqual({
      name: "k", kind: "kube-config", kubeconfig: KUBECONFIG, context: "prod", skip_check: false,
    });
  });

  it("shows an unreachable cluster's error inline and can save without the check", async () => {
    let attempt = 0;
    const calls = mockFetch({
      ...agentRoutes,
      "POST /api/v1/kube/contexts": { json: { contexts: ["dev"], current: "dev" } },
      "POST /api/v1/envs": () =>
        ++attempt === 1
          ? json(400, { code: "invalid", message: "invalid input: cannot reach cluster: timed out", request_id: null })
          : json(201, { id: 9, name: "k", kind: "kube-config", url: "https://k", created_at: 1, updated_at: 1, enrollment: null }),
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /kubeconfig/i }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "k");
    await userEvent.click(within(dialog).getByLabelText("Kubeconfig"));
    await userEvent.paste(KUBECONFIG);
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(await within(dialog).findByText(/cannot reach cluster/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /without checking/i }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    const posts = calls.filter((c) => c.method === "POST" && c.path === "/api/v1/envs");
    expect(posts[1].body).toMatchObject({ skip_check: true });
  });

  it("adds an agent cluster and shows the kubectl command", async () => {
    mockFetch({
      ...agentRoutes,
      "POST /api/v1/envs": { status: 201, json: { id: 9, name: "k8s-01", kind: "kube-agent", url: "agent://9", created_at: 1, updated_at: 1,
        enrollment: { join_token: "fj_abc", expires_at: Math.floor(Date.now() / 1000) + 900, command: "kubectl apply -f - <<'EOF'\n…\nEOF\n" } } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /in-cluster agent/i }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "k8s-01");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    const shown = await screen.findByRole("dialog", { name: /run this against the cluster/i });
    expect(within(shown).getByText(/kubectl apply/)).toBeInTheDocument();
  });

  it("lists a kubeconfig cluster's server and context", async () => {
    mockFetch({
      ...agentRoutes,
      "GET /api/v1/envs": { json: [{ id: 9, name: "k", kind: "kube-config", url: "https://k:6443", created_at: 1, updated_at: 1, agent: null,
        kube: { server: "https://k:6443", context: "prod", user_kind: "token" } }] },
    });
    const { screen } = renderApp("/admin/envs");
    const r = await row(screen, "k");
    expect(within(r).getByText(/prod/)).toBeInTheDocument();
    expect(within(r).getByText(/token/)).toBeInTheDocument();
  });
});
```

Import `json` from `@/test/fetch` if it isn't imported already.

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm -C web exec vitest run src/pages/admin 2>&1 | tail -10`
Expected: the 4 new tests fail, because there are no kube radios.

- [ ] **Step 3: Implement**

In `EnvironmentsPage.tsx`:
- `type Kind = EnvKind` from the generated types, replacing the `"docker-local" | "docker-agent"` union and the `agent` boolean. `isAgent(e)` becomes `e.kind === "docker-agent" || e.kind === "kube-agent"`, so Rotate works for both.
- Render the kind radios from a list (only on create):

```tsx
const KINDS: [EnvKind, string][] = [
  ["docker-local", "Local socket or Docker API URL"],
  ["docker-agent", "Agent on a remote host"],
  ["kube-config", "Kubernetes cluster (kubeconfig)"],
  ["kube-agent", "Kubernetes cluster (in-cluster agent)"],
];
```

- Per kind:
  - **`docker-local`:** the Docker URL field, as today.
  - **`kube-config`:**
    - A `<textarea id="env-kubeconfig">` labelled "Kubeconfig". Its `onBlur` and `onPaste` (deferred with `setTimeout(…, 0)`) call `kubeContexts({ body: { kubeconfig }, throwOnError: true })`. They fill `contexts` and set `context` to `current ?? contexts[0]`. A failure shows its message under the textarea.
    - A "Context" `<select>` appears once `contexts.length > 0`.
    - A checkbox, "Save without checking that the cluster answers", is bound to `skipCheck`.
    - Help text: "Paste a kubeconfig with a ServiceAccount token or client certificate. It is stored encrypted and never shown again. exec plugins (aws, gcloud) are not supported: use the in-cluster agent instead."
  - **`kube-agent`:** no fields beyond Name. Help text: "You'll get a kubectl command that installs the agent in the cluster. The cluster needs no inbound access."
- Submit for `kube-config` sends `createEnv({ body: { name, kind, kubeconfig, context, skip_check: skipCheck }, throwOnError: true })`. Leave out `context` when it is undefined, so the request body matches the test. The `kube-agent` submit is the agent path with `kind: "kube-agent"`.
- Edit for `kube-config` shows an optional "Replace kubeconfig" textarea and sends `updateEnv({ body: { name, kubeconfig?, skip_check } })`. The URL field is hidden for every kind but `docker-local`.
- The URL column for rows with `e.kube` shows `${e.kube.server} · ${e.kube.context} (${e.kube.user_kind})`.

In `AgentEnrollment.tsx`, take `kind: EnvKind` and pass it from the page (store the created or rotated env's kind with the enrollment). The title is "Run this against the cluster" for `kube-agent` and "Run this on the remote host" otherwise. For `kube-agent`, add the line "Run it with a kubectl context for the target cluster."

- [ ] **Step 4: Run them to verify they pass**

Run: `make test-web 2>&1 | tail -5`
Expected: green, including the 4 new tests and every existing admin test.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): add Kubernetes clusters by kubeconfig or in-cluster agent

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---
### Task 17: End to end, smoke, footprint, CI and docs

**Files:**
- Create: `web/e2e/kube.spec.ts`, `scripts/agent-kube-smoke.sh`
- Modify: `scripts/rss-bench.sh` (kube phase), `Makefile` (`e2e-kube`, `agent-kube-smoke`, `rss-bench` passes `KUBE_IT`), `.github/workflows/ci.yml` (job `kube`)
- Modify: `README.md` (Kubernetes section, roadmap, status, backups), `docs/superpowers/specs/2026-10-01-fleetops-kubernetes-design.md` (status and the rulings below)

**Interfaces:**
- Consumes: everything above; `scripts/kind-up.sh` and its fixtures (Task 1); the images `make image` / `make image-agent` build.
- Produces: nothing other tasks consume.

- [ ] **Step 1: Write the Playwright test**

`web/e2e/kube.spec.ts`. Specs run in name order, so this runs after `core-flow`. The test also sets up the admin itself when it runs alone.

```ts
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// Sub-project 3 end to end against the kind cluster from `make kind-up`:
// add it with its kubeconfig, browse, scale, read logs, open a terminal.
// Runs only with FLEETOPS_KIND=1 (make e2e-kube).

const PASSWORD = "e2e-password-1";
const ENV = `kind-${Date.now().toString(36)}`;

test.skip(!process.env.FLEETOPS_KIND, "needs a kind cluster: make e2e-kube");

test.beforeAll(async ({ request }) => {
  const status = await (await request.get("/api/v1/auth/status")).json();
  if (status.setup_required) {
    await request.post("/api/v1/auth/setup", { data: { username: "admin", password: PASSWORD } });
  }
});

test("a kubeconfig cluster can be browsed, scaled, logged and entered", async ({ page }) => {
  const kubeconfig = readFileSync(join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig"), "utf8");

  await test.step("log in", async () => {
    await page.goto("/login");
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page.getByRole("navigation", { name: "Environment" }).or(page.getByText("No environments"))).toBeVisible();
  });

  await test.step("add the cluster", async () => {
    await page.goto("/admin/envs");
    await page.getByRole("button", { name: "Add environment" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("radio", { name: /kubeconfig/i }).check();
    await dialog.getByLabel("Name").fill(ENV);
    await dialog.getByLabel("Kubeconfig").fill(kubeconfig);
    await dialog.getByLabel("Kubeconfig").blur();
    await expect(dialog.getByRole("combobox", { name: "Context" })).toHaveValue("kind-fleetops-test");
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("browse deployments in a namespace", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments`);
    await page.getByRole("combobox", { name: "Namespace" }).selectOption("fleetops-it");
    await expect(page.getByRole("link", { name: "web", exact: true })).toBeVisible();
  });

  await test.step("scale web to 3", async () => {
    await page.getByRole("link", { name: "web", exact: true }).click();
    await page.getByRole("button", { name: "Scale" }).click();
    await page.getByRole("dialog").getByLabel("Replicas").fill("3");
    await page.getByRole("dialog").getByRole("button", { name: "Scale" }).click();
    await expect(page.getByText(/\/3$/).first()).toBeVisible({ timeout: 60_000 });
  });

  await test.step("read the ticker's logs", async () => {
    await page.goto(`/envs/${ENV}/k8s/pods/fleetops-it/ticker?tab=logs`);
    await expect(page.getByText(/^tick \d+/).first()).toBeVisible({ timeout: 30_000 });
  });

  await test.step("open a terminal in the ticker", async () => {
    await page.goto(`/envs/${ENV}/k8s/pods/fleetops-it/ticker?tab=terminal`);
    await page.getByRole("button", { name: /connect/i }).click();
    await page.locator(".xterm").click();
    await page.keyboard.type("echo hi-from-e2e\n");
    await expect(page.locator(".xterm")).toContainText("hi-from-e2e", { timeout: 15_000 });
  });

  await test.step("scale back and remove the environment", async () => {
    await page.goto(`/envs/${ENV}/k8s/deployments/fleetops-it/web`);
    await page.getByRole("button", { name: "Scale" }).click();
    await page.getByRole("dialog").getByLabel("Replicas").fill("2");
    await page.getByRole("dialog").getByRole("button", { name: "Scale" }).click();
    await page.goto("/admin/envs");
    const row = page.getByRole("row", { name: new RegExp(ENV) });
    await row.getByRole("button", { name: "Delete" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete" }).click();
    await expect(row).toBeHidden();
  });
});
```

Use the exact button names that `TerminalTab` (`Connect` or `Open terminal`) and the environments page's delete confirmation render.

- [ ] **Step 2: The in-cluster smoke test**

`scripts/agent-kube-smoke.sh` (`chmod +x`):

```sh
#!/bin/sh
# The agent's Kubernetes mode end to end, with the real images: the server
# runs on kind's Docker network with TLS (its self-signed cert names
# "fleetops"), the generated manifest is applied to the kind cluster, and
# the environment must come up and list the agent's own Deployment.
#
# Usage: scripts/agent-kube-smoke.sh [server image] [agent image]
set -eu

server_image=${1:-fleetops:dev}
agent_image=${2:-fleetops-agent:dev}
cluster=${KIND_CLUSTER:-fleetops-test}
srv=fleetops-kube-smoke-$$
work=$(mktemp -d)
jar="$work/jar"
export KUBECONFIG="$work/kubeconfig"

cleanup() {
  docker rm -f "$srv" >/dev/null 2>&1 || true
  kubectl delete namespace fleetops --ignore-not-found --wait=false >/dev/null 2>&1 || true
  kubectl delete clusterrolebinding fleetops-agent --ignore-not-found >/dev/null 2>&1 || true
  kubectl delete clusterrole fleetops-agent --ignore-not-found >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "agent-kube-smoke: $*" >&2
  docker logs "$srv" >&2 2>&1 || true
  kubectl -n fleetops logs deploy/fleetops-agent >&2 2>&1 || true
  exit 1
}

kind get clusters 2>/dev/null | grep -qx "$cluster" || fail "no kind cluster '$cluster': run make kind-up"
kind get kubeconfig --name "$cluster" > "$KUBECONFIG"
kind load docker-image "$agent_image" --name "$cluster" >/dev/null

docker run -d --name "$srv" --network kind -p 127.0.0.1::9443 "$server_image" --tls-bind 0.0.0.0:9443 >/dev/null
ip=$(docker inspect -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}' "$srv")
base="https://$(docker port "$srv" 9443/tcp | head -n1)"
i=0
until curl -fsSk "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 30 ] || fail "server did not start"; sleep 1
done
docker cp "$srv:/data/tls/cert.pem" "$work/ca.pem" || fail "no server certificate"

origin="https://fleetops:9443"
csrf=$(curl -fsSk -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" |
  sed -n 's/.*"csrf_token":"\([^"]*\)".*/\1/p')
[ -n "$csrf" ] || fail "setup"
curl -fsSk -b "$jar" -H 'content-type: application/json' -H "Origin: $origin" -H "x-csrf-token: $csrf" \
  -d '{"name":"cluster","kind":"kube-agent"}' "$base/api/v1/envs" > "$work/env.json" || fail "create env"
python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["enrollment"]["command"])' "$work/env.json" |
  sed "s#ghcr.io/pravinbanjade/fleetops-agent:[^ ]*#$agent_image#" > "$work/apply.sh"
grep -q -- '--kube' "$work/apply.sh" || fail "command is not a kube manifest"
sh "$work/apply.sh" >/dev/null || fail "kubectl apply"

# Trust the self-signed server and resolve its name to the container.
kubectl -n fleetops create configmap fleetops-ca --from-file=ca.pem="$work/ca.pem" >/dev/null
kubectl -n fleetops patch deployment fleetops-agent --type json -p "[
  {\"op\":\"add\",\"path\":\"/spec/template/spec/hostAliases\",\"value\":[{\"ip\":\"$ip\",\"hostnames\":[\"fleetops\"]}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/volumes\",\"value\":[{\"name\":\"ca\",\"configMap\":{\"name\":\"fleetops-ca\"}}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/volumeMounts\",\"value\":[{\"name\":\"ca\",\"mountPath\":\"/ca\"}]},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/args/-\",\"value\":\"--ca\"},
  {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/args/-\",\"value\":\"/ca/ca.pem\"}
]" >/dev/null || fail "patch"

i=0
until curl -fsSk -b "$jar" "$base/api/v1/envs/cluster/status" | grep -q '"state":"up"'; do
  i=$((i + 1)); [ "$i" -lt 90 ] || fail "the cluster environment did not come up"; sleep 2
done
curl -fsSk -b "$jar" "$base/api/v1/envs/cluster/k8s/deployments?namespace=fleetops" | grep -q '"name":"fleetops-agent"' ||
  fail "the agent's Deployment is not listed through the tunnel"
kubectl -n fleetops get secret fleetops-agent-key >/dev/null || fail "the agent did not keep its key in its Secret"
echo "agent-kube-smoke: ok"
```

The manifest's `--server` is the `Origin` the request sent: `https://fleetops:9443`. The `hostAliases` patch makes `fleetops` resolve to the server container. The generated certificate names `fleetops` (see `crates/fleetops-server/src/tls.rs`).

- [ ] **Step 3: The footprint phase**

In `scripts/rss-bench.sh`, add a phase after the agent phase. It runs only when `KUBE_IT` is set (the directory `kind-up.sh` writes):

```sh
KUBE_ENVS=${KUBE_ENVS:-5}
if [ -n "${KUBE_IT:-}" ] && [ "$KUBE_ENVS" -gt 0 ]; then
  kubeconfig=$(python3 -c 'import json,sys; print(json.dumps(open(sys.argv[1]).read()))' "$KUBE_IT/kubeconfig")
  i=1
  while [ "$i" -le "$KUBE_ENVS" ]; do
    curl -fsS -b "$jar" -H 'content-type: application/json' -H "Origin: $base" -H "x-csrf-token: $csrf" \
      -d "{\"name\":\"rss-kube-$i\",\"kind\":\"kube-config\",\"kubeconfig\":$kubeconfig}" "$base/api/v1/envs" >/dev/null ||
      fail "create rss-kube-$i"
    i=$((i + 1))
  done
  round=0
  while [ "$round" -lt 3 ]; do
    i=1
    while [ "$i" -le "$KUBE_ENVS" ]; do
      for kind in pods deployments services; do
        curl -fsS -b "$jar" "$base/api/v1/envs/rss-kube-$i/k8s/$kind" >/dev/null || fail "list $kind on rss-kube-$i"
      done
      i=$((i + 1))
    done
    round=$((round + 1))
  done
  sleep "$SETTLE"
  kube_limit=$((LIMIT_MIB + AGENTS + 5))
  rss=$(peak_rss "$server_pid")
  echo "server with $AGENTS agents and $KUBE_ENVS kube environments: ${rss} MiB (limit ${kube_limit})"
  [ "$rss" -le "$kube_limit" ] || fail "server RSS ${rss} MiB over ${kube_limit} MiB"
fi
```

Use the names the script already uses for its cookie jar, CSRF token, base URL, server PID, `peak_rss` and `fail`. Rename the variables in this snippet to match, so it reads like the rest of the script. In `Makefile`, `rss-bench` passes `KUBE_IT="$(KUBE_IT_RSS)"`, with `KUBE_IT_RSS ?=` empty by default.

- [ ] **Step 4: Makefile and CI**

`Makefile`:

```make
e2e-kube: build kind-up
	cargo build --release -p fleetops-agent
	pnpm -C web exec playwright install chromium
	FLEETOPS_KIND=1 FLEETOPS_KUBE_IT=$(KUBE_IT) FLEETOPS_BIN=$(CURDIR)/target/release/fleetops \
	  pnpm -C web exec playwright test e2e/kube.spec.ts

agent-kube-smoke: kind-up
	scripts/agent-kube-smoke.sh $(IMAGE) $(AGENT_IMAGE)
```

Add both to `.PHONY`. Use the same target-dir and binary-path expressions the `e2e` target uses.

`.github/workflows/ci.yml`, a new job:

```yaml
  kube:
    name: Kubernetes (kind)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: Swatinem/rust-cache@v2
      - uses: helm/kind-action@v1
        with:
          cluster_name: fleetops-test
      - run: make test-kube
      - uses: pnpm/action-setup@v6
        with:
          package_json_file: web/package.json
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: pnpm
          cache-dependency-path: web/pnpm-lock.yaml
      - run: pnpm -C web install --frozen-lockfile
      - run: pnpm -C web exec playwright install --with-deps chromium
      - run: make e2e-kube
      - run: make image IMAGE=fleetops:dev PLATFORMS=linux/amd64 && make image-agent AGENT_IMAGE=fleetops-agent:dev PLATFORMS=linux/amd64
      - run: make agent-kube-smoke
      - run: make rss-bench KUBE_IT_RSS=$PWD/target/kube-it
      - if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: kube-playwright-report
          path: web/playwright-report
```

- [ ] **Step 5: Run the delivery checks**

Run, in order:
```bash
make kind-up
make e2e-kube 2>&1 | tail -15
make image IMAGE=fleetops:dev && make image-agent AGENT_IMAGE=fleetops-agent:dev
make agent-kube-smoke 2>&1 | tail -5
make rss-bench KUBE_IT_RSS=$PWD/target/kube-it 2>&1 | tail -8
```
Expected:
- the kube spec passes;
- the smoke script prints `agent-kube-smoke: ok`;
- the bench prints the server's RSS with kube environments within its limit, and the earlier phases still pass.

Record each result in the ledger. If the kube spec fails at a step that `core-flow` also fails at (the known log-out bug), say so and don't count it against this task.

- [ ] **Step 6: Docs**

`README.md`:
- **Status line:** sub-project 3 (Kubernetes) is implemented; next is sub-project 4, Stacks + GitOps.
- **Roadmap table:**
  - row 3 becomes **Kubernetes (done)**, with its current summary;
  - row 4 becomes Stacks + GitOps;
  - every "sub-project 3" reference to stacks or GitOps becomes "sub-project 4", and "Kubernetes (sub-project 4)" becomes "Kubernetes". Find them with `rg -n "sub-project [34]" README.md`.
- **New section "Kubernetes clusters":**
  - The two ways in: a kubeconfig (token or client certificate, inline data, no exec plugins), or the in-cluster agent applied with the `kubectl` command from Administration › Environments.
  - What the agent's ClusterRole grants. The read-only variant: delete the `delete` and `patch` verbs, the `pods/exec` rule and the `*/scale` rule from the ClusterRole before applying.
  - Secret values are never shown.
  - The secret key: `FLEETOPS_SECRET_KEY`, or `<data>/secret.key`. Back it up with `fleetops.db`; without it, stored kubeconfigs cannot be read and must be re-entered.
  - Lists refresh every 5 s. Watch streams come later.
- **Tests section:** add `make test-kube`, `make e2e-kube` and `make agent-kube-smoke`, all needing `kind`.

`docs/superpowers/specs/2026-10-01-fleetops-kubernetes-design.md`:
- **Status:** Implemented.
- §4.1 and §5: the contexts endpoint is `POST /kube/contexts`.
- §5: drop the `GET /namespaces` row. The namespace picker lists the `namespaces` kind.
- §2.5:
  - the key Secret is chosen with `--key-secret <name>` (the manifest passes `fleetops-agent-key`);
  - its Role grants `get` and `patch` on that name, and `create` on Secrets.
- §8: the agent may delete Secrets, like any listed kind, but it creates and updates only its own key Secret.

- [ ] **Step 7: Final full check and commit**

Run: `make lint && make test 2>&1 | tail -3 && make test-web 2>&1 | tail -3 && make test-docker 2>&1 | tail -3 && make test-kube 2>&1 | grep "test result"`
Expected: green everywhere.

```bash
git add -A web/e2e scripts Makefile .github README.md docs/superpowers/specs
git commit -m "test(kube): e2e, in-cluster smoke, footprint and CI; docs for Kubernetes clusters

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01XJT1vDpqzWR9KhegaGrAw3"
```

---

## Rulings Made While Planning

These deviate from, or sharpen, the spec. Task 17 folds them back into it. Copy each into the ledger before Task 1.

- **Contexts endpoint path:** `POST /kube/contexts`, not `/envs/kube/contexts`. The latter collides with `/envs/{env}/…` for an environment named `kube`. Cost if wrong: one path rename.
- **No names-only namespaces route:** `GET /envs/{env}/k8s/namespaces` is the list route for the `namespaces` kind, so the picker reads row names. Cost if wrong: one extra endpoint later.
- **Where the agent keeps its key:** in a Secret only when `--key-secret <name>` is passed. Inferring that from "`--kube` without `--data`" is impossible, because `--data` has a default. The key's Role grants `get` and `patch` on that name, plus `create` on Secrets. Cost if wrong: the manifest changes.
- **Composite factory:** the server's factory is a composite in `fleetops-server` (`Factory { docker, kube }`). `fleetops-docker` does not delegate to `fleetops-kube`, which keeps the two crates independent. Cost if wrong: none for users.
- **Stores own the cipher:** `Store::with_secret_key`, and the store returns decrypted `Credential`s. The API never sees key material. Cost if wrong: an internal refactor.
- **Version floors:** `k8s-openapi` 0.28 supports Kubernetes 1.32 at the oldest, so the minimum supported Kubernetes is 1.32 (the spec said 1.28). The workspace `rust-version` moves to 1.89 for kube 4.2.
- **Roles, logs and exec:** these were already corrected in the spec before planning:
  - roles are admin/user, with `KubeRead`/`KubeWrite`;
  - logs and exec run over the existing `/ws` topics;
  - Kubernetes status is polled by the web every 15 s;
  - YAML is shown in a plain `<pre>`.
