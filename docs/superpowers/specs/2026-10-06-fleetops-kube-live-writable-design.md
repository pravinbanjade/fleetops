# FleetOps — Sub-project 3b: Kubernetes live and writable

- **Date:** 2026-10-06
- **Status:** Draft
- **Builds on:** [Sub-project 3 spec](./2026-10-01-fleetops-kubernetes-design.md), [Sub-project 2 spec](./2026-10-01-fleetops-agent-multihost-design.md)

## 1. Intent

Sub-project 3 shipped a read-mostly Kubernetes slice: lists poll, objects are read-only YAML, and the only writes are scale, restart and delete. This slice makes Kubernetes environments **live** (lists and detail pages update from watch streams, as Docker pages do from events) and **writable** (create from a template, edit an object's YAML in place, and apply arbitrary YAML), each write previewed as a diff before it happens.

Kubernetes is finished in slices before Stacks + GitOps (sub-project 4):

| Slice | Scope |
|---|---|
| **3b** (this spec) | Watch streams; create, edit and apply YAML |
| 3c | RBAC: per-environment roles for Docker and Kubernetes environments, plus namespace scoping inside clusters. Replaces the RBAC part of sub-project 5 |
| 3d | Helm |
| later | Exec and auth-provider kubeconfig users (EKS, GKE, AKS), CRD browsing, metrics, port-forward, multi-cluster views, `secret.key` rotation |

### Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Watch model | Snapshot plus deltas over a new websocket topic; the web patches its cache. Watches are shared per (env, kind, ns) and stop when unwatched. No server-side reflector cache. |
| Write model | Server-side apply (`fieldManager=fleetops`) for create and apply; replace with the opened `resourceVersion` for in-place edits. Every write is dry-run first and shown as a diff. |
| Kinds for apply | Any kind the cluster serves, resolved through discovery. Browsing stays on the existing 12 kinds. |
| Create UI | YAML only, from per-kind starter templates. No forms. |
| Editor | CodeMirror 6 with YAML mode, loaded lazily. |

### Success criteria

- A change made with `kubectl` appears in an open FleetOps list or detail page within 2 s, through both a kubeconfig and an agent environment.
- Lists of Kubernetes objects no longer poll.
- Create, edit and apply each show a dry-run diff before writing; nothing is written without confirmation.
- An edit made against a stale `resourceVersion` is refused with the live YAML, never silently overwritten.
- No Secret value ever reaches the browser, a log line or an audit row, and saving a Secret with unchanged masked values keeps the real values.
- Server idle RSS is unchanged with no open watches; with 5 kubeconfig environments × 3 open watches it stays within the sub-project 3 gate + 5 MiB.
- Every existing Docker and Kubernetes test passes.

## 2. Architecture

### 2.1 Layout changes

```
crates/
  fleetops-core/   kube.rs: WatchEvent, EditableObject, ApplyMode, ApplyResult; KubeOps gains watch, get_editable, apply, replace
  fleetops-kube/   + watch.rs (watcher → WatchEvent), + apply.rs (split, discovery, SSA, replace), + templates/ (embedded YAML)
                   mask.rs gains unmask(); enrollment manifest gains an access level
  fleetops-api/    + ws/watches.rs (shared watch registry); ws topic kube-watch; routes for edit, apply, replace, templates
web/src/
  live/kubeWatch.ts           subscribe, snapshot → cache, deltas → patch
  components/yaml-editor/     lazy CodeMirror editor, review/diff/apply flow
```

The agent binary does not change.

### 2.2 `fleetops-core::kube`

```rust
/// Serialised as `{"type": "snapshot", "rows": […]}` and so on. A watch
/// that fails for good ends its stream with `Err`.
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WatchEvent {
    Snapshot { rows: Vec<KubeRow> },
    Added { row: KubeRow },
    Modified { row: KubeRow },
    Deleted { namespace: Option<String>, name: String },
    /// The upstream watch restarted; a Snapshot follows.
    Resync,
}

pub struct EditableObject {
    pub yaml: String,             // managedFields and status removed; Secret values masked
    pub resource_version: String,
}

pub enum ApplyMode { DryRun, Commit { force: bool } }

pub enum ApplyAction { Created, Configured, Unchanged }

pub struct ApplyResult {
    pub index: usize,             // document position in the submitted YAML
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub action: Option<ApplyAction>, // None when the document failed
    pub before: Option<String>,   // cleaned, masked live YAML; None when created
    pub after: Option<String>,    // cleaned, masked result YAML
    pub error: Option<ApplyError>,
}

pub struct ApplyError {
    pub code: String,             // an ErrorBody code: conflict, forbidden, invalid, not_found…
    pub message: String,          // Kubernetes' message, verbatim
}

#[async_trait]
pub trait KubeOps {
    // existing methods unchanged
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent>;
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject>;
    async fn apply(&self, yaml: &str, default_ns: &str, mode: ApplyMode) -> Result<Vec<ApplyResult>>;
    async fn replace(&self, kind: KubeKind, ns: &str, name: &str, yaml: &str,
                     resource_version: &str, mode: ApplyMode) -> Result<ApplyResult>;
}
```

`apply` returns `Err` only when nothing could be attempted (YAML that does not parse, an empty document set, the cluster unreachable). Per-document failures are reported in `error`; a field-manager conflict has code `conflict`. `replace` returns `Error::Conflict` when the `resourceVersion` is stale.

### 2.3 `fleetops-kube`

**`watch.rs`.** Wraps kube-rs `runtime::watcher` for the kind's typed API. The initial list becomes `Snapshot`, built with the existing `summaries.rs` row mapping. Apply events become `Added` or `Modified` (an object first seen after the snapshot is `Added`), delete events `Deleted`. The watcher resumes from the last `resourceVersion`; on 410 Gone or a re-list it emits `Resync` and then a new `Snapshot`. Transient errors back off (1 s doubling to 30 s) and resume; after 5 consecutive failures, or on 401/403, it emits `Error` with Kubernetes' message and ends.

**`apply.rs`.**

1. Parse the input as multi-document YAML; empty documents are skipped. Each document must have `apiVersion`, `kind` and `metadata.name`.
2. Resolve each `apiVersion/kind` through discovery (`kube::discovery`), cached per environment for 5 minutes and refreshed once on a miss. An unknown kind yields a per-document error naming `apiVersion/kind`.
3. Namespaced kinds without `metadata.namespace` take `default_ns`; cluster-scoped kinds ignore it.
4. For each document in order: server-side apply via `Api<DynamicObject>::patch` with `Patch::Apply`, `fieldManager=fleetops`, `force` from the mode, and `dryRun=All` for `DryRun`. The live object is fetched first to fill `before` and to restore masked Secret values.
5. `action` is `Created` when there was no live object, `Unchanged` when the cleaned result equals the cleaned live object, and `Configured` otherwise.

Documents are applied in submission order with no rollback, as `kubectl apply` does.

**`replace`.** Parses the single document, requires its `kind`, namespace and name to match the path, sets `metadata.resourceVersion` to the opened version, restores masked Secret values, and sends a PUT (`dryRun=All` for `DryRun`). A 409 from the apiserver maps to `Error::Conflict("changed since you opened it")`.

**Cleaning.** One function used by `get_editable` and by both write paths: removes `metadata.managedFields`, `status`, and `metadata.annotations["kubectl.kubernetes.io/last-applied-configuration"]`, then masks Secret values with the existing `mask.rs`.

**`mask.rs::unmask(edited, live)`.** For a Secret, any `data` or `stringData` value still equal to the mask sentinel is replaced with the live value for the same key; a sentinel with no live counterpart is a per-document error ("masked value for key K has no current value"). Changed values pass through. Non-Secret documents are untouched.

**Templates.** `templates/*.yaml`, embedded with `include_str!`, one per kind: Deployment, StatefulSet, DaemonSet, Job, CronJob, Pod, Service, Ingress, ConfigMap, Secret, Namespace. Each uses a `NAMESPACE` placeholder filled by the web with the current namespace.

### 2.4 Shared watch registry (`fleetops-api::ws::watches`)

- Keyed by `(env_id, kind, ns)` where `ns` is `None` for all namespaces or cluster-scoped kinds.
- The first subscriber starts one upstream `watch` stream; its events go to a `tokio::sync::broadcast` channel (capacity 256).
- The registry keeps the current rows (snapshot with deltas folded in), so a later subscriber receives a `Snapshot` of current state at once and then live deltas.
- A subscriber that lags the broadcast channel is sent a fresh `Snapshot` of the folded state instead of the missed deltas.
- When the last subscriber leaves, the watch stops after 30 s unless someone subscribes again.
- An environment being deleted or going offline ends its watches with `Error`.

### 2.5 Agent access levels

The kube-agent enrollment manifest takes an access level, chosen when the cluster is added (`POST /envs` gains `access: "read-only" | "standard" | "cluster-admin"` for `kube-agent`, default `standard`, and rotation keeps the stored level):

| Level | ClusterRole |
|---|---|
| `read-only` | `get`, `list`, `watch` on the 12 kinds and events; `get` on `pods/log`. Today's README variant, now generated. |
| `standard` | Today's role plus `create`, `update`, `patch` on the 12 kinds. |
| `cluster-admin` | The `standard` role plus a second ClusterRoleBinding, `fleetops-agent-cluster-admin`, to the built-in `cluster-admin`. Needed to apply other kinds (CRDs, RBAC objects). |

A binding's `roleRef` cannot change, so the extra binding has its own name: raising the level is a re-apply; lowering it from `cluster-admin` also needs `kubectl delete clusterrolebinding fleetops-agent-cluster-admin`, which the lower manifests say in a comment. All levels keep the agent's existing grants on its own key Secret. Existing agents keep their role until their manifest is re-applied. Kubeconfig environments are limited only by their credentials.

## 3. Data model

`agent_credentials` gains a nullable `access TEXT` column, set for `kube-agent` environments and used to regenerate the manifest on rotation. Existing `kube-agent` rows are backfilled with `standard`. `AgentView` reports it.

## 4. Flows

### 4.1 Live list

1. The web opens a Kubernetes list page and subscribes to `kube-watch` with id `pods/default`.
2. The server authorizes `KubeRead`, attaches to (or starts) the shared watch, and sends `Snapshot`.
3. The web writes the snapshot into the list's query cache and applies each delta by `ns/name`.
4. On websocket reconnect the web resubscribes; the fresh `Snapshot` replaces the cache.

### 4.2 Edit in place

1. "Edit YAML" calls `GET …/{kind}/{ns}/{name}/edit` and opens the editor with the YAML and `resourceVersion`.
2. "Review" sends `PUT` with `dry_run: true`; the editor shows the diff between `before` and `after`.
3. "Apply" sends the same `PUT` with `dry_run: false`.
4. On 409 (`conflict`) the editor fetches the live object with `GET …/edit` and shows the user's text beside the live YAML and offers "Reload and reapply my edit" (fetch the live object, put the user's text back, return to step 2 with the new `resourceVersion`) or "Discard".

### 4.3 Create and apply

1. "Create" on a list opens the editor with that kind's template; "Apply YAML" (environment top bar and command palette) opens it empty. Both submit to `POST /apply`.
2. "Review" posts with `dry_run: true`; the editor shows one diff per document, with created objects shown as additions.
3. "Apply" posts with `dry_run: false`. Results are listed per document; if any failed, the editor stays open with the failures marked and the successes noted as applied.
4. A field-manager conflict names the conflicting managers from Kubernetes' message. "Force apply" re-posts with `force: true` after a second confirmation.

## 5. API

All routes are under `/api/v1/envs/{env}/k8s`. Writes need CSRF, like every mutation.

| Method | Path | Authz | Notes |
|---|---|---|---|
| GET | `/{kind}/{ns}/{name}/edit` | KubeRead | `EditableObject` |
| PUT | `/{kind}/{ns}/{name}` `{yaml, resource_version, dry_run}` | KubeWrite | `ApplyResult`; a stale version is 409 `conflict`; audited `k8s.edit` (success or failure) unless dry run |
| POST | `/apply` `{yaml, namespace, dry_run, force}` | KubeWrite | `ApplyResult[]`, always 200 when attempted; unless dry run, audited per document as `k8s.create` (created), `k8s.apply` (configured, or failed); unchanged documents are not audited |
| GET | `/templates` | KubeRead | `{kind: yaml}` |

The request body for `PUT` and `POST /apply` is limited to 1 MiB.

**Websocket topic `kube-watch`.** Subscribe `{topic: "kube-watch", env, id: "<kind>/<ns>"}` with `ns` `-` for all namespaces or cluster-scoped kinds. Each `data` frame carries one `WatchEvent`: `{type: "snapshot", rows}`, `{type: "added"|"modified", row}`, `{type: "deleted", namespace, name}`, `{type: "resync"}`; failure is the topic's existing `end` frame with reason `error` and Kubernetes' message. Subscribing on a Docker environment ends with `wrong environment kind`.

All new types are in the OpenAPI document; the web client is regenerated.

## 6. Web UI

- **Live lists.** `KUBE_LIST_POLL_MS` polling is removed from Kubernetes list and detail queries. Lists use `kubeWatch` and render rows from the cache. The status query keeps its existing poll.
- **Live detail.** A detail page subscribes to its kind's watch in its namespace and filters by name. On `modified` it refetches the object, and the YAML and events tabs if open. On `deleted` it shows "This object was deleted" with a link back to the list, and keeps the last data visible beneath it as stale.
- **Paused.** When a watch ends with an error, the page shows a banner "Live updates paused — showing data from <time>" with Kubernetes' message and a "Retry" button that resubscribes. Data is never presented as live while paused.
- **Editor.** One modal component, CodeMirror 6 with YAML language, line numbers, and inline YAML parse errors (parsed in the browser before any request). Its states are Edit → Review (diff, read-only, per document) → Applied or Failed. The diff view is a unified line diff of `before`/`after`. Masked values show as the sentinel with a note "Secret values are hidden; leave them unchanged to keep them". CodeMirror and the diff code load only when the editor opens.
- **Entry points.** "Edit YAML" on every object page (except Events); "Create" on every list of the 12 kinds except Nodes; "Apply YAML" in the Kubernetes environment top bar and the command palette.
- **Agent clusters.** The add-cluster form gains the access-level choice with a one-line description of each; `cluster-admin` is marked "full control of the cluster".
- Accessibility: the editor is keyboard operable (Esc asks before discarding unsaved changes; CodeMirror's tab-focus toggle, Ctrl+M or Shift+Alt+M on macOS, lets Tab leave the editor, and the shortcut is shown under it), the diff is readable by screen readers as added/removed lines, and live list updates do not use `aria-live`.

## 7. Error handling

| Case | Behaviour |
|---|---|
| Kubernetes rejects a document (validation, admission webhook, quota, 403) | Its message is shown verbatim for that document. A 403 on an agent environment adds which access level would allow it. |
| Unknown `apiVersion/kind` | Per-document error naming it. |
| Stale `resourceVersion` | 409 with the live object; §4.2 step 4. |
| Field-manager conflict | Per-document error with Kubernetes' message; "Force apply" offered. |
| Masked Secret value with no current value | Per-document error naming the key. |
| YAML parse error | Caught in the browser; the server also rejects it with 400 and the line. |
| Watch cannot resume | `end` with `error`; paused banner. |
| Environment offline | Writes fail with the existing offline error; watches end. |

## 8. Security

- Secret values are never sent to the browser: `get_editable`, `before` and `after` are masked; `unmask` happens only server-side immediately before the write.
- Audit rows record action and `kind/ns/name` only, never YAML.
- Dry runs are not audited and change nothing.
- Discovery and apply act with the environment's own credentials; FleetOps adds no privilege. The `cluster-admin` access level is opt-in and labelled.
- Body size is limited to 1 MiB; documents per apply to 100.

## 9. Testing

- **`kube_it`** against a `kind` cluster, through a kubeconfig and through the agent tunnel:
  - watch: snapshot, then `added`/`modified`/`deleted` in order after changes made with the kube API (a 410 re-list cannot be forced on kind; the fold's `resync` path is unit-tested);
  - apply: multi-document create, re-apply is `unchanged`, dry run changes nothing, a conflicting field manager is refused then succeeds with force;
  - replace: a stale `resourceVersion` gives `Conflict`;
  - Secret round trip: edit a Secret's label leaving masked values, real values preserved;
  - a narrow token is refused creating a ClusterRole, with Kubernetes' 403 message in that document's error.
- **Unit:** the watch fold (snapshot, added vs modified, deleted, re-list → `resync` + snapshot); watch registry (shared upstream, late subscriber snapshot, lag → snapshot, 30 s grace stop); `unmask`; cleaning; document splitting and namespace defaulting.
- **Vitest:** cache patching from each delta type; paused banner; editor states and conflict view.
- **Playwright:** create a Deployment from its template in a `kind` cluster, then `kubectl scale` it and see the list update without a refresh.
- **rss-bench:** kube phase adds 5 kubeconfig environments × 3 open watches; gate is the sub-project 3 figure + 5 MiB.

## 10. Build and delivery

- `kube` gains the `runtime` feature (watcher); CodeMirror 6 (`@codemirror/view`, `state`, `lang-yaml`) and a line-diff library are added to the web as lazy chunks.
- README: the Kubernetes section documents live updates, editing and applying, and the three access levels; the roadmap lists slices 3b–3d.

## 11. Out of scope

Guided create forms, CRD and custom-resource browsing (apply works, browsing does not), server-side reflector caches, `kubectl diff`-style three-way diff, rollback of multi-document applies, Helm (3d), RBAC (3c), exec and auth-provider kubeconfig users, metrics, port-forward, multi-cluster views, revealing Secret values, `secret.key` rotation.

## 12. Risks

- **Watch load on large clusters.** An all-namespaces pod watch on a big cluster is a large snapshot. Mitigation: one shared upstream per key and the existing list row DTO; measured in rss-bench.
- **`Unchanged` detection** compares cleaned objects and may report `Configured` when only server-defaulted fields differ. Acceptable: the diff shows what changed.
- **CodeMirror bundle size** (about 150 KB gzipped). Loaded only when the editor opens.
