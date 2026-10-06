# Sub-project 3b: Kubernetes Live and Writable — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task, inline in this session. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kubernetes lists and object pages update live from shared watch streams, and users can create, edit and apply YAML with a dry-run diff before every write.

**Architecture:** `fleetops-kube` gains a watch fold over kube-rs `watcher` and a server-side-apply/replace path resolved through discovery; `KubeOps` exposes both. `fleetops-api` shares one upstream watch per (env, kind, ns) through a registry and serves it on a new `kube-watch` websocket topic, plus REST routes for edit, replace, apply and templates. The web patches its query cache from watch events and gets one lazily loaded YAML editor (CodeMirror 6) with a review/diff/apply flow. Agent enrollment gains three access levels.

**Tech Stack:** Rust (kube-rs 4.2 with `runtime`, axum 0.8, utoipa, sqlx/SQLite, tokio), React 19 + TanStack Query/Router, CodeMirror 6, `yaml`, `diff`, vitest, Playwright.

**Spec:** `docs/superpowers/specs/2026-10-06-fleetops-kube-live-writable-design.md`

## Global Constraints

- Apply/replace body: YAML at most 1 MiB (`APPLY_MAX_BYTES = 1 << 20`), at most 100 documents (`APPLY_MAX_DOCS`).
- Field manager for every write: `fleetops`.
- Shared watch grace period after the last subscriber leaves: 30 s (`ApiConfig::kube_watch_grace`).
- Watch failure policy: transient errors back off 1 s doubling to 30 s; after 5 consecutive failures, or immediately on 401/403, the stream ends with `Err`.
- Broadcast buffer per shared watch: 256.
- Discovery cache per environment: 5 minutes.
- Secret values never leave the server: every YAML the API returns (`/edit`, `before`, `after`) is cleaned and masked with `MASK` (`••••`).
- Audit actions: `k8s.edit`, `k8s.apply`, `k8s.create`; target `kind/ns/name`; dry runs and unchanged documents are never audited; YAML is never stored in audit rows.
- Access levels: `read-only`, `standard` (default), `cluster-admin`.
- Kubernetes list and events queries no longer poll; env status polling is unchanged.
- CodeMirror, `yaml` and `diff` load only when the editor opens (lazy chunk).
- Docker terminology unchanged; tests find UI by role and name; WCAG 2.2 AA.

## Review Focus

1. **A Secret saved with untouched masked values** must keep its real values (and its `last-applied-configuration` annotation on replace). Pinned in Task 2 (`unmask`, `restore_hidden`) and Task 11 (kube_it round trip).
2. **A slow browser tab on a busy namespace** must not corrupt its list: a lagging subscriber gets a fresh snapshot, never a gap. Pinned in Task 6 (`a_lagging_subscriber_gets_a_snapshot`).
3. **Editing an object someone else just changed** must refuse with 409 and never overwrite. Pinned in Task 8 (`a_stale_resource_version_is_409`) and Task 11 (kube_it).
4. **Multi-document apply where one document fails** must report each document and audit only what was attempted. Pinned in Task 8 (`apply_reports_each_document_and_audits_commits`).
5. **A watch the cluster forbids** must show "paused" with Kubernetes' reason rather than an empty list presented as live. Pinned in Task 4 (fatal 403 ends the stream) and Task 10 (paused banner).

---

## File Structure

```
crates/fleetops-core/src/
  kube.rs            + WatchEvent, EditableObject, ApplyMode, ApplyAction, ApplyError, ApplyResult,
                       constants; KubeOps gains watch, get_editable, apply, replace
  error.rs           + Error::detail()
crates/fleetops-kube/
  Cargo.toml         kube "runtime" feature via workspace
  src/mask.rs        + clean, to_yaml, unmask, restore_hidden
  src/watch.rs       NEW: Fold (events → WatchEvent) and the failure-policy stream
  src/apply.rs       NEW: split multi-document YAML, namespace defaulting
  src/templates.rs   NEW: embedded starter YAML per kind
  src/templates/*.yaml NEW
  src/client.rs      KubeOps::{watch, get_editable, apply, replace}, discovery cache
  tests/kube_it.rs   + watch, apply, replace, Secret, forbidden cases
crates/fleetops-api/
  src/ws/watches.rs  NEW: shared watch registry
  src/ws/hub.rs      Hubs gains kube_watches
  src/ws/protocol.rs Topic::KubeWatch; End doc gains `error`
  src/ws/topics.rs   kube_watch topic
  src/ws/conn.rs     dispatch
  src/state.rs       ApiConfig::kube_watch_grace
  src/routes/kube.rs editable, replace, apply, templates; audit helper; access hints
  src/routes/mod.rs  route registration
  src/agents/mod.rs  KubeAccess; command_for takes access; AgentView.access
  src/agents/manifest.rs rules per level; cluster-admin binding
  src/routes/envs.rs CreateEnv.access; rotate keeps the level
  src/openapi.rs     new schemas
  tests/common/fake_kube.rs watch channels, apply/replace/editable fakes
  tests/ws_kube.rs, tests/kube_routes.rs, tests/kube_envs.rs  new tests
crates/fleetops-store/
  migrations/0004_agent_access.sql NEW
  src/agents.rs      access column
web/src/
  live/kubeWatch.ts           NEW: applyWatchEvent, useKubeWatch
  live/kubeWatch.test.ts      NEW
  api/queries.ts              polling removed
  pages/kube/LivePaused.tsx   NEW
  pages/kube/KubeListPage.tsx live + Create
  pages/kube/KubeObjectPage.tsx live + deleted banner + Edit YAML
  pages/kube/KubeActions.tsx  Edit YAML button
  pages/kube/live.test.tsx    NEW
  components/yaml-editor/CodeMirrorField.tsx NEW
  components/yaml-editor/DiffView.tsx        NEW
  components/yaml-editor/check.ts            NEW
  components/yaml-editor/YamlDialog.tsx      NEW
  components/yaml-editor/applyYaml.ts        NEW: open-from-anywhere event
  components/yaml-editor/YamlDialog.test.tsx NEW
  components/Shell.tsx, components/CommandPalette.tsx  Apply YAML entry points
  pages/admin/EnvironmentsPage.tsx           access level picker
  e2e/kube.spec.ts                           live + create step
scripts/kube-watch-load.mjs, scripts/rss-bench.sh   watch load phase
README.md
```

---

### Task 1: Core types and the extended `KubeOps` trait

**Files:**
- Modify: `crates/fleetops-core/src/kube.rs`
- Modify: `crates/fleetops-core/src/error.rs`
- Modify: `crates/fleetops-kube/src/client.rs` (interim implementations that return an error, removed by Task 5; see Step 5)
- Modify: `crates/fleetops-api/tests/common/fake_kube.rs`

**Interfaces:**
- Produces (in `fleetops_core::kube`):
  ```rust
  pub const APPLY_MAX_BYTES: usize; pub const APPLY_MAX_DOCS: usize; pub const FIELD_MANAGER: &str;
  pub enum WatchEvent { Snapshot { rows: Vec<KubeRow> }, Added { row: KubeRow }, Modified { row: KubeRow },
                        Deleted { namespace: Option<String>, name: String }, Resync }
  pub struct EditableObject { pub yaml: String, pub resource_version: String }
  pub enum ApplyMode { DryRun, Commit { force: bool } }  // .dry_run() -> bool, .force() -> bool
  pub enum ApplyAction { Created, Configured, Unchanged }
  pub struct ApplyError { pub code: String, pub message: String }   // From<&Error>
  pub struct ApplyResult { index, api_version, kind, namespace, name, action: Option<ApplyAction>,
                           before: Option<String>, after: Option<String>, error: Option<ApplyError> }
  trait KubeOps { fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent>;
                  async fn get_editable(&self, kind, ns: &str, name: &str) -> Result<EditableObject>;
                  async fn apply(&self, yaml: &str, default_ns: &str, mode: ApplyMode) -> Result<Vec<ApplyResult>>;
                  async fn replace(&self, kind, ns: &str, name: &str, yaml: &str, resource_version: &str, mode: ApplyMode) -> Result<ApplyResult>; }
  ```
- Produces (in `fleetops_core::Error`): `pub fn detail(&self) -> String` — the message without the variant's prefix.

- [ ] **Step 1: Write the failing tests** (append to the `tests` module of `crates/fleetops-core/src/kube.rs`; create the module if absent)

```rust
#[cfg(test)]
mod watch_and_apply_tests {
    use super::*;
    use serde_json::json;

    fn row(name: &str) -> KubeRow {
        KubeRow {
            name: name.into(),
            namespace: Some("apps".into()),
            created: 1,
            status: "Running".into(),
            health: RowHealth::Ok,
            columns: Default::default(),
        }
    }

    #[test]
    fn watch_events_are_tagged_by_type() {
        assert_eq!(
            serde_json::to_value(WatchEvent::Snapshot { rows: vec![row("a")] }).unwrap()["type"],
            "snapshot"
        );
        assert_eq!(
            serde_json::to_value(WatchEvent::Deleted { namespace: Some("apps".into()), name: "a".into() })
                .unwrap(),
            json!({"type": "deleted", "namespace": "apps", "name": "a"})
        );
        assert_eq!(serde_json::to_value(WatchEvent::Resync).unwrap(), json!({"type": "resync"}));
    }

    #[test]
    fn apply_mode_says_whether_it_writes_and_forces() {
        assert!(ApplyMode::DryRun.dry_run() && !ApplyMode::DryRun.force());
        let c = ApplyMode::Commit { force: true };
        assert!(!c.dry_run() && c.force());
    }

    #[test]
    fn an_apply_error_keeps_the_code_and_the_bare_message() {
        let e = ApplyError::from(&Error::Conflict("field manager kubectl owns .spec.replicas".into()));
        assert_eq!(e.code, "conflict");
        assert_eq!(e.message, "field manager kubectl owns .spec.replicas");
    }
}
```

And in `crates/fleetops-core/src/error.rs` tests:

```rust
    #[test]
    fn detail_drops_the_variant_prefix() {
        assert_eq!(Error::NotFound("pods \"x\"".into()).detail(), "pods \"x\"");
        assert_eq!(Error::Forbidden("no".into()).detail(), "no");
        assert_eq!(Error::Upstream { status: 502, message: "boom".into() }.detail(), "boom");
        assert_eq!(Error::RateLimited { retry_after_secs: 3 }.detail(), "too many attempts; retry in 3s");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fleetops-core`
Expected: compile errors: `WatchEvent`, `ApplyMode`, `ApplyError`, `detail` not found.

- [ ] **Step 3: Implement**

In `error.rs`, inside `impl Error`:

```rust
    /// The message without the variant's prefix, for showing next to a
    /// separate `code` (e.g. one document of an apply).
    pub fn detail(&self) -> String {
        match self {
            Error::NotFound(m)
            | Error::Conflict(m)
            | Error::Invalid(m)
            | Error::Unauthorized(m)
            | Error::Forbidden(m)
            | Error::EnvUnavailable(m)
            | Error::Internal(m) => m.clone(),
            Error::Upstream { message, .. } => message.clone(),
            Error::RateLimited { .. } => self.to_string(),
        }
    }
```

In `kube.rs`, after `MASK`:

```rust
/// Largest YAML accepted by apply and replace (spec §8).
pub const APPLY_MAX_BYTES: usize = 1 << 20;
/// Most documents in one apply (spec §8).
pub const APPLY_MAX_DOCS: usize = 100;
/// The field manager every FleetOps write uses.
pub const FIELD_MANAGER: &str = "fleetops";
```

After `KubeExec`:

```rust
/// One change to a watched list. A watch that fails for good ends its
/// stream with `Err` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WatchEvent {
    /// Every row, replacing what the client holds.
    Snapshot { rows: Vec<KubeRow> },
    Added { row: KubeRow },
    Modified { row: KubeRow },
    Deleted { namespace: Option<String>, name: String },
    /// The upstream watch restarted; a `Snapshot` follows.
    Resync,
}

/// An object as the YAML editor opens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EditableObject {
    /// `managedFields`, `status` and the last-applied annotation removed;
    /// Secret values masked.
    pub yaml: String,
    /// Sent back with the edit; a newer version on the server is a conflict.
    pub resource_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    DryRun,
    Commit { force: bool },
}

impl ApplyMode {
    pub fn dry_run(self) -> bool {
        matches!(self, ApplyMode::DryRun)
    }
    pub fn force(self) -> bool {
        matches!(self, ApplyMode::Commit { force: true })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApplyAction {
    Created,
    Configured,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyError {
    /// An `ErrorBody` code: `conflict` for a field-manager conflict,
    /// `forbidden`, `invalid`, `not_found`, `upstream`…
    pub code: String,
    /// Kubernetes' message, verbatim.
    pub message: String,
}

impl From<&Error> for ApplyError {
    fn from(e: &Error) -> Self {
        Self {
            code: e.code().into(),
            message: e.detail(),
        }
    }
}

/// What happened to one document of an apply or edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyResult {
    /// Position among the submitted non-empty documents, from 0.
    pub index: usize,
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    /// `None` when the document failed.
    pub action: Option<ApplyAction>,
    /// The live object before the write, cleaned and masked; `None` when created.
    pub before: Option<String>,
    /// The object as written (or as it would be, for a dry run).
    pub after: Option<String>,
    pub error: Option<ApplyError>,
}
```

Extend the trait (after `exec_start`):

```rust
    /// Rows of `kind`, live: a `Snapshot`, then changes. Ends with `Err`
    /// when the watch cannot continue (spec §2.3).
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent>;
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject>;
    /// Server-side apply of every document in `yaml`, in order. `Err` only
    /// when nothing could be attempted; per-document failures are in the results.
    async fn apply(&self, yaml: &str, default_ns: &str, mode: ApplyMode) -> Result<Vec<ApplyResult>>;
    /// Replaces one object, refusing with `Error::Conflict` when it changed
    /// since `resource_version`.
    async fn replace(
        &self,
        kind: KubeKind,
        ns: &str,
        name: &str,
        yaml: &str,
        resource_version: &str,
        mode: ApplyMode,
    ) -> Result<ApplyResult>;
```

- [ ] **Step 4: Run core tests**

Run: `cargo test -p fleetops-core`
Expected: PASS. (`fleetops-kube` and `fleetops-api` tests do not compile yet; Steps 5–6 fix that.)

- [ ] **Step 5: Keep the workspace compiling**

The real implementations arrive in Tasks 3–5. So that every commit builds, add to `impl KubeOps for KubeClient` in `crates/fleetops-kube/src/client.rs` implementations that return a clear error until then:

```rust
    fn watch(&self, _kind: KubeKind, _ns: Option<&str>) -> BoxStream<WatchEvent> {
        Box::pin(futures_util::stream::once(async {
            Err(Error::Invalid("watching is not available yet".into()))
        }))
    }

    async fn get_editable(&self, _kind: KubeKind, _ns: &str, _name: &str) -> Result<EditableObject> {
        Err(Error::Invalid("editing is not available yet".into()))
    }

    async fn apply(&self, _yaml: &str, _default_ns: &str, _mode: ApplyMode) -> Result<Vec<ApplyResult>> {
        Err(Error::Invalid("apply is not available yet".into()))
    }

    async fn replace(&self, _kind: KubeKind, _ns: &str, _name: &str, _yaml: &str, _rv: &str, _mode: ApplyMode) -> Result<ApplyResult> {
        Err(Error::Invalid("editing is not available yet".into()))
    }
```

(import `ApplyMode, ApplyResult, EditableObject, WatchEvent` in the `use fleetops_core::kube::{…}` line). These are replaced in Tasks 4 and 5; no commit after Task 5 may still contain them.

- [ ] **Step 6: Extend the API test fake**

In `crates/fleetops-api/tests/common/fake_kube.rs` add fields to `FakeKube`:

```rust
    watches: Mutex<Vec<(KubeKind, Option<String>, mpsc::UnboundedSender<Result<WatchEvent>>)>>,
    pub applied: Mutex<Vec<(String, String, ApplyMode)>>,
```

helpers in `impl FakeKube`:

```rust
    /// Sends `ev` to every open watch of `kind`.
    pub fn emit_watch(&self, kind: KubeKind, ev: Result<WatchEvent>) {
        let mut w = self.watches.lock().unwrap();
        w.retain(|(_, _, tx)| !tx.is_closed());
        for (k, _, tx) in w.iter() {
            if *k == kind {
                let _ = tx.send(match &ev {
                    Ok(e) => Ok(e.clone()),
                    Err(e) => Err(Error::Forbidden(e.detail())),
                });
            }
        }
    }
    /// Upstream watches still open.
    pub fn open_watches(&self) -> usize {
        self.watches.lock().unwrap().iter().filter(|(_, _, tx)| !tx.is_closed()).count()
    }
```

and the trait methods:

```rust
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent> {
        self.call(format!("watch {} {}", kind.as_str(), ns.unwrap_or("*")));
        let (tx, rx) = mpsc::unbounded_channel();
        self.watches.lock().unwrap().push((kind, ns.map(str::to_string), tx));
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|v| (v, rx))
        }))
    }
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject> {
        let o = self.find(kind, ns, name)?;
        Ok(EditableObject { yaml: o.yaml, resource_version: "1".into() })
    }
    /// One result per `---`-separated document; a ClusterRole is forbidden,
    /// as on a narrow agent.
    async fn apply(&self, yaml: &str, default_ns: &str, mode: ApplyMode) -> Result<Vec<ApplyResult>> {
        self.applied.lock().unwrap().push(("apply".into(), yaml.into(), mode));
        let field = |doc: &str, key: &str| {
            doc.lines().find_map(|l| l.trim().strip_prefix(key)).unwrap_or("").trim().to_string()
        };
        Ok(yaml
            .split("\n---\n")
            .filter(|d| !d.trim().is_empty())
            .enumerate()
            .map(|(index, doc)| {
                let kind = field(doc, "kind:");
                let forbidden = kind == "ClusterRole";
                ApplyResult {
                    index,
                    api_version: field(doc, "apiVersion:"),
                    namespace: (!forbidden).then(|| default_ns.to_string()),
                    name: field(doc, "name:"),
                    action: (!forbidden).then_some(ApplyAction::Created),
                    before: None,
                    after: (!forbidden).then(|| doc.to_string()),
                    error: forbidden.then(|| ApplyError {
                        code: "forbidden".into(),
                        message: "clusterroles.rbac.authorization.k8s.io is forbidden".into(),
                    }),
                    kind,
                }
            })
            .collect())
    }
    async fn replace(&self, kind: KubeKind, ns: &str, name: &str, yaml: &str, rv: &str, mode: ApplyMode) -> Result<ApplyResult> {
        self.find(kind, ns, name)?;
        if rv != "1" {
            return Err(Error::Conflict("changed since you opened it".into()));
        }
        self.applied.lock().unwrap().push((format!("replace {}/{ns}/{name}", kind.as_str()), yaml.into(), mode));
        Ok(ApplyResult {
            index: 0,
            api_version: "v1".into(),
            kind: kind.kind_name().into(),
            namespace: Some(ns.into()),
            name: name.into(),
            action: Some(ApplyAction::Configured),
            before: Some(String::new()),
            after: Some(yaml.into()),
            error: None,
        })
    }
```

- [ ] **Step 7: Build everything**

Run: `cargo test --workspace --no-run && cargo test -p fleetops-core -p fleetops-api`
Expected: builds; all existing tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/fleetops-core crates/fleetops-kube/src/client.rs crates/fleetops-api/tests/common/fake_kube.rs
git commit -m "feat(core): watch events, editable objects and apply results on KubeOps"
```

---

### Task 2: Cleaning, masking and unmasking editable YAML

**Files:**
- Modify: `crates/fleetops-kube/src/mask.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn clean(v: &mut serde_json::Value);                       // strip + mask, Secrets detected by v["kind"]
  pub fn to_yaml(v: &serde_json::Value) -> fleetops_core::Result<String>;   // Invalid when > YAML_CAP
  pub fn unmask(edited: &mut Value, live: Option<&Value>) -> std::result::Result<(), String>;
  pub fn restore_hidden(edited: &mut Value, live: &Value);       // puts back the last-applied annotation
  ```

- [ ] **Step 1: Write the failing tests** (add to `mask.rs` tests)

```rust
    #[test]
    fn clean_strips_server_fields_and_masks_secrets() {
        let mut v = json!({"apiVersion": "v1", "kind": "Secret",
            "metadata": {"name": "s", "managedFields": [{}], "annotations": {
                "kubectl.kubernetes.io/last-applied-configuration": "{\"data\":{\"p\":\"x\"}}", "team": "a"}},
            "status": {"x": 1}, "data": {"p": "aHVudGVyMg=="}});
        clean(&mut v);
        assert!(v["metadata"].get("managedFields").is_none());
        assert!(v.get("status").is_none());
        assert!(v["metadata"]["annotations"].get("kubectl.kubernetes.io/last-applied-configuration").is_none());
        assert_eq!(v["metadata"]["annotations"]["team"], "a");
        assert_eq!(v["data"]["p"], MASK);
    }

    #[test]
    fn clean_leaves_non_secret_data_alone() {
        let mut v = json!({"kind": "ConfigMap", "metadata": {"name": "c"}, "data": {"mode": "prod"}});
        clean(&mut v);
        assert_eq!(v["data"]["mode"], "prod");
    }

    #[test]
    fn unmask_restores_untouched_values_and_keeps_changed_ones() {
        let live = json!({"kind": "Secret", "data": {"a": "QQ==", "b": "Qg=="}});
        let mut edited = json!({"kind": "Secret", "data": {"a": MASK, "b": "bmV3"}});
        unmask(&mut edited, Some(&live)).unwrap();
        assert_eq!(edited["data"], json!({"a": "QQ==", "b": "bmV3"}));
    }

    #[test]
    fn a_masked_string_data_value_is_restored_into_data() {
        let live = json!({"kind": "Secret", "data": {"token": "dA=="}});
        let mut edited = json!({"kind": "Secret", "stringData": {"token": MASK, "new": "plain"}});
        unmask(&mut edited, Some(&live)).unwrap();
        assert_eq!(edited["stringData"], json!({"new": "plain"}));
        assert_eq!(edited["data"], json!({"token": "dA=="}));
    }

    #[test]
    fn a_masked_value_without_a_live_counterpart_is_refused() {
        let mut edited = json!({"kind": "Secret", "data": {"ghost": MASK}});
        let e = unmask(&mut edited, None).unwrap_err();
        assert!(e.contains("ghost"), "{e}");
    }

    #[test]
    fn unmask_ignores_other_kinds() {
        let mut edited = json!({"kind": "ConfigMap", "data": {"x": MASK}});
        unmask(&mut edited, None).unwrap();
        assert_eq!(edited["data"]["x"], MASK);
    }

    #[test]
    fn restore_hidden_puts_back_the_last_applied_annotation() {
        let live = json!({"metadata": {"annotations": {LAST_APPLIED: "{}"}}});
        let mut edited = json!({"metadata": {"name": "x"}});
        restore_hidden(&mut edited, &live);
        assert_eq!(edited["metadata"]["annotations"][LAST_APPLIED], "{}");
    }

    #[test]
    fn an_object_too_large_to_edit_is_refused() {
        let v = json!({"data": {"big": "x".repeat(YAML_CAP + 1)}});
        assert!(matches!(to_yaml(&v), Err(fleetops_core::Error::Invalid(_))));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleetops-kube --lib mask`
Expected: FAIL — `clean`, `unmask`, `restore_hidden`, `to_yaml` not found.

- [ ] **Step 3: Implement** (add to `mask.rs`)

```rust
fn is_secret(v: &Value) -> bool {
    v.get("kind").and_then(Value::as_str) == Some("Secret")
}

fn mask_values(v: &mut Value) {
    for field in ["data", "stringData"] {
        if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
            for val in map.values_mut() {
                *val = MASK.into();
            }
        }
    }
}

/// The object as the editor shows it: no `managedFields`, `status` or
/// last-applied annotation, and a Secret's values masked.
pub fn clean(v: &mut Value) {
    if let Some(meta) = v.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
        if let Some(ann) = meta.get_mut("annotations").and_then(Value::as_object_mut) {
            ann.remove(LAST_APPLIED);
            if ann.is_empty() {
                meta.remove("annotations");
            }
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("status");
    }
    if is_secret(v) {
        mask_values(v);
    }
}

/// YAML for the editor. Unlike the page view it is never cut: a cut
/// document saved back would destroy the object.
pub fn to_yaml(v: &Value) -> fleetops_core::Result<String> {
    let text = serde_saphyr::to_string(v)
        .map_err(|e| fleetops_core::Error::Internal(format!("cannot render YAML: {e}")))?;
    if text.len() > YAML_CAP {
        return Err(fleetops_core::Error::Invalid(
            "this object is larger than 1 MiB and cannot be edited here".into(),
        ));
    }
    Ok(text)
}

/// Puts a Secret's real values back wherever the edit left the mask
/// (spec §2.3). A masked `stringData` value moves to `data`, since the
/// apiserver only returns `data`. Changed values pass through.
pub fn unmask(edited: &mut Value, live: Option<&Value>) -> Result<(), String> {
    if !is_secret(edited) {
        return Ok(());
    }
    let live_data = live
        .and_then(|l| l.get("data"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut restored = serde_json::Map::new();
    for field in ["data", "stringData"] {
        let Some(map) = edited.get_mut(field).and_then(Value::as_object_mut) else {
            continue;
        };
        let masked: Vec<String> = map
            .iter()
            .filter(|(_, v)| v.as_str() == Some(MASK))
            .map(|(k, _)| k.clone())
            .collect();
        for k in masked {
            let real = live_data
                .get(&k)
                .ok_or_else(|| format!("masked value for key {k} has no current value"))?;
            map.remove(&k);
            restored.insert(k, real.clone());
        }
    }
    if !restored.is_empty() {
        let obj = edited.as_object_mut().expect("a Secret is an object");
        let data = obj.entry("data").or_insert_with(|| Value::Object(Default::default()));
        if !data.is_object() {
            *data = Value::Object(Default::default());
        }
        data.as_object_mut().unwrap().extend(restored);
    }
    Ok(())
}

/// Puts back what `clean` hid and a replace must keep: kubectl's
/// last-applied annotation (a PUT without it would drop it).
pub fn restore_hidden(edited: &mut Value, live: &Value) {
    let Some(applied) = live["metadata"]["annotations"].get(LAST_APPLIED).cloned() else {
        return;
    };
    let Some(meta) = edited.get_mut("metadata").and_then(Value::as_object_mut) else {
        return;
    };
    let ann = meta
        .entry("annotations")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(ann) = ann.as_object_mut() {
        ann.entry(LAST_APPLIED).or_insert(applied);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p fleetops-kube --lib mask`
Expected: PASS (old and new).

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-kube/src/mask.rs
git commit -m "feat(kube): clean, mask and unmask YAML for the editor"
```

---

### Task 3: Splitting and normalising applied YAML

**Files:**
- Create: `crates/fleetops-kube/src/apply.rs`
- Modify: `crates/fleetops-kube/src/lib.rs` (`pub mod apply;`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Doc { pub index: usize, pub value: serde_json::Value, pub api_version: String,
                   pub kind: String, pub name: String, pub namespace: Option<String> }
  pub fn split(yaml: &str) -> fleetops_core::Result<Vec<Doc>>;
  pub fn set_namespace(doc: &mut Doc, namespaced: bool, default_ns: &str);
  pub fn gvk(api_version: &str, kind: &str) -> kube::api::GroupVersionKind;
  impl Doc { pub fn result(&self) -> ApplyResult }   // empty result naming the document
  ```

- [ ] **Step 1: Write the failing tests** (in `apply.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use fleetops_core::Error;

    #[test]
    fn documents_are_split_and_empty_ones_skipped() {
        let y = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\n# only a comment\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: b\n  namespace: x\n";
        let docs = split(y).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!((docs[0].index, docs[0].kind.as_str(), docs[0].name.as_str()), (0, "ConfigMap", "a"));
        assert_eq!((docs[1].index, docs[1].namespace.as_deref()), (1, Some("x")));
    }

    #[test]
    fn a_document_without_a_name_is_refused_with_its_position() {
        let e = split("apiVersion: v1\nkind: ConfigMap\nmetadata: {}\n").unwrap_err();
        assert!(matches!(&e, Error::Invalid(m) if m.contains("document 1") && m.contains("metadata.name")), "{e}");
    }

    #[test]
    fn bad_yaml_names_the_document() {
        let e = split("apiVersion: v1\nkind: [\n").unwrap_err();
        assert!(matches!(&e, Error::Invalid(m) if m.starts_with("document 1")), "{e}");
    }

    #[test]
    fn nothing_to_apply_is_an_error() {
        assert!(matches!(split("\n---\n"), Err(Error::Invalid(_))));
    }

    #[test]
    fn more_than_the_cap_is_refused() {
        let one = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n";
        let y = vec![one; APPLY_MAX_DOCS + 1].join("---\n");
        assert!(matches!(split(&y), Err(Error::Invalid(m)) if m.contains("100")));
    }

    #[test]
    fn namespaced_documents_take_the_default_and_cluster_ones_drop_it() {
        let mut d = split("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n").unwrap().remove(0);
        set_namespace(&mut d, true, "apps");
        assert_eq!(d.namespace.as_deref(), Some("apps"));
        assert_eq!(d.value["metadata"]["namespace"], "apps");
        let mut n = split("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: n\n  namespace: x\n").unwrap().remove(0);
        set_namespace(&mut n, false, "apps");
        assert!(n.namespace.is_none() && n.value["metadata"].get("namespace").is_none());
    }

    #[test]
    fn a_core_api_version_has_an_empty_group() {
        let g = gvk("v1", "Pod");
        assert_eq!((g.group.as_str(), g.version.as_str()), ("", "v1"));
        let g = gvk("apps/v1", "Deployment");
        assert_eq!((g.group.as_str(), g.version.as_str(), g.kind.as_str()), ("apps", "v1", "Deployment"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleetops-kube --lib apply`
Expected: FAIL — module empty.

- [ ] **Step 3: Implement**

```rust
//! Multi-document YAML as applied (spec §2.3): split, checked, and given
//! a namespace when the kind needs one.

use fleetops_core::kube::{ApplyResult, APPLY_MAX_DOCS};
use fleetops_core::{Error, Result};
use kube::api::GroupVersionKind;
use serde_json::Value;

pub struct Doc {
    pub index: usize,
    pub value: Value,
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

impl Doc {
    /// A result naming this document, with nothing done yet.
    pub fn result(&self) -> ApplyResult {
        ApplyResult {
            index: self.index,
            api_version: self.api_version.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
            action: None,
            before: None,
            after: None,
            error: None,
        }
    }
}

/// The text of each document. A line of `---` (optionally followed by a
/// space or comment) separates documents.
fn documents(yaml: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in yaml.lines() {
        if line == "---" || line.starts_with("--- ") || line.starts_with("---\t") {
            out.push(String::new());
            continue;
        }
        let cur = out.last_mut().expect("never empty");
        cur.push_str(line);
        cur.push('\n');
    }
    out
}

fn text(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn split(yaml: &str) -> Result<Vec<Doc>> {
    let mut docs = Vec::new();
    for raw in documents(yaml) {
        let n = docs.len() + 1;
        let value: Value = serde_saphyr::from_str(&raw)
            .map_err(|e| Error::Invalid(format!("document {n}: {e}")))?;
        if value.is_null() {
            continue;
        }
        if !value.is_object() {
            return Err(Error::Invalid(format!("document {n} is not a Kubernetes object")));
        }
        let need = |ptr: &str, what: &str| {
            text(&value, ptr).ok_or_else(|| Error::Invalid(format!("document {n} has no {what}")))
        };
        let doc = Doc {
            index: docs.len(),
            api_version: need("/apiVersion", "apiVersion")?,
            kind: need("/kind", "kind")?,
            name: need("/metadata/name", "metadata.name")?,
            namespace: text(&value, "/metadata/namespace"),
            value,
        };
        docs.push(doc);
        if docs.len() > APPLY_MAX_DOCS {
            return Err(Error::Invalid(format!(
                "at most {APPLY_MAX_DOCS} documents can be applied at once"
            )));
        }
    }
    if docs.is_empty() {
        return Err(Error::Invalid("there is nothing to apply".into()));
    }
    Ok(docs)
}

/// Namespaced kinds without a namespace take `default_ns`; cluster-scoped
/// kinds lose any namespace they were given.
pub fn set_namespace(doc: &mut Doc, namespaced: bool, default_ns: &str) {
    let meta = doc.value["metadata"].as_object_mut().expect("checked in split");
    if namespaced {
        let ns = doc.namespace.get_or_insert_with(|| default_ns.to_string());
        meta.insert("namespace".into(), Value::String(ns.clone()));
    } else {
        doc.namespace = None;
        meta.remove("namespace");
    }
}

pub fn gvk(api_version: &str, kind: &str) -> GroupVersionKind {
    let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
    GroupVersionKind::gvk(group, version, kind)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p fleetops-kube --lib apply`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-kube/src/apply.rs crates/fleetops-kube/src/lib.rs
git commit -m "feat(kube): split and normalise multi-document YAML for apply"
```

---

### Task 4: The watch fold and its failure policy

**Files:**
- Modify: `Cargo.toml` (workspace `kube` features add `"runtime"`)
- Create: `crates/fleetops-kube/src/watch.rs`
- Modify: `crates/fleetops-kube/src/lib.rs` (`pub mod watch;`)
- Modify: `crates/fleetops-kube/src/client.rs` (`watch` implemented)

**Interfaces:**
- Consumes: `summaries::row(kind, &DynamicObject) -> KubeRow`, `errors::map`.
- Produces:
  ```rust
  pub struct Fold;  impl Fold { pub fn new(kind: KubeKind) -> Self; pub fn step(&mut self, ev: watcher::Event<DynamicObject>) -> Vec<WatchEvent>; }
  pub fn stream<S>(upstream: S, kind: KubeKind, backoff_start: Duration) -> BoxStream<WatchEvent>
      where S: Stream<Item = Result<watcher::Event<DynamicObject>, watcher::Error>> + Send + 'static;
  pub const MAX_FAILURES: u32 = 5; pub const BACKOFF_START: Duration = 1 s; pub const BACKOFF_MAX: Duration = 30 s;
  ```

> kube-rs note: `kube::runtime::watcher::Event` has the variants `Init`, `InitApply(K)`, `InitDone`, `Apply(K)`, `Delete(K)`, and `watcher::Error` wraps `kube::Error` in `InitialListFailed`, `WatchStartFailed`, `WatchFailed`, plus `WatchError(status)` and `NoResourceVersion`. If kube 4.2 names differ, check `cargo doc -p kube --open` → `runtime::watcher` and adjust `api_code` only; the fold logic does not change.

- [ ] **Step 1: Enable the feature**

In root `Cargo.toml`: `kube = { version = "4.2", default-features = false, features = ["client", "rustls-tls", "ring", "ws", "runtime"] }`. Run `cargo build -p fleetops-kube` — expected: builds.

- [ ] **Step 2: Write the failing tests** (in `watch.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use serde_json::json;

    fn pod(name: &str, phase: &str) -> DynamicObject {
        serde_json::from_value(json!({"apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": name, "namespace": "apps"},
            "status": {"phase": phase}})).unwrap()
    }

    fn names(ev: &WatchEvent) -> Vec<String> {
        match ev {
            WatchEvent::Snapshot { rows } => rows.iter().map(|r| r.name.clone()).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn the_initial_list_becomes_one_snapshot() {
        let mut f = Fold::new(KubeKind::Pods);
        assert!(f.step(Event::Init).is_empty());
        assert!(f.step(Event::InitApply(pod("b", "Running"))).is_empty());
        assert!(f.step(Event::InitApply(pod("a", "Running"))).is_empty());
        let out = f.step(Event::InitDone);
        assert_eq!(out.len(), 1);
        assert_eq!(names(&out[0]), ["a", "b"]);
    }

    #[test]
    fn later_changes_are_added_modified_or_deleted() {
        let mut f = Fold::new(KubeKind::Pods);
        f.step(Event::Init);
        f.step(Event::InitApply(pod("a", "Running")));
        f.step(Event::InitDone);
        assert!(matches!(&f.step(Event::Apply(pod("b", "Pending")))[..], [WatchEvent::Added { row }] if row.name == "b"));
        assert!(matches!(&f.step(Event::Apply(pod("b", "Running")))[..], [WatchEvent::Modified { row }] if row.status == "Running"));
        // The same row again is not news.
        assert!(f.step(Event::Apply(pod("b", "Running"))).is_empty());
        assert!(matches!(&f.step(Event::Delete(pod("a", "Running")))[..],
            [WatchEvent::Deleted { namespace: Some(ns), name }] if ns == "apps" && name == "a"));
    }

    #[test]
    fn a_relist_is_a_resync_then_a_fresh_snapshot() {
        let mut f = Fold::new(KubeKind::Pods);
        f.step(Event::Init);
        f.step(Event::InitApply(pod("a", "Running")));
        f.step(Event::InitDone);
        assert_eq!(f.step(Event::Init), [WatchEvent::Resync]);
        f.step(Event::InitApply(pod("c", "Running")));
        assert_eq!(names(&f.step(Event::InitDone)[0]), ["c"]);
    }

    fn status(code: u16) -> watcher::Error {
        let s: kube::core::Status = serde_json::from_value(json!({
            "status": "Failure", "code": code, "message": "pods is forbidden", "reason": "Forbidden"})).unwrap();
        watcher::Error::InitialListFailed(kube::Error::Api(Box::new(s)))
    }

    #[tokio::test]
    async fn a_forbidden_watch_ends_at_once_with_the_reason() {
        let up = futures_util::stream::iter(vec![Err(status(403))]);
        let mut s = stream(up, KubeKind::Pods, Duration::from_millis(1));
        match s.next().await {
            Some(Err(Error::Forbidden(m))) => assert!(m.contains("forbidden")),
            other => panic!("{other:?}"),
        }
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn transient_failures_are_retried_until_the_fifth() {
        let errs: Vec<_> = (0..MAX_FAILURES).map(|_| Err(watcher::Error::NoResourceVersion)).collect();
        let mut s = stream(futures_util::stream::iter(errs), KubeKind::Pods, Duration::from_millis(1));
        assert!(matches!(s.next().await, Some(Err(Error::Upstream { .. }))));
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn a_success_resets_the_failure_count() {
        let mut items: Vec<Result<Event<DynamicObject>, watcher::Error>> = vec![];
        for _ in 0..MAX_FAILURES - 1 {
            items.push(Err(watcher::Error::NoResourceVersion));
        }
        items.push(Ok(Event::Init));
        items.push(Ok(Event::InitDone));
        for _ in 0..MAX_FAILURES - 1 {
            items.push(Err(watcher::Error::NoResourceVersion));
        }
        let s = stream(futures_util::stream::iter(items), KubeKind::Pods, Duration::from_millis(1));
        let out: Vec<_> = s.collect().await;
        assert_eq!(out.len(), 1, "only the snapshot; the upstream ended before a 5th failure");
        assert!(matches!(out[0], Ok(WatchEvent::Snapshot { .. })));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p fleetops-kube --lib watch`
Expected: FAIL — `Fold`, `stream` not found.

- [ ] **Step 4: Implement `watch.rs`**

```rust
//! Live lists (spec §2.3): kube-rs `watcher` events folded into the rows
//! FleetOps shows, with the failure policy on top.

use crate::{errors, summaries};
use fleetops_core::kube::{KubeKind, KubeRow, WatchEvent};
use fleetops_core::streams::BoxStream;
use fleetops_core::Error;
use futures_util::{Stream, StreamExt};
use kube::api::DynamicObject;
use kube::runtime::watcher::{self, Event};
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

pub const MAX_FAILURES: u32 = 5;
pub const BACKOFF_START: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

type Key = (Option<String>, String);

fn key(r: &KubeRow) -> Key {
    (r.namespace.clone(), r.name.clone())
}

/// The rows of one watched list, kept to tell `Added` from `Modified` and
/// to build snapshots.
pub struct Fold {
    kind: KubeKind,
    rows: BTreeMap<Key, KubeRow>,
    /// Rows of a list in progress (between `Init` and `InitDone`).
    listing: Option<BTreeMap<Key, KubeRow>>,
    /// A snapshot has gone out, so a new list is a resync.
    synced: bool,
}

impl Fold {
    pub fn new(kind: KubeKind) -> Self {
        Self { kind, rows: BTreeMap::new(), listing: None, synced: false }
    }

    pub fn step(&mut self, ev: Event<DynamicObject>) -> Vec<WatchEvent> {
        match ev {
            Event::Init => {
                self.listing = Some(BTreeMap::new());
                if self.synced { vec![WatchEvent::Resync] } else { vec![] }
            }
            Event::InitApply(o) => {
                let r = summaries::row(self.kind, &o);
                if let Some(l) = self.listing.as_mut() {
                    l.insert(key(&r), r);
                }
                vec![]
            }
            Event::InitDone => {
                self.rows = self.listing.take().unwrap_or_default();
                self.synced = true;
                vec![WatchEvent::Snapshot { rows: self.rows.values().cloned().collect() }]
            }
            Event::Apply(o) => {
                let r = summaries::row(self.kind, &o);
                match self.rows.insert(key(&r), r.clone()) {
                    Some(old) if old == r => vec![],
                    Some(_) => vec![WatchEvent::Modified { row: r }],
                    None => vec![WatchEvent::Added { row: r }],
                }
            }
            Event::Delete(o) => {
                let r = summaries::row(self.kind, &o);
                self.rows.remove(&key(&r));
                vec![WatchEvent::Deleted { namespace: r.namespace, name: r.name }]
            }
        }
    }
}

/// The apiserver status code behind a watcher error, when there is one.
fn api_code(e: &watcher::Error) -> Option<u16> {
    match e {
        watcher::Error::InitialListFailed(kube::Error::Api(s))
        | watcher::Error::WatchStartFailed(kube::Error::Api(s))
        | watcher::Error::WatchFailed(kube::Error::Api(s)) => Some(s.code),
        watcher::Error::WatchError(s) => Some(s.code),
        _ => None,
    }
}

fn to_error(e: watcher::Error) -> Error {
    match e {
        watcher::Error::InitialListFailed(k)
        | watcher::Error::WatchStartFailed(k)
        | watcher::Error::WatchFailed(k) => errors::map(k),
        other => Error::Upstream { status: 502, message: other.to_string() },
    }
}

struct State<S> {
    upstream: S,
    fold: Fold,
    queue: VecDeque<WatchEvent>,
    failures: u32,
    backoff: Duration,
    done: bool,
}

/// `upstream` (a kube-rs watcher) as FleetOps watch events. Transient
/// errors back off and resume; 401/403, or `MAX_FAILURES` in a row, end
/// the stream with the error.
pub fn stream<S>(upstream: S, kind: KubeKind, backoff_start: Duration) -> BoxStream<WatchEvent>
where
    S: Stream<Item = Result<Event<DynamicObject>, watcher::Error>> + Send + Unpin + 'static,
{
    let st = State {
        upstream,
        fold: Fold::new(kind),
        queue: VecDeque::new(),
        failures: 0,
        backoff: backoff_start,
        done: false,
    };
    Box::pin(futures_util::stream::unfold(st, move |mut st| async move {
        loop {
            if let Some(ev) = st.queue.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.done {
                return None;
            }
            match st.upstream.next().await {
                None => return None,
                Some(Ok(ev)) => {
                    st.failures = 0;
                    st.backoff = backoff_start;
                    let out = st.fold.step(ev);
                    st.queue.extend(out);
                }
                Some(Err(e)) => {
                    st.failures += 1;
                    let fatal = matches!(api_code(&e), Some(401 | 403));
                    if fatal || st.failures >= MAX_FAILURES {
                        st.done = true;
                        return Some((Err(to_error(e)), st));
                    }
                    tracing::debug!(error = %e, failures = st.failures, "watch failed; retrying");
                    tokio::time::sleep(st.backoff).await;
                    st.backoff = (st.backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }))
}

/// A live list of `kind` through `api`.
pub fn watch(api: kube::Api<DynamicObject>, kind: KubeKind) -> BoxStream<WatchEvent> {
    stream(watcher::watcher(api, watcher::Config::default()).boxed(), kind, BACKOFF_START)
}
```

(`stream` requires `Unpin`; `futures_util::stream::iter` in tests is `Unpin`, and `.boxed()` makes the watcher `Unpin`.)

- [ ] **Step 5: Wire `KubeClient::watch`**

Replace the Task 1 stub in `client.rs`:

```rust
    fn watch(&self, kind: KubeKind, ns: Option<&str>) -> BoxStream<WatchEvent> {
        crate::watch::watch(self.api(kind, ns), kind)
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p fleetops-kube --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/fleetops-kube
git commit -m "feat(kube): watch streams folded into rows, with resync and a failure policy"
```

---

### Task 5: Apply, replace and editable objects on `KubeClient`

**Files:**
- Modify: `crates/fleetops-kube/src/client.rs`

**Interfaces:**
- Consumes: `apply::{split, set_namespace, gvk, Doc}`, `mask::{clean, to_yaml, unmask, restore_hidden}`, `kinds::typed_json`, `FIELD_MANAGER`.
- Produces: real `get_editable`, `apply`, `replace` on `KubeClient`; private `resolve(&self, api_version, kind) -> Result<(ApiResource, bool)>` with a 5-minute cache.

These paths need a real apiserver; they are tested in Task 11's `kube_it`. This task's check is that it compiles and the unit suite passes.

- [ ] **Step 1: Add the discovery cache**

```rust
/// Discovery answers are reused for this long (spec §2.3).
pub const DISCOVERY_TTL: Duration = Duration::from_secs(300);

type Resolved = (ApiResource, bool);
```

Add to `KubeClient` the field `discovery: Mutex<HashMap<(String, String), (Instant, Resolved)>>` (initialise with `Mutex::new(HashMap::new())` in `new`), and:

```rust
    /// The resource for `apiVersion/kind` and whether it is namespaced.
    async fn resolve(&self, api_version: &str, kind: &str) -> Result<Resolved> {
        let k = (api_version.to_string(), kind.to_string());
        if let Some((at, r)) = self.discovery.lock().unwrap().get(&k) {
            if at.elapsed() < DISCOVERY_TTL {
                return Ok(r.clone());
            }
        }
        let (ar, caps) = timed(kube::discovery::pinned_kind(&self.client, &crate::apply::gvk(api_version, kind)))
            .await
            .map_err(|e| match e {
                Error::NotFound(_) => Error::Invalid(format!("the cluster does not serve {api_version}/{kind}")),
                other => other,
            })?;
        let r = (ar, caps.scope == kube::discovery::Scope::Namespaced);
        self.discovery.lock().unwrap().insert(k, (Instant::now(), r.clone()));
        Ok(r)
    }
```

(imports: `std::collections::HashMap`, `kube::discovery::ApiResource`.)

- [ ] **Step 2: Implement one document's apply**

```rust
fn cleaned(v: serde_json::Value) -> serde_json::Value {
    let mut v = v;
    crate::mask::clean(&mut v);
    v
}

impl KubeClient {
    async fn apply_doc(&self, doc: &mut crate::apply::Doc, default_ns: &str, mode: ApplyMode)
        -> Result<(ApplyAction, Option<String>, String)>
    {
        let (ar, namespaced) = self.resolve(&doc.api_version, &doc.kind).await?;
        crate::apply::set_namespace(doc, namespaced, default_ns);
        let api: Api<DynamicObject> = match &doc.namespace {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        };
        let live = timed(api.get_opt(&doc.name)).await?
            .map(|o| serde_json::to_value(o).unwrap_or_default());
        crate::mask::unmask(&mut doc.value, live.as_ref()).map_err(Error::Invalid)?;
        let mut pp = PatchParams::apply(FIELD_MANAGER);
        pp.dry_run = mode.dry_run();
        pp.force = mode.force();
        let out = timed(api.patch(&doc.name, &pp, &Patch::Apply(&doc.value))).await?;
        let before = live.map(cleaned);
        let after = cleaned(serde_json::to_value(out).unwrap_or_default());
        let action = match &before {
            None => ApplyAction::Created,
            Some(b) if *b == after => ApplyAction::Unchanged,
            Some(_) => ApplyAction::Configured,
        };
        let before = before.map(|b| crate::mask::to_yaml(&b)).transpose()?;
        Ok((action, before, crate::mask::to_yaml(&after)?))
    }
}
```

- [ ] **Step 3: Replace the three Task 1 stubs**

```rust
    async fn get_editable(&self, kind: KubeKind, ns: &str, name: &str) -> Result<EditableObject> {
        let obj = timed(self.api(kind, Some(ns)).get(name)).await?;
        let resource_version = obj.metadata.resource_version.clone().unwrap_or_default();
        let yaml = crate::mask::to_yaml(&cleaned(crate::kinds::typed_json(kind, &obj)))?;
        Ok(EditableObject { yaml, resource_version })
    }

    async fn apply(&self, yaml: &str, default_ns: &str, mode: ApplyMode) -> Result<Vec<ApplyResult>> {
        let docs = crate::apply::split(yaml)?;
        let mut out = Vec::with_capacity(docs.len());
        for mut doc in docs {
            let r = self.apply_doc(&mut doc, default_ns, mode).await;
            let mut res = doc.result();
            match r {
                Ok((action, before, after)) => {
                    res.action = Some(action);
                    res.before = before;
                    res.after = Some(after);
                }
                Err(e) => res.error = Some(ApplyError::from(&e)),
            }
            out.push(res);
        }
        Ok(out)
    }

    async fn replace(&self, kind: KubeKind, ns: &str, name: &str, yaml: &str, resource_version: &str, mode: ApplyMode) -> Result<ApplyResult> {
        let mut docs = crate::apply::split(yaml)?;
        if docs.len() != 1 {
            return Err(Error::Invalid("edit one object at a time".into()));
        }
        let mut doc = docs.remove(0);
        if doc.kind != kind.kind_name() || doc.name != name {
            return Err(Error::Invalid(format!(
                "the YAML names {} {}, but this is {} {name}",
                doc.kind, doc.name, kind.kind_name()
            )));
        }
        let ns_opt = (!kind.cluster_scoped()).then_some(ns);
        if doc.namespace.is_some() && doc.namespace.as_deref() != ns_opt {
            return Err(Error::Invalid("an edit cannot move an object to another namespace".into()));
        }
        crate::apply::set_namespace(&mut doc, !kind.cluster_scoped(), ns);
        let api = self.api(kind, Some(ns));
        let live = serde_json::to_value(timed(api.get(name)).await?).unwrap_or_default();
        crate::mask::unmask(&mut doc.value, Some(&live)).map_err(Error::Invalid)?;
        crate::mask::restore_hidden(&mut doc.value, &live);
        doc.value["metadata"]["resourceVersion"] = resource_version.into();
        let obj: DynamicObject = serde_json::from_value(doc.value.clone())
            .map_err(|e| Error::Invalid(format!("not a Kubernetes object: {e}")))?;
        let pp = PostParams { dry_run: mode.dry_run(), field_manager: Some(FIELD_MANAGER.into()) };
        let out = match timed(api.replace(name, &pp, &obj)).await {
            Err(Error::Conflict(_)) => return Err(Error::Conflict("changed since you opened it".into())),
            r => r?,
        };
        let before = cleaned(live);
        let after = cleaned(serde_json::to_value(out).unwrap_or_default());
        let mut res = doc.result();
        res.action = Some(if before == after { ApplyAction::Unchanged } else { ApplyAction::Configured });
        res.before = Some(crate::mask::to_yaml(&before)?);
        res.after = Some(crate::mask::to_yaml(&after)?);
        Ok(res)
    }
```

(imports: `ApplyAction, ApplyError, FIELD_MANAGER` from `fleetops_core::kube`; `kube::api::PostParams`.)

- [ ] **Step 4: Verify**

Run: `cargo clippy -p fleetops-kube --all-targets -- -D warnings && cargo test -p fleetops-kube --lib`
Expected: clean; PASS. `grep -n "not available yet" crates/fleetops-kube/src/client.rs` → no matches.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-kube/src/client.rs
git commit -m "feat(kube): server-side apply through discovery, replace with resourceVersion"
```

---

### Task 6: Shared watch registry

**Files:**
- Create: `crates/fleetops-api/src/ws/watches.rs`
- Modify: `crates/fleetops-api/src/ws/mod.rs` (`pub mod watches;`)
- Modify: `crates/fleetops-api/src/ws/hub.rs` (`Hubs` gains `pub kube_watches: Arc<watches::Watches>`)
- Modify: `crates/fleetops-api/src/state.rs` (`ApiConfig::kube_watch_grace`, default 30 s)

**Interfaces:**
- Produces:
  ```rust
  pub struct Watches;  // Default
  impl Watches {
      pub fn subscribe(self: &Arc<Self>, env: &Arc<dyn Environment>, ops: Arc<dyn KubeOps>,
                       kind: KubeKind, ns: Option<String>, grace: Duration) -> WatchSub;
      pub fn open(&self) -> usize;
  }
  pub struct WatchSub;
  impl WatchSub { pub async fn next(&mut self) -> Option<Result<WatchEvent, String>>; }
  ```

- [ ] **Step 1: Write the failing tests** (in `watches.rs`, using a local scripted `KubeOps`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use fleetops_core::env::{EnvKind, EnvStatus};
    use fleetops_core::kube::*;
    use fleetops_core::streams::{ExecOptions, LogLine, LogOptions};
    use fleetops_core::{Error, Result};
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct Scripted {
        txs: Mutex<Vec<mpsc::UnboundedSender<Result<WatchEvent>>>>,
    }
    impl Scripted {
        fn send(&self, ev: Result<WatchEvent>) {
            let txs = self.txs.lock().unwrap();
            let tx = txs.last().expect("a watch is open");
            let _ = tx.send(ev);
        }
        fn started(&self) -> usize { self.txs.lock().unwrap().len() }
        fn open(&self) -> usize { self.txs.lock().unwrap().iter().filter(|t| !t.is_closed()).count() }
    }
    #[async_trait]
    impl KubeOps for Scripted {
        fn watch(&self, _: KubeKind, _: Option<&str>) -> BoxStream<WatchEvent> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.txs.lock().unwrap().push(tx);
            Box::pin(futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) }))
        }
        async fn version(&self) -> Result<KubeVersion> { unreachable!() }
        async fn list(&self, _: KubeKind, _: Option<&str>) -> Result<KubeList> { unreachable!() }
        async fn get(&self, _: KubeKind, _: &str, _: &str) -> Result<KubeObject> { unreachable!() }
        async fn events(&self, _: KubeKind, _: &str, _: &str) -> Result<Vec<KubeEvent>> { unreachable!() }
        async fn delete(&self, _: KubeKind, _: &str, _: &str) -> Result<()> { unreachable!() }
        async fn scale(&self, _: KubeKind, _: &str, _: &str, _: u32) -> Result<()> { unreachable!() }
        async fn restart(&self, _: KubeKind, _: &str, _: &str) -> Result<()> { unreachable!() }
        fn logs(&self, _: &str, _: &str, _: &LogOptions) -> BoxStream<LogLine> { unreachable!() }
        async fn exec_start(&self, _: &str, _: &str, _: &ExecOptions) -> Result<KubeExec> { unreachable!() }
        async fn get_editable(&self, _: KubeKind, _: &str, _: &str) -> Result<EditableObject> { unreachable!() }
        async fn apply(&self, _: &str, _: &str, _: ApplyMode) -> Result<Vec<ApplyResult>> { unreachable!() }
        async fn replace(&self, _: KubeKind, _: &str, _: &str, _: &str, _: &str, _: ApplyMode) -> Result<ApplyResult> { unreachable!() }
    }

    struct Env;
    #[async_trait]
    impl Environment for Env {
        fn id(&self) -> i64 { 1 }
        fn name(&self) -> &str { "k" }
        fn kind(&self) -> EnvKind { EnvKind::KubeConfig }
        async fn status(&self) -> EnvStatus { unreachable!() }
        fn docker(&self) -> Option<Arc<dyn fleetops_core::docker::DockerOps>> { None }
    }

    fn row(name: &str) -> KubeRow {
        KubeRow { name: name.into(), namespace: Some("apps".into()), created: 1,
            status: "Running".into(), health: RowHealth::Ok, columns: Default::default() }
    }
    fn snap(names: &[&str]) -> WatchEvent {
        WatchEvent::Snapshot { rows: names.iter().map(|n| row(n)).collect() }
    }
    const GRACE: Duration = Duration::from_millis(50);

    fn setup() -> (Arc<Watches>, Arc<dyn Environment>, Arc<Scripted>) {
        (Arc::new(Watches::default()), Arc::new(Env), Arc::new(Scripted::default()))
    }

    #[tokio::test]
    async fn subscribers_share_one_upstream_and_a_late_one_starts_from_a_snapshot() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(&env, ops.clone(), KubeKind::Pods, Some("apps".into()), GRACE);
        ops.send(Ok(snap(&["x"])));
        assert_eq!(a.next().await, Some(Ok(snap(&["x"]))));
        ops.send(Ok(WatchEvent::Added { row: row("y") }));
        assert!(matches!(a.next().await, Some(Ok(WatchEvent::Added { .. }))));
        let mut b = w.subscribe(&env, ops.clone(), KubeKind::Pods, Some("apps".into()), GRACE);
        assert_eq!(b.next().await, Some(Ok(snap(&["x", "y"]))));
        assert_eq!(ops.started(), 1);
    }

    #[tokio::test]
    async fn a_lagging_subscriber_gets_a_snapshot() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        ops.send(Ok(snap(&[])));
        assert_eq!(a.next().await, Some(Ok(snap(&[]))));
        for i in 0..(FEED_BUFFER + 10) {
            ops.send(Ok(WatchEvent::Added { row: row(&format!("p{i:04}")) }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        match a.next().await {
            Some(Ok(WatchEvent::Snapshot { rows })) => assert_eq!(rows.len(), FEED_BUFFER + 10),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failure_reaches_every_subscriber_and_the_next_subscribe_restarts() {
        let (w, env, ops) = setup();
        let mut a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        ops.send(Err(Error::Forbidden("pods is forbidden".into())));
        assert_eq!(a.next().await, Some(Err("pods is forbidden".into())));
        let _b = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        assert_eq!(ops.started(), 2);
    }

    #[tokio::test]
    async fn the_upstream_stops_after_the_grace_period_unless_someone_returns() {
        let (w, env, ops) = setup();
        let a = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        drop(a);
        let _b = w.subscribe(&env, ops.clone(), KubeKind::Pods, None, GRACE);
        tokio::time::sleep(GRACE * 3).await;
        assert_eq!((ops.started(), ops.open()), (1, 1), "a quick return keeps the watch");
        drop(_b);
        tokio::time::sleep(GRACE * 3).await;
        assert_eq!(ops.open(), 0);
        assert_eq!(w.open(), 0);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleetops-api --lib ws::watches`
Expected: FAIL — module empty.

- [ ] **Step 3: Implement**

```rust
//! Shared Kubernetes watches (spec §2.4): one upstream watch per
//! (environment, kind, namespace), however many subscribers, folded into
//! the current rows so a late or lagging subscriber starts from a snapshot.

use fleetops_core::env::Environment;
use fleetops_core::kube::{KubeKind, KubeOps, KubeRow, WatchEvent};
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};

pub const FEED_BUFFER: usize = 256;

#[derive(Debug, Clone)]
enum Feed {
    Event(WatchEvent),
    End(String),
}

#[derive(Default)]
struct State {
    rows: BTreeMap<(Option<String>, String), KubeRow>,
    /// A snapshot has arrived and no resync is in progress.
    ready: bool,
    ended: Option<String>,
    subscribers: usize,
}

impl State {
    fn snapshot(&self) -> WatchEvent {
        WatchEvent::Snapshot { rows: self.rows.values().cloned().collect() }
    }

    fn apply(&mut self, ev: &WatchEvent) {
        match ev {
            WatchEvent::Snapshot { rows } => {
                self.rows = rows.iter().map(|r| ((r.namespace.clone(), r.name.clone()), r.clone())).collect();
                self.ready = true;
            }
            WatchEvent::Added { row } | WatchEvent::Modified { row } => {
                self.rows.insert((row.namespace.clone(), row.name.clone()), row.clone());
            }
            WatchEvent::Deleted { namespace, name } => {
                self.rows.remove(&(namespace.clone(), name.clone()));
            }
            WatchEvent::Resync => self.ready = false,
        }
    }
}

struct Shared {
    tx: broadcast::Sender<Feed>,
    state: Mutex<State>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Shared {
    fn start(ops: Arc<dyn KubeOps>, kind: KubeKind, ns: Option<String>) -> Arc<Shared> {
        let shared = Arc::new(Shared {
            tx: broadcast::channel(FEED_BUFFER).0,
            state: Mutex::new(State::default()),
            task: Mutex::new(None),
        });
        let s = shared.clone();
        let handle = tokio::spawn(async move {
            let mut stream = ops.watch(kind, ns.as_deref());
            let end = loop {
                match stream.next().await {
                    Some(Ok(ev)) => {
                        // Update and send under one lock, so a subscriber
                        // never sees a snapshot and then an event it already holds.
                        let mut st = s.state.lock().unwrap();
                        st.apply(&ev);
                        let _ = s.tx.send(Feed::Event(ev));
                    }
                    Some(Err(e)) => break e.detail(),
                    None => break "the watch ended".to_string(),
                }
            };
            let mut st = s.state.lock().unwrap();
            st.ended = Some(end.clone());
            let _ = s.tx.send(Feed::End(end));
        });
        *shared.task.lock().unwrap() = Some(handle.abort_handle());
        shared
    }

    fn stop(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }

    fn running(&self) -> bool {
        self.task.lock().unwrap().as_ref().is_some_and(|t| !t.is_finished())
    }
}

type Key = (i64, usize, KubeKind, Option<String>);

#[derive(Default)]
pub struct Watches {
    map: Mutex<HashMap<Key, Arc<Shared>>>,
}

impl Watches {
    pub fn subscribe(
        self: &Arc<Self>,
        env: &Arc<dyn Environment>,
        ops: Arc<dyn KubeOps>,
        kind: KubeKind,
        ns: Option<String>,
        grace: Duration,
    ) -> WatchSub {
        // The instance pointer is part of the key: a re-created environment
        // gets a fresh watch, as Docker event hubs do.
        let key = (env.id(), Arc::as_ptr(env) as *const () as usize, kind, ns.clone());
        let mut map = self.map.lock().unwrap();
        let shared = match map.get(&key) {
            Some(s) if s.state.lock().unwrap().ended.is_none() => s.clone(),
            _ => {
                let s = Shared::start(ops, kind, ns);
                map.insert(key.clone(), s.clone());
                s
            }
        };
        let (pending, rx) = {
            let mut st = shared.state.lock().unwrap();
            st.subscribers += 1;
            (st.ready.then(|| st.snapshot()), shared.tx.subscribe())
        };
        WatchSub { shared, rx, pending, owner: Arc::downgrade(self), key, grace }
    }

    /// Upstream watches running, for tests and metrics.
    pub fn open(&self) -> usize {
        self.map.lock().unwrap().values().filter(|s| s.running()).count()
    }
}

pub struct WatchSub {
    shared: Arc<Shared>,
    rx: broadcast::Receiver<Feed>,
    pending: Option<WatchEvent>,
    owner: Weak<Watches>,
    key: Key,
    grace: Duration,
}

impl WatchSub {
    /// The next event; `Err(message)` once when the watch has failed.
    pub async fn next(&mut self) -> Option<Result<WatchEvent, String>> {
        if let Some(ev) = self.pending.take() {
            return Some(Ok(ev));
        }
        loop {
            match self.rx.recv().await {
                Ok(Feed::Event(ev)) => return Some(Ok(ev)),
                Ok(Feed::End(m)) => return Some(Err(m)),
                Err(RecvError::Lagged(_)) => {
                    let st = self.shared.state.lock().unwrap();
                    self.rx = self.shared.tx.subscribe();
                    if let Some(m) = &st.ended {
                        return Some(Err(m.clone()));
                    }
                    if st.ready {
                        return Some(Ok(st.snapshot()));
                    }
                    // Mid-resync: the coming snapshot is on the new receiver.
                }
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

impl Drop for WatchSub {
    fn drop(&mut self) {
        let left = {
            let mut st = self.shared.state.lock().unwrap();
            st.subscribers -= 1;
            st.subscribers
        };
        if left > 0 {
            return;
        }
        let (owner, key, shared, grace) = (self.owner.clone(), self.key.clone(), self.shared.clone(), self.grace);
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let Some(watches) = owner.upgrade() else { return };
            let mut map = watches.map.lock().unwrap();
            if shared.state.lock().unwrap().subscribers > 0 {
                return;
            }
            if map.get(&key).is_some_and(|s| Arc::ptr_eq(s, &shared)) {
                map.remove(&key);
            }
            shared.stop();
        });
    }
}
```

In `hub.rs` add `pub kube_watches: Arc<crate::ws::watches::Watches>,` to `Hubs` (it derives `Default`; `Arc<Watches>: Default`).

In `state.rs` add to `ApiConfig`:

```rust
    /// How long a Kubernetes watch outlives its last subscriber (spec §2.4).
    pub kube_watch_grace: Duration,
```

with `kube_watch_grace: Duration::from_secs(30)` in `Default`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p fleetops-api --lib ws::watches`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api/src
git commit -m "feat(api): shared Kubernetes watches with snapshots for late and lagging subscribers"
```

---

### Task 7: The `kube-watch` websocket topic

**Files:**
- Modify: `crates/fleetops-api/src/ws/protocol.rs`, `topics.rs`, `conn.rs`
- Modify: `crates/fleetops-api/src/openapi.rs` (register `WatchEvent`)
- Modify: `crates/fleetops-api/tests/common/mod.rs` (`ws_config()` sets `kube_watch_grace: Duration::from_millis(50)`)
- Test: `crates/fleetops-api/tests/ws_kube.rs`

**Interfaces:**
- Consumes: `Watches::subscribe`, `WatchSub::next`, `routes::kube::{kube_for, check_namespace}`.
- Produces: subscribe frame `{op:"sub", topic:"kube-watch", env, id:"<kind>/<ns|->"}`; `data` frames carrying `WatchEvent`; failure ends with `{type:"end", reason:"error", message}`.

- [ ] **Step 1: Write the failing tests** (append to `tests/ws_kube.rs`)

```rust
use fleetops_core::kube::WatchEvent;

async fn watch_sub(ws: &mut Ws, sub: u32, id: &str) {
    ws.send(json!({"op": "sub", "sub": sub, "topic": "kube-watch", "env": "k", "id": id})).await;
}

async fn opened(t: &TestApp, n: usize) {
    for _ in 0..100 {
        if t.fake.kube.open_watches() == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {n} open watches, have {}", t.fake.kube.open_watches());
}

#[tokio::test]
async fn a_kube_watch_streams_a_snapshot_then_changes() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "pods/apps").await;
    opened(&t, 1).await;
    let row = common::fake_kube::row("web-1", Some("apps"));
    t.fake.kube.emit_watch(KubeKind::Pods, Ok(WatchEvent::Snapshot { rows: vec![row.clone()] }));
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"]["type"], "snapshot");
    assert_eq!(m["data"]["rows"][0]["name"], "web-1");
    t.fake.kube.emit_watch(KubeKind::Pods, Ok(WatchEvent::Deleted { namespace: Some("apps".into()), name: "web-1".into() }));
    let m = ws.until(|m| m["type"] == "data").await;
    assert_eq!(m["data"], json!({"type": "deleted", "namespace": "apps", "name": "web-1"}));
    assert!(t.fake.kube.calls().contains(&"watch pods apps".to_string()));
}

#[tokio::test]
async fn two_tabs_share_one_upstream_watch() {
    let (t, admin, mut ws) = kube_ws().await;
    let mut ws2 = Ws::open(t.base.clone(), &admin).await;
    watch_sub(&mut ws, 1, "pods/-").await;
    watch_sub(&mut ws2, 1, "pods/-").await;
    opened(&t, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(t.fake.kube.calls().iter().filter(|c| c.starts_with("watch")).count(), 1);
}

#[tokio::test]
async fn a_failed_watch_ends_with_the_reason() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 4, "secrets/-").await;
    opened(&t, 1).await;
    t.fake.kube.emit_watch(KubeKind::Secrets, Err(fleetops_core::Error::Forbidden("secrets is forbidden".into())));
    let m = ws.until(|m| m["type"] == "end").await;
    assert_eq!((m["sub"].as_u64(), m["reason"].as_str()), (Some(4), Some("error")));
    assert_eq!(m["message"], "secrets is forbidden");
}

#[tokio::test]
async fn a_watch_id_must_be_kind_slash_namespace() {
    let (_t, _admin, mut ws) = kube_ws().await;
    for (sub, bad) in [(1, "pods"), (2, "widgets/apps"), (3, "pods/Not_A_NS")] {
        watch_sub(&mut ws, sub, bad).await;
        let e = ws.until(|m| m["type"] == "error").await;
        assert!(e["code"] == "invalid" || e["code"] == "not_found", "{bad}: {e}");
    }
}

#[tokio::test]
async fn unsubscribing_stops_the_watch_after_the_grace_period() {
    let (t, _admin, mut ws) = kube_ws().await;
    watch_sub(&mut ws, 1, "pods/apps").await;
    opened(&t, 1).await;
    ws.send(json!({"op": "unsub", "sub": 1})).await;
    opened(&t, 0).await;
}
```

(If `TestApp` has no `base` field, use whatever `t.serve()` returned for the first socket; keep it in a variable in `kube_ws`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleetops-api --test ws_kube`
Expected: FAIL — unknown topic `kube-watch` (subscribe frame rejected).

- [ ] **Step 3: Implement**

`protocol.rs`: add `#[serde(rename = "kube-watch")] KubeWatch,` to `Topic`. In `ServerMsg::Data` doc add "a `WatchEvent` (kube-watch)". In `ServerMsg::End` doc list reason `error` (kube-watch, with `message`) and add the field `message: Option<String>,`.

`topics.rs`:

```rust
/// `<kind>/<namespace>`, with `-` for all namespaces or a cluster-scoped kind.
fn watch_id(id: &Option<String>) -> Result<(KubeKind, Option<String>)> {
    let id = required(id, "id")?;
    let (kind, ns) = id
        .split_once('/')
        .ok_or_else(|| Error::Invalid("a watch id is <kind>/<namespace or ->".into()))?;
    let kind: KubeKind = kind.parse()?;
    crate::routes::kube::check_namespace(ns)?;
    Ok((kind, (ns != "-" && !kind.cluster_scoped()).then(|| ns.to_string())))
}

/// A live list of Kubernetes objects: a snapshot, then changes (spec §5).
/// The producer waits for the client; a client that falls behind the shared
/// watch gets a fresh snapshot rather than a gap.
pub(super) async fn kube_watch(c: &Ctx<'_>, env: Option<String>, id: Option<String>) -> Result<Sub> {
    let (s, cu, sub) = (c.s, c.cu, c.sub);
    let (env, ops) = crate::routes::kube::kube_for(s, cu, required(&env, "env")?, Action::KubeRead)?;
    let (kind, ns) = watch_id(&id)?;
    let mut w = s.hubs.kube_watches.subscribe(&env, ops, kind, ns, s.config.kube_watch_grace);
    let q = c.queue(Policy::Block);
    let qq = q.clone();
    let task = tokio::spawn(async move {
        while let Some(item) = w.next().await {
            match item {
                Ok(ev) => {
                    let v = serde_json::to_value(&ev).unwrap_or(Value::Null);
                    if !qq.push_wait(protocol::data(sub, v)).await {
                        return;
                    }
                }
                Err(message) => {
                    return qq.finish(protocol::end(sub, "error", json!({ "message": message })))
                }
            }
        }
        qq.finish(protocol::end(sub, "closed", Value::Null));
    });
    Ok(Sub { queue: q, task: task.abort_handle(), exec: None })
}
```

(import `fleetops_core::kube::KubeKind`.) Update the module doc ("The five subscription topics" → "The subscription topics").

`conn.rs` `start`: `Topic::KubeWatch => topics::kube_watch(&c, env, id).await,`.

`openapi.rs` `components(schemas(...))`: add `fleetops_core::kube::WatchEvent`.

`tests/common/mod.rs` `ws_config()`: add `kube_watch_grace: Duration::from_millis(50),`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p fleetops-api --test ws_kube --test ws --test openapi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-api
git commit -m "feat(api): kube-watch topic over the shared websocket"
```

---

### Task 8: REST routes — edit, replace, apply, templates

**Files:**
- Create: `crates/fleetops-kube/src/templates.rs`, `crates/fleetops-kube/src/templates/{deployments,statefulsets,daemonsets,jobs,cronjobs,pods,services,ingresses,configmaps,secrets,namespaces}.yaml`
- Modify: `crates/fleetops-kube/src/lib.rs` (`pub mod templates;`)
- Modify: `crates/fleetops-api/src/routes/kube.rs`, `routes/mod.rs`, `openapi.rs`
- Test: `crates/fleetops-api/tests/kube_routes.rs`

**Interfaces:**
- Consumes: `KubeOps::{get_editable, apply, replace}`, `APPLY_MAX_BYTES`.
- Produces: `GET /envs/{env}/k8s/{kind}/{ns}/{name}/edit` → `EditableObject`; `PUT /envs/{env}/k8s/{kind}/{ns}/{name}` (`ReplaceBody`) → `ApplyResult`; `POST /envs/{env}/k8s/apply` (`ApplyBody`) → `[ApplyResult]`; `GET /envs/{env}/k8s/templates` → `{kind: yaml}`. Operation ids: `editableKube`, `replaceKube`, `applyKube`, `kubeTemplates`. `pub(crate) fn access_hint(level: Option<&str>) -> Option<&'static str>` (used in Task 9 tests).

- [ ] **Step 1: Write the templates**

`templates.rs`:

```rust
//! Starter YAML for "Create", one per kind (spec §2.3). `NAMESPACE` is
//! replaced by the web with the namespace being viewed.

use fleetops_core::kube::KubeKind;

pub fn get(kind: KubeKind) -> Option<&'static str> {
    Some(match kind {
        KubeKind::Deployments => include_str!("templates/deployments.yaml"),
        KubeKind::StatefulSets => include_str!("templates/statefulsets.yaml"),
        KubeKind::DaemonSets => include_str!("templates/daemonsets.yaml"),
        KubeKind::Jobs => include_str!("templates/jobs.yaml"),
        KubeKind::CronJobs => include_str!("templates/cronjobs.yaml"),
        KubeKind::Pods => include_str!("templates/pods.yaml"),
        KubeKind::Services => include_str!("templates/services.yaml"),
        KubeKind::Ingresses => include_str!("templates/ingresses.yaml"),
        KubeKind::ConfigMaps => include_str!("templates/configmaps.yaml"),
        KubeKind::Secrets => include_str!("templates/secrets.yaml"),
        KubeKind::Namespaces => include_str!("templates/namespaces.yaml"),
        KubeKind::Nodes => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_parses_as_one_object_of_its_kind() {
        for kind in KubeKind::ALL {
            let Some(t) = get(kind) else { continue };
            let docs = crate::apply::split(t).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(docs.len(), 1, "{kind:?}");
            assert_eq!(docs[0].kind, kind.kind_name());
            assert_eq!(kind.cluster_scoped(), !t.contains("namespace: NAMESPACE"), "{kind:?}");
        }
    }
}
```

`deployments.yaml`:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: NAMESPACE
spec:
  replicas: 1
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
        - name: app
          image: nginx:1.27
          ports:
            - containerPort: 80
```

`statefulsets.yaml`:

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: my-db
  namespace: NAMESPACE
spec:
  serviceName: my-db
  replicas: 1
  selector:
    matchLabels:
      app: my-db
  template:
    metadata:
      labels:
        app: my-db
    spec:
      containers:
        - name: db
          image: postgres:17
          env:
            - name: POSTGRES_PASSWORD
              value: change-me
```

`daemonsets.yaml`:

```yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: my-agent
  namespace: NAMESPACE
spec:
  selector:
    matchLabels:
      app: my-agent
  template:
    metadata:
      labels:
        app: my-agent
    spec:
      containers:
        - name: agent
          image: busybox:1.37
          command: ["sh", "-c", "while true; do sleep 3600; done"]
```

`jobs.yaml`:

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: my-job
  namespace: NAMESPACE
spec:
  backoffLimit: 2
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: job
          image: busybox:1.37
          command: ["sh", "-c", "echo hello"]
```

`cronjobs.yaml`:

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: my-cron
  namespace: NAMESPACE
spec:
  schedule: "*/15 * * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers:
            - name: job
              image: busybox:1.37
              command: ["sh", "-c", "date"]
```

`pods.yaml`:

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: my-pod
  namespace: NAMESPACE
spec:
  containers:
    - name: main
      image: busybox:1.37
      command: ["sh", "-c", "while true; do sleep 3600; done"]
```

`services.yaml`:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-app
  namespace: NAMESPACE
spec:
  selector:
    app: my-app
  ports:
    - port: 80
      targetPort: 80
```

`ingresses.yaml`:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-app
  namespace: NAMESPACE
spec:
  rules:
    - host: my-app.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: my-app
                port:
                  number: 80
```

`configmaps.yaml`:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: my-config
  namespace: NAMESPACE
data:
  key: value
```

`secrets.yaml`:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-secret
  namespace: NAMESPACE
type: Opaque
stringData:
  password: change-me
```

`namespaces.yaml`:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: my-namespace
```

Run: `cargo test -p fleetops-kube --lib templates` → PASS.

- [ ] **Step 2: Write the failing route tests** (append to `tests/kube_routes.rs`)

```rust
#[tokio::test]
async fn edit_returns_yaml_and_a_resource_version() {
    let (t, admin) = kube_app().await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/deployments/apps/web/edit", None, Some(&admin))).await;
    assert_eq!(r.status, 200, "{:?}", r.json);
    assert_eq!(r.json["resource_version"], "1");
    assert!(r.json["yaml"].as_str().unwrap().contains("name: web"));
}

#[tokio::test]
async fn a_stale_resource_version_is_409() {
    let (t, admin) = kube_app().await;
    let body = json!({"yaml": "kind: Deployment\n", "resource_version": "0", "dry_run": false});
    let r = t.send(req("PUT", "/api/v1/envs/k/k8s/deployments/apps/web", Some(body), Some(&admin))).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json["code"], "conflict");
    assert!(t.fake.kube.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_edit_is_audited_but_its_dry_run_is_not() {
    let (t, admin) = kube_app().await;
    for dry_run in [true, false] {
        let body = json!({"yaml": "kind: Deployment\n", "resource_version": "1", "dry_run": dry_run});
        let r = t.send(req("PUT", "/api/v1/envs/k/k8s/deployments/apps/web", Some(body), Some(&admin))).await;
        assert_eq!(r.status, 200, "{:?}", r.json);
        assert_eq!(r.json["action"], "configured");
    }
    let audit = t.audit_actions().await;
    assert_eq!(audit.iter().filter(|a| *a == "k8s.edit").count(), 1, "{audit:?}");
}

#[tokio::test]
async fn apply_reports_each_document_and_audits_commits() {
    let (t, admin) = kube_app().await;
    let yaml = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\napiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: r\n";
    let dry = t.send(req("POST", "/api/v1/envs/k/k8s/apply",
        Some(json!({"yaml": yaml, "namespace": "apps", "dry_run": true})), Some(&admin))).await;
    assert_eq!(dry.status, 200, "{:?}", dry.json);
    assert_eq!(t.audit_actions().await.iter().filter(|a| a.starts_with("k8s.")).count(), 0);
    let r = t.send(req("POST", "/api/v1/envs/k/k8s/apply",
        Some(json!({"yaml": yaml, "namespace": "apps"})), Some(&admin))).await;
    let docs = r.json.as_array().unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["action"], "created");
    assert_eq!(docs[1]["error"]["code"], "forbidden");
    let audit = t.audit_actions().await;
    assert!(audit.contains(&"k8s.create".to_string()), "{audit:?}");
    assert!(audit.contains(&"k8s.apply".to_string()), "the failure is audited: {audit:?}");
}

#[tokio::test]
async fn apply_refuses_oversized_yaml_and_a_missing_namespace() {
    let (t, admin) = kube_app().await;
    let big = "x".repeat(fleetops_core::kube::APPLY_MAX_BYTES + 1);
    let r = t.send(req("POST", "/api/v1/envs/k/k8s/apply", Some(json!({"yaml": big, "namespace": "apps"})), Some(&admin))).await;
    assert_eq!(r.status, 400);
    let r = t.send(req("POST", "/api/v1/envs/k/k8s/apply", Some(json!({"yaml": "a: b", "namespace": "-"})), Some(&admin))).await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn templates_are_listed_by_kind() {
    let (t, admin) = kube_app().await;
    let r = t.send(req("GET", "/api/v1/envs/k/k8s/templates", None, Some(&admin))).await;
    assert_eq!(r.status, 200);
    assert!(r.json["deployments"].as_str().unwrap().contains("kind: Deployment"));
    assert!(r.json.get("nodes").is_none());
}

#[tokio::test]
async fn writes_need_csrf() {
    let (t, admin) = kube_app().await;
    let mut rq = req("POST", "/api/v1/envs/k/k8s/apply", Some(json!({"yaml": "a: b", "namespace": "apps"})), Some(&admin));
    rq.headers_mut().remove("x-csrf-token");
    assert_eq!(t.send(rq).await.status, 403);
}
```

(If `TestApp` has no `audit_actions()` helper, add one to `tests/common/mod.rs`: list audit rows through `GET /api/v1/audit` as the admin and return their `action` fields. If the CSRF header name differs, use the one `req()` sets.)

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p fleetops-api --test kube_routes`
Expected: FAIL — 404 "no such API route" for the new paths.

- [ ] **Step 4: Implement the routes** (in `routes/kube.rs`)

Refactor the audit helper so it can record a message without a `Result<()>`:

```rust
async fn audit_with(s: &AppState, cu: &CurrentUser, env_id: i64, action: &str, target: &str, error: Option<&str>) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id: Some(cu.user.id),
        environment_id: Some(env_id),
        action,
        target_type: Some("k8s"),
        target_id: Some(target),
        result: if error.is_none() { "success" } else { "failure" },
        detail: error.map(|e| serde_json::json!({ "error": e })),
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, "failed to write audit log");
    }
}

async fn audit(s: &AppState, cu: &CurrentUser, env_id: i64, action: &str, target: &str, outcome: &Result<()>) {
    let err = outcome.as_ref().err().map(|e| e.to_string());
    audit_with(s, cu, env_id, action, target, err.as_deref()).await;
}
```

Bodies and helpers:

```rust
#[derive(Deserialize, ToSchema)]
pub struct ReplaceBody {
    pub yaml: String,
    /// The `resource_version` the editor opened.
    pub resource_version: String,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ApplyBody {
    /// One or more YAML documents.
    pub yaml: String,
    /// For namespaced documents that name none.
    pub namespace: String,
    #[serde(default)]
    pub dry_run: bool,
    /// Take ownership of fields another manager owns.
    #[serde(default)]
    pub force: bool,
}

fn check_size(yaml: &str) -> Result<()> {
    if yaml.len() > APPLY_MAX_BYTES {
        return Err(Error::Invalid("the YAML is larger than 1 MiB".into()));
    }
    Ok(())
}

fn mode(dry_run: bool, force: bool) -> ApplyMode {
    if dry_run { ApplyMode::DryRun } else { ApplyMode::Commit { force } }
}

/// What an agent's access level would need to allow a forbidden write.
pub(crate) fn access_hint(level: Option<&str>) -> Option<&'static str> {
    match level {
        Some("read-only") => Some("this agent is installed read-only; re-install it at the standard level to change the common kinds, or cluster-admin for any kind"),
        Some("standard") => Some("this agent's standard level covers the 12 common kinds; re-install it at cluster-admin to apply other kinds"),
        _ => None,
    }
}

async fn agent_level(s: &AppState, env: &Arc<dyn Environment>) -> Option<String> {
    if env.kind() != EnvKind::KubeAgent {
        return None;
    }
    s.store.get_agent(env.id()).await.ok().flatten().and_then(|a| a.access)
}

fn with_hint(message: String, hint: Option<&str>) -> String {
    match hint {
        Some(h) => format!("{message} ({h})"),
        None => message,
    }
}

/// The audit target for a document: the plural kind when FleetOps knows it.
fn doc_target(r: &ApplyResult) -> String {
    let kind = KubeKind::from_kind_name(&r.kind)
        .map(|k| k.as_str().to_string())
        .unwrap_or_else(|| r.kind.to_lowercase());
    format!("{kind}/{}/{}", r.namespace.as_deref().unwrap_or("-"), r.name)
}
```

Handlers:

```rust
#[utoipa::path(get, path = "/envs/{env}/k8s/{kind}/{ns}/{name}/edit", operation_id = "editableKube", tag = "kubernetes",
    summary = "An object as YAML for editing, with its resource version", params(ObjectPath),
    responses((status = 200, description = "Cleaned, masked YAML", body = EditableObject)))]
pub async fn editable(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>) -> ApiResult<Json<EditableObject>> {
    let k = p.checked()?;
    let (_, ops) = kube_for(&s, &cu, &p.env, Action::KubeRead)?;
    Ok(Json(ops.get_editable(k, &p.ns, &p.name).await?))
}

#[utoipa::path(put, path = "/envs/{env}/k8s/{kind}/{ns}/{name}", operation_id = "replaceKube", tag = "kubernetes",
    summary = "Replace an object with edited YAML; 409 when it changed since it was opened",
    params(ObjectPath), request_body = ReplaceBody,
    responses((status = 200, description = "What changed (or would, for a dry run)", body = ApplyResult)))]
pub async fn replace(State(s): State<AppState>, cu: CurrentUser, ApiPath(p): ApiPath<ObjectPath>, ApiJson(body): ApiJson<ReplaceBody>) -> ApiResult<Json<ApplyResult>> {
    let k = p.checked()?;
    check_size(&body.yaml)?;
    let (env, ops) = kube_for(&s, &cu, &p.env, Action::KubeWrite)?;
    let r = ops
        .replace(k, &p.ns, &p.name, &body.yaml, &body.resource_version, mode(body.dry_run, false))
        .await;
    let r = match r {
        Err(Error::Forbidden(m)) => {
            let level = agent_level(&s, &env).await;
            Err(Error::Forbidden(with_hint(m, access_hint(level.as_deref()))))
        }
        other => other,
    };
    if !body.dry_run {
        let err = r.as_ref().err().map(|e| e.to_string());
        audit_with(&s, &cu, env.id(), "k8s.edit", &target(k, &p), err.as_deref()).await;
    }
    Ok(Json(r?))
}

#[utoipa::path(post, path = "/envs/{env}/k8s/apply", operation_id = "applyKube", tag = "kubernetes",
    summary = "Server-side apply one or more YAML documents", params(EnvParam), request_body = ApplyBody,
    responses((status = 200, description = "One result per document, in order", body = [ApplyResult])))]
pub async fn apply(State(s): State<AppState>, cu: CurrentUser, ApiPath(env_name): ApiPath<String>, ApiJson(body): ApiJson<ApplyBody>) -> ApiResult<Json<Vec<ApplyResult>>> {
    check_size(&body.yaml)?;
    if body.namespace == "-" {
        return Err(Error::Invalid("pick a namespace for documents that name none".into()).into());
    }
    check_namespace(&body.namespace)?;
    let (env, ops) = kube_for(&s, &cu, &env_name, Action::KubeWrite)?;
    let mut results = ops.apply(&body.yaml, &body.namespace, mode(body.dry_run, body.force)).await?;
    let level = agent_level(&s, &env).await;
    if let Some(hint) = access_hint(level.as_deref()) {
        for e in results.iter_mut().filter_map(|r| r.error.as_mut()).filter(|e| e.code == "forbidden") {
            e.message = with_hint(std::mem::take(&mut e.message), Some(hint));
        }
    }
    if !body.dry_run {
        for r in &results {
            let action = match (r.action, &r.error) {
                (Some(ApplyAction::Unchanged), _) => continue,
                (Some(ApplyAction::Created), _) => "k8s.create",
                _ => "k8s.apply",
            };
            let err = r.error.as_ref().map(|e| e.message.as_str());
            audit_with(&s, &cu, env.id(), action, &doc_target(r), err).await;
        }
    }
    Ok(Json(results))
}

#[utoipa::path(get, path = "/envs/{env}/k8s/templates", operation_id = "kubeTemplates", tag = "kubernetes",
    summary = "Starter YAML for creating each kind", params(EnvParam),
    responses((status = 200, description = "YAML by plural kind; NAMESPACE is a placeholder", body = std::collections::BTreeMap<String, String>)))]
pub async fn templates(State(s): State<AppState>, cu: CurrentUser, ApiPath(env_name): ApiPath<String>) -> ApiResult<Json<std::collections::BTreeMap<String, String>>> {
    kube_for(&s, &cu, &env_name, Action::KubeRead)?;
    Ok(Json(
        KubeKind::ALL
            .into_iter()
            .filter_map(|k| fleetops_kube::templates::get(k).map(|t| (k.as_str().to_string(), t.to_string())))
            .collect(),
    ))
}
```

Imports: `fleetops_core::env::EnvKind` (already), `fleetops_core::kube::{ApplyAction, ApplyMode, ApplyResult, EditableObject, APPLY_MAX_BYTES}`, `crate::openapi::EnvParam`. Note `templates` builds its map without contacting the cluster, but `kube_for` still checks the environment kind and authorization.

`routes/mod.rs` — change and add:

```rust
        .routes(routes!(kube::get, kube::delete, kube::replace))
        .routes(routes!(kube::editable))
        .routes(routes!(kube::apply))
        .routes(routes!(kube::templates))
```

`openapi.rs` schemas: add `EditableObject, ApplyResult, ApplyError, ApplyAction` from `fleetops_core::kube` and `crate::routes::kube::{ReplaceBody, ApplyBody}`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p fleetops-api --test kube_routes --test openapi --test rejections`
Expected: PASS. If `/k8s/templates` or `/k8s/apply` is shadowed by `/k8s/{kind}`, register the static routes before the parameterised `kube::list` line.

- [ ] **Step 6: Commit**

```bash
git add crates/fleetops-kube crates/fleetops-api
git commit -m "feat(api): edit, replace, apply and templates routes for Kubernetes"
```

---

### Task 9: Agent access levels

**Files:**
- Create: `crates/fleetops-store/migrations/0004_agent_access.sql`
- Modify: `crates/fleetops-store/src/agents.rs`
- Modify: `crates/fleetops-api/src/agents/mod.rs`, `agents/manifest.rs`, `routes/envs.rs`, `openapi.rs`
- Test: `crates/fleetops-api/tests/kube_envs.rs`, manifest unit tests

**Interfaces:**
- Produces: `pub enum KubeAccess { ReadOnly, Standard, ClusterAdmin }` (serde kebab-case, `as_str`, `FromStr`, `Default = Standard`) in `fleetops_api::agents`; `manifest::manifest(server, token, access)`, `manifest::command(server, token, access)`, `agents::command_for(kind, server, token, access)`; `CreateEnv.access: Option<KubeAccess>`; `AgentRecord.access: Option<String>`; `AgentView.access: Option<String>`; `Store::create_agent_environment(name, kind, join_hash, expires, now, access: Option<&str>)`.

- [ ] **Step 1: Write the failing tests**

Manifest unit tests (in `manifest.rs`, update the existing calls to pass `KubeAccess::Standard`):

```rust
    fn rules(m: &str) -> &str {
        let start = m.find("kind: ClusterRole\n").unwrap();
        &m[start..m[start..].find("\n---\n").unwrap() + start]
    }

    #[test]
    fn read_only_grants_no_write_verbs_and_no_exec() {
        let m = manifest("https://f", "fj_x", KubeAccess::ReadOnly);
        let r = rules(&m);
        for verb in ["create", "update", "patch", "delete"] {
            assert!(!r.contains(verb), "{verb} in {r}");
        }
        assert!(!r.contains("pods/exec"));
        assert!(!m.contains("fleetops-agent-cluster-admin\n") || m.contains("# "), "only a comment mentions it");
    }

    #[test]
    fn standard_can_create_update_and_patch_the_common_kinds() {
        let r = rules(&manifest("https://f", "fj_x", KubeAccess::Standard)).to_string();
        assert!(r.contains("deployments, statefulsets, daemonsets]\n    verbs: [get, list, watch, create, update, patch, delete]"), "{r}");
        assert!(!r.contains("'*'") && !r.contains("\"*\""));
    }

    #[test]
    fn cluster_admin_adds_a_separately_named_binding() {
        let m = manifest("https://f", "fj_x", KubeAccess::ClusterAdmin);
        assert!(m.contains("name: fleetops-agent-cluster-admin\nroleRef:\n  apiGroup: rbac.authorization.k8s.io\n  kind: ClusterRole\n  name: cluster-admin"), "{m}");
        let s = manifest("https://f", "fj_x", KubeAccess::Standard);
        assert!(s.contains("kubectl delete clusterrolebinding fleetops-agent-cluster-admin"), "the downgrade note");
    }
```

API test (append to `tests/kube_envs.rs`):

```rust
#[tokio::test]
async fn a_kube_agent_keeps_its_access_level_across_rotation() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let mut r = req("POST", "/api/v1/envs", Some(json!({"name": "ka", "kind": "kube-agent", "access": "cluster-admin"})), Some(&admin));
    r.headers_mut().insert("origin", "https://fleet.example".parse().unwrap());
    let created = t.send(r).await;
    assert_eq!(created.status, 201, "{:?}", created.json);
    assert!(created.json["enrollment"]["command"].as_str().unwrap().contains("name: cluster-admin"));
    let list = t.send(req("GET", "/api/v1/envs", None, Some(&admin))).await;
    let ka = list.json.as_array().unwrap().iter().find(|e| e["name"] == "ka").unwrap().clone();
    assert_eq!(ka["agent"]["access"], "cluster-admin");
    let mut rot = req("POST", "/api/v1/envs/ka/agent/rotate", None, Some(&admin));
    rot.headers_mut().insert("origin", "https://fleet.example".parse().unwrap());
    let rotated = t.send(rot).await;
    assert!(rotated.json["command"].as_str().unwrap().contains("name: cluster-admin"));
}

#[tokio::test]
async fn only_a_kube_agent_takes_an_access_level() {
    let t = app().await;
    let admin = t.setup_admin().await;
    let r = t.send(req("POST", "/api/v1/envs",
        Some(json!({"name": "d", "kind": "docker-local", "url": "unix:///x.sock", "access": "standard"})), Some(&admin))).await;
    assert_eq!(r.status, 400);
}
```

And a unit test for `access_hint` in `routes/kube.rs`:

```rust
#[cfg(test)]
mod hint_tests {
    use super::access_hint;

    #[test]
    fn the_hint_names_the_level_that_would_allow_it() {
        assert!(access_hint(Some("read-only")).unwrap().contains("standard"));
        assert!(access_hint(Some("standard")).unwrap().contains("cluster-admin"));
        assert!(access_hint(Some("cluster-admin")).is_none());
        assert!(access_hint(None).is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fleetops-api --lib agents::manifest && cargo test -p fleetops-api --test kube_envs`
Expected: FAIL — `KubeAccess` missing; `access` rejected/ignored.

- [ ] **Step 3: Implement**

Migration `0004_agent_access.sql`:

```sql
-- Sub-project 3b: the access level a kube-agent manifest grants
-- (read-only, standard, cluster-admin); null for Docker agents.
ALTER TABLE agent_credentials ADD COLUMN access TEXT;
UPDATE agent_credentials SET access = 'standard'
  WHERE environment_id IN (SELECT id FROM environments WHERE kind = 'kube-agent');
```

`agents.rs` (store): add `pub access: Option<String>` to `AgentRecord`, `access` to `AGENT_COLS`, and an `access: Option<&str>` parameter to `create_agent_environment`, bound in the `INSERT INTO agent_credentials (environment_id, join_token_hash, join_expires_at, access) VALUES (?, ?, ?, ?)`. Update every caller (`grep -rn create_agent_environment crates`), passing `None` for Docker agents.

`agents/mod.rs`:

```rust
/// What the in-cluster agent's ClusterRole allows (spec §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum KubeAccess {
    ReadOnly,
    #[default]
    Standard,
    ClusterAdmin,
}

impl KubeAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            KubeAccess::ReadOnly => "read-only",
            KubeAccess::Standard => "standard",
            KubeAccess::ClusterAdmin => "cluster-admin",
        }
    }
}

impl std::str::FromStr for KubeAccess {
    type Err = fleetops_core::Error;
    fn from_str(s: &str) -> fleetops_core::Result<Self> {
        match s {
            "read-only" => Ok(KubeAccess::ReadOnly),
            "standard" => Ok(KubeAccess::Standard),
            "cluster-admin" => Ok(KubeAccess::ClusterAdmin),
            _ => Err(fleetops_core::Error::Invalid(format!("unknown access level '{s}'"))),
        }
    }
}
```

`command_for(kind, server_url, join_token, access: KubeAccess)` passes `access` to `manifest::command`. `AgentView` gains `pub access: Option<String>` set from `rec.access`.

`manifest.rs`: `command(server_url, join_token, access)` and `manifest(server_url, join_token, access)`. Replace the fixed ClusterRole `rules:` block with `{rules}` from:

```rust
const READ_ONLY_RULES: &str = r#"  - apiGroups: [""]
    resources: [namespaces, nodes, pods, services, configmaps, secrets, events]
    verbs: [get, list, watch]
  - apiGroups: [""]
    resources: [pods/log]
    verbs: [get]
  - apiGroups: [apps]
    resources: [deployments, statefulsets, daemonsets, replicasets]
    verbs: [get, list, watch]
  - apiGroups: [batch]
    resources: [jobs, cronjobs]
    verbs: [get, list, watch]
  - apiGroups: [networking.k8s.io]
    resources: [ingresses]
    verbs: [get, list, watch]
  - nonResourceURLs: [/version]
    verbs: [get]"#;

const STANDARD_RULES: &str = r#"  - apiGroups: [""]
    resources: [namespaces, nodes]
    verbs: [get, list, watch, create, update, patch]
  - apiGroups: [""]
    resources: [pods, services, configmaps, secrets]
    verbs: [get, list, watch, create, update, patch, delete]
  - apiGroups: [""]
    resources: [events]
    verbs: [get, list, watch]
  - apiGroups: [""]
    resources: [pods/log]
    verbs: [get]
  - apiGroups: [""]
    resources: [pods/exec]
    verbs: [get, create]
  - apiGroups: [apps]
    resources: [deployments, statefulsets, daemonsets]
    verbs: [get, list, watch, create, update, patch, delete]
  - apiGroups: [apps]
    resources: [deployments/scale, statefulsets/scale]
    verbs: [get, patch]
  - apiGroups: [apps]
    resources: [replicasets]
    verbs: [get]
  - apiGroups: [batch]
    resources: [jobs, cronjobs]
    verbs: [get, list, watch, create, update, patch, delete]
  - apiGroups: [networking.k8s.io]
    resources: [ingresses]
    verbs: [get, list, watch, create, update, patch, delete]
  - nonResourceURLs: [/version]
    verbs: [get]"#;
```

with `let rules = if access == KubeAccess::ReadOnly { READ_ONLY_RULES } else { STANDARD_RULES };`. Prepend a header comment: for `ClusterAdmin`, `# Access level: cluster-admin (full control of the cluster).\n`; otherwise `# Access level: {level}. Lowering it from cluster-admin? Also run: kubectl delete clusterrolebinding fleetops-agent-cluster-admin\n`. For `ClusterAdmin`, insert after the `fleetops-agent` ClusterRoleBinding document:

```rust
const ADMIN_BINDING: &str = r#"
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: fleetops-agent-cluster-admin
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cluster-admin
subjects:
  - kind: ServiceAccount
    name: fleetops-agent
    namespace: fleetops"#;
```

Update `the_manifest_installs_a_namespaced_agent_with_its_join_token` to call with `KubeAccess::Standard` (the kind list is unchanged for standard).

`routes/envs.rs`: `CreateEnv` gains

```rust
    /// kube-agent: what the agent's ClusterRole allows; default `standard`.
    #[serde(default)]
    pub access: Option<KubeAccess>,
```

In `create`, before the match: `if body.access.is_some() && body.kind != EnvKind::KubeAgent { return Err(Error::Invalid("only a kube-agent environment takes an access level".into()).into()); }`. In the agent branch: `let access = (body.kind == EnvKind::KubeAgent).then(|| body.access.unwrap_or_default());` pass `access.map(KubeAccess::as_str)` to the store and `access.unwrap_or_default()` to `command_for`. In `new_enrollment` (rotate path) read the level: `let access = s.store.get_agent(env_id).await?.and_then(|a| a.access).map(|a| a.parse()).transpose()?.unwrap_or_default();` and pass it.

`openapi.rs`: add `crate::agents::KubeAccess` to schemas.

- [ ] **Step 4: Run tests**

Run: `cargo test -p fleetops-store && cargo test -p fleetops-api`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleetops-store crates/fleetops-api
git commit -m "feat(agent): read-only, standard and cluster-admin access levels for in-cluster agents"
```

---

### Task 10: Web — live lists and object pages

**Files:**
- Run: `make openapi` (regenerates `crates/fleetops-api/openapi.json` and `web/src/api/gen`)
- Create: `web/src/live/kubeWatch.ts`, `web/src/live/kubeWatch.test.ts`, `web/src/pages/kube/LivePaused.tsx`, `web/src/pages/kube/live.test.tsx`
- Modify: `web/src/api/queries.ts`, `web/src/pages/kube/KubeListPage.tsx`, `web/src/pages/kube/KubeObjectPage.tsx`

**Interfaces:**
- Produces:
  ```ts
  export function applyWatchEvent(list: KubeList | undefined, ev: WatchEvent): KubeList | undefined;
  export interface WatchPause { at: number; message: string }
  export function useKubeWatch(env: string, kind: KubeKind, ns: string | undefined, onEvent?: (ev: WatchEvent) => void): { paused: WatchPause | null; retry(): void };
  export function LivePaused(props: { paused: WatchPause; onRetry(): void }): JSX.Element;
  ```

- [ ] **Step 1: Regenerate the client**

Run: `make openapi`
Expected: `web/src/api/gen/types.gen.ts` contains `WatchEvent`, `ApplyResult`, `EditableObject`, `KubeAccess`; `sdk.gen.ts` has `editableKube`, `replaceKube`, `applyKube`, `kubeTemplates`; `Topic` includes `"kube-watch"`.

- [ ] **Step 2: Write the failing unit tests** (`web/src/live/kubeWatch.test.ts`)

```ts
import { describe, expect, it } from "vitest";
import type { KubeList, KubeRow } from "@/api/gen/types.gen";
import { applyWatchEvent } from "./kubeWatch";

const row = (name: string, status = "Running"): KubeRow => ({ name, namespace: "apps", created: 1, status, health: "ok", columns: {} });

describe("applyWatchEvent", () => {
  const base: KubeList = { rows: [row("a"), row("c")], truncated: true };

  it("replaces everything on a snapshot", () => {
    expect(applyWatchEvent(base, { type: "snapshot", rows: [row("z")] })).toEqual({ rows: [row("z")], truncated: false });
    expect(applyWatchEvent(undefined, { type: "snapshot", rows: [] })).toEqual({ rows: [], truncated: false });
  });

  it("inserts added rows in order and updates modified ones in place", () => {
    const added = applyWatchEvent(base, { type: "added", row: row("b") })!;
    expect(added.rows.map((r) => r.name)).toEqual(["a", "b", "c"]);
    const modified = applyWatchEvent(added, { type: "modified", row: row("b", "Pending") })!;
    expect(modified.rows.find((r) => r.name === "b")!.status).toBe("Pending");
    expect(modified.rows).toHaveLength(3);
  });

  it("removes deleted rows by namespace and name", () => {
    expect(applyWatchEvent(base, { type: "deleted", namespace: "apps", name: "a" })!.rows.map((r) => r.name)).toEqual(["c"]);
    expect(applyWatchEvent(base, { type: "deleted", namespace: "other", name: "a" })!.rows).toHaveLength(2);
  });

  it("leaves the list alone on resync and ignores deltas before a list exists", () => {
    expect(applyWatchEvent(base, { type: "resync" })).toBe(base);
    expect(applyWatchEvent(undefined, { type: "added", row: row("x") })).toBeUndefined();
  });
});
```

Page tests (`web/src/pages/kube/live.test.tsx`):

```tsx
import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { FakeWebSocket } from "@/test/fake-ws";
import { mockFetch } from "@/test/fetch";

async function watchSub(id: string) {
  const sock = FakeWebSocket.latest();
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let frame: { sub: number } | undefined;
  await vi.waitFor(() => {
    frame = sock.frames().find((f) => f.topic === "kube-watch" && f.id === id);
    expect(frame).toBeDefined();
  });
  return { sock, sub: frame!.sub };
}

describe("live Kubernetes pages", () => {
  it("patches a list from its watch without polling", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    const { sock, sub } = await watchSub("deployments/-");
    act(() => sock.receive({ sub, type: "data", data: { type: "added", row: kubeRow("api", "apps", { ready: "1/1" }) } }));
    expect(await within(table).findByRole("link", { name: "api" })).toBeInTheDocument();
    act(() => sock.receive({ sub, type: "data", data: { type: "deleted", namespace: "apps", name: "web" } }));
    await vi.waitFor(() => expect(within(table).queryByRole("link", { name: "web" })).not.toBeInTheDocument());
    expect(calls.filter((c) => c.path.startsWith("/api/v1/envs/k/k8s/deployments")).length).toBe(1);
  });

  it("says when live updates pause, and retries", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    const { sock, sub } = await watchSub("pods/-");
    act(() => sock.receive({ sub, type: "end", reason: "error", message: "pods is forbidden" }));
    const banner = await screen.findByText(/Live updates paused/);
    expect(banner).toHaveTextContent("pods is forbidden");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await vi.waitFor(() => expect(sock.frames().filter((f) => f.topic === "kube-watch").length).toBe(2));
  });

  it("marks an object page when the object is deleted", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/deployments/apps/web": {
        json: { row: kubeRow("web", "apps"), yaml: "kind: Deployment\n", conditions: [], owners: [], containers: [] },
      },
    });
    renderApp("/envs/k/k8s/deployments/apps/web");
    await screen.findByRole("heading", { name: "web" });
    const { sock, sub } = await watchSub("deployments/apps");
    act(() => sock.receive({ sub, type: "data", data: { type: "deleted", namespace: "apps", name: "web" } }));
    expect(await screen.findByText(/This object was deleted/)).toBeInTheDocument();
  });
});
```

(add `import { vi } from "vitest";` to the imports.)

- [ ] **Step 3: Run to verify failure**

Run: `pnpm -C web exec vitest run src/live/kubeWatch.test.ts src/pages/kube/live.test.tsx`
Expected: FAIL — module `./kubeWatch` not found.

- [ ] **Step 4: Implement `kubeWatch.ts`**

```ts
import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { KubeKind, KubeList, KubeRow, WatchEvent } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { useSubscription } from "./LiveProvider";

const same = (r: KubeRow, ns: string | null | undefined, name: string) => (r.namespace ?? null) === (ns ?? null) && r.name === name;
const order = (a: KubeRow, b: KubeRow) => (a.namespace ?? "").localeCompare(b.namespace ?? "") || a.name.localeCompare(b.name);

/** A cached list after one watch event; deltas before any list are dropped (a snapshot follows). */
export function applyWatchEvent(list: KubeList | undefined, ev: WatchEvent): KubeList | undefined {
  switch (ev.type) {
    case "snapshot":
      return { rows: [...ev.rows].sort(order), truncated: false };
    case "resync":
      return list;
    case "added":
    case "modified": {
      if (!list) return list;
      const rows = list.rows.filter((r) => !same(r, ev.row.namespace, ev.row.name));
      rows.push(ev.row);
      return { ...list, rows: rows.sort(order) };
    }
    case "deleted":
      return list && { ...list, rows: list.rows.filter((r) => !same(r, ev.namespace, ev.name)) };
  }
}

/** Why a list stopped being live, and since when its data is from. */
export interface WatchPause {
  at: number;
  message: string;
}

/**
 * Keeps the cached list of `kind` in `ns` (all namespaces when empty) live
 * from the shared `kube-watch` topic. When the watch ends, the page shows
 * the data as paused (spec §6) until `retry`.
 */
export function useKubeWatch(env: string, kind: KubeKind, ns: string | undefined, onEvent?: (ev: WatchEvent) => void) {
  const qc = useQueryClient();
  const [paused, setPaused] = useState<WatchPause | null>(null);
  const [attempt, setAttempt] = useState(0);
  const last = useRef(Date.now());
  const pause = (message: string) => setPaused({ at: last.current, message });
  useSubscription(
    paused ? null : { topic: "kube-watch", env, id: `${kind}/${ns || "-"}`, ...(attempt ? { opts: { attempt } } : {}) },
    {
      onData: (d) => {
        const ev = d as WatchEvent;
        last.current = Date.now();
        qc.setQueryData<KubeList>(keys.kube(env, kind, ns), (old) => applyWatchEvent(old, ev));
        onEvent?.(ev);
      },
      onEnd: (m) => pause((m as { message?: string }).message ?? "the watch ended"),
      onError: (m) => pause(m.message),
    },
  );
  return {
    paused,
    retry: () => {
      setPaused(null);
      setAttempt((n) => n + 1);
    },
  };
}
```

`LivePaused.tsx`:

```tsx
import { Button } from "@/components/ui/button";
import type { WatchPause } from "@/live/kubeWatch";

export function LivePaused({ paused, onRetry }: { paused: WatchPause; onRetry: () => void }) {
  const time = new Date(paused.at).toLocaleTimeString([], { hour12: false });
  return (
    <div role="status" className="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
      <span>
        Live updates paused — showing data from {time}. <span className="text-muted-foreground">{paused.message}</span>
      </span>
      <Button size="sm" variant="outline" onClick={onRetry}>Retry</Button>
    </div>
  );
}
```

- [ ] **Step 5: Wire the pages and drop polling**

`queries.ts`: delete `KUBE_LIST_POLL_MS` and both `refetchInterval: KUBE_LIST_POLL_MS` lines (list and events). `grep -rn KUBE_LIST_POLL_MS web/src` must return nothing.

`KubeListPage.tsx`: after the `list` query:

```tsx
  const live = useKubeWatch(env, kind, scoped ? ns : undefined);
```

and as the first child of `<Page>`: `{live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}`.

`KubeObjectPage.tsx`: add

```tsx
  const qc = useQueryClient();
  const [gone, setGone] = useState(false);
  const cluster = CLUSTER_SCOPED.includes(kind);
  const live = useKubeWatch(env, kind, cluster ? undefined : ns, (ev) => {
    if (ev.type === "snapshot") setGone(!ev.rows.some((r) => r.name === name));
    if ((ev.type === "added" || ev.type === "modified") && ev.row.name === name) {
      setGone(false);
      void qc.invalidateQueries({ queryKey: keys.kubeObject(env, kind, ns, name) });
    }
    if (ev.type === "deleted" && ev.name === name) setGone(true);
  });
```

(the `kubeObject` key is a prefix of the events key, so an open Events tab refetches too). Render, before the owners block:

```tsx
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {gone && (
        <p role="status" className="mb-3 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          This object was deleted.{" "}
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="underline">Back to {KIND_LABEL[kind]}</Link>
        </p>
      )}
```

Imports: `useQueryClient` from `@tanstack/react-query`, `keys` from `@/api/queries`, `CLUSTER_SCOPED` from `./kinds`, `useKubeWatch` from `@/live/kubeWatch`, `LivePaused` from `./LivePaused`.

- [ ] **Step 6: Run tests**

Run: `pnpm -C web exec vitest run && pnpm -C web exec tsc --noEmit && pnpm -C web lint`
Expected: PASS (existing kube tests still pass; they never asserted polling).

- [ ] **Step 7: Commit**

```bash
git add crates/fleetops-api/openapi.json web
git commit -m "feat(web): Kubernetes lists and object pages update live from watches"
```

---

### Task 11: Cluster suite for watch, apply and replace

**Files:**
- Modify: `crates/fleetops-kube/tests/kube_it.rs`

Run against kind with both `make test-kube` passes (direct and tunnel).

- [ ] **Step 1: Write the tests**

```rust
use fleetops_core::kube::{ApplyAction, ApplyMode, WatchEvent};
use futures_util::StreamExt;

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() % 1_000_000_000)
}

#[tokio::test]
async fn a_watch_sees_a_snapshot_then_add_modify_delete() {
    let ops = common::ops();
    let mut w = ops.watch(KubeKind::ConfigMaps, Some(NS));
    let first = timeout(WAIT, w.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(first, WatchEvent::Snapshot { .. }), "{first:?}");
    let name = uniq("watch");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let mut cm = ConfigMap::default();
    cm.metadata.name = Some(name.clone());
    cms.create(&PostParams::default(), &cm).await.unwrap();
    let mut seen = vec![];
    while seen.len() < 1 {
        if let WatchEvent::Added { row } = timeout(WAIT, w.next()).await.unwrap().unwrap().unwrap() {
            if row.name == name { seen.push("added"); }
        }
    }
    cms.delete(&name, &Default::default()).await.unwrap();
    loop {
        match timeout(WAIT, w.next()).await.unwrap().unwrap().unwrap() {
            WatchEvent::Deleted { name: n, .. } if n == name => break,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn apply_creates_then_is_unchanged_and_a_dry_run_writes_nothing() {
    let ops = common::ops();
    let name = uniq("apply");
    let yaml = format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\ndata:\n  a: \"1\"\n");
    let dry = ops.apply(&yaml, NS, ApplyMode::DryRun).await.unwrap();
    assert_eq!(dry[0].action, Some(ApplyAction::Created));
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    assert!(cms.get_opt(&name).await.unwrap().is_none(), "a dry run writes nothing");
    let r = ops.apply(&yaml, NS, ApplyMode::Commit { force: false }).await.unwrap();
    assert_eq!(r[0].action, Some(ApplyAction::Created), "{:?}", r[0].error);
    let again = ops.apply(&yaml, NS, ApplyMode::Commit { force: false }).await.unwrap();
    assert_eq!(again[0].action, Some(ApplyAction::Unchanged));
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_field_owned_by_another_manager_conflicts_until_forced() {
    let ops = common::ops();
    let name = uniq("owned");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let theirs = serde_json::json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": name}, "data": {"k": "theirs"}});
    cms.patch(&name, &kube::api::PatchParams::apply("someone-else"), &kube::api::Patch::Apply(&theirs)).await.unwrap();
    let ours = format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\ndata:\n  k: ours\n");
    let r = ops.apply(&ours, NS, ApplyMode::Commit { force: false }).await.unwrap();
    assert_eq!(r[0].error.as_ref().map(|e| e.code.as_str()), Some("conflict"), "{r:?}");
    let forced = ops.apply(&ours, NS, ApplyMode::Commit { force: true }).await.unwrap();
    assert_eq!(forced[0].action, Some(ApplyAction::Configured), "{forced:?}");
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_stale_edit_is_a_conflict() {
    let ops = common::ops();
    let name = uniq("stale");
    let cms: Api<ConfigMap> = Api::namespaced(kube_client(), NS);
    let mut cm = ConfigMap::default();
    cm.metadata.name = Some(name.clone());
    cms.create(&PostParams::default(), &cm).await.unwrap();
    let opened = ops.get_editable(KubeKind::ConfigMaps, NS, &name).await.unwrap();
    let mut changed = cms.get(&name).await.unwrap();
    changed.data = Some([("x".to_string(), "1".to_string())].into());
    cms.replace(&name, &PostParams::default(), &changed).await.unwrap();
    let edit = opened.yaml.replace("metadata:", "data:\n  y: \"2\"\nmetadata:");
    let r = ops.replace(KubeKind::ConfigMaps, NS, &name, &edit, &opened.resource_version, ApplyMode::Commit { force: false }).await;
    assert!(matches!(r, Err(Error::Conflict(_))), "{r:?}");
    cms.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn editing_a_secret_label_keeps_its_values_and_never_shows_them() {
    let ops = common::ops();
    let name = uniq("sec");
    let secrets: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(kube_client(), NS);
    let mut s = k8s_openapi::api::core::v1::Secret::default();
    s.metadata.name = Some(name.clone());
    s.string_data = Some([("password".to_string(), "hunter2".to_string())].into());
    secrets.create(&PostParams::default(), &s).await.unwrap();
    let opened = ops.get_editable(KubeKind::Secrets, NS, &name).await.unwrap();
    assert!(!opened.yaml.contains("aHVudGVyMg==") && opened.yaml.contains(MASK), "{}", opened.yaml);
    let edit = opened.yaml.replace("metadata:", "metadata:\n  labels:\n    team: a");
    let r = ops.replace(KubeKind::Secrets, NS, &name, &edit, &opened.resource_version, ApplyMode::Commit { force: false }).await.unwrap();
    assert!(!r.after.unwrap().contains("aHVudGVyMg=="));
    let live = secrets.get(&name).await.unwrap();
    assert_eq!(live.data.unwrap()["password"].0, b"hunter2");
    assert_eq!(live.metadata.labels.unwrap()["team"], "a");
    secrets.delete(&name, &Default::default()).await.unwrap();
}

#[tokio::test]
async fn a_narrow_token_cannot_create_a_cluster_role() {
    let ops = fleetops_kube::KubeClient::new(narrow_client());
    let yaml = "apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata:\n  name: fleetops-it-nope\nrules: []\n";
    let r = ops.apply(yaml, NS, ApplyMode::Commit { force: false }).await.unwrap();
    let e = r[0].error.as_ref().expect("forbidden");
    assert_eq!(e.code, "forbidden");
    assert!(e.message.contains("forbidden"), "{}", e.message);
}
```

`kube_client()` and `narrow_client()` are the raw kube clients the suite already builds for its other tests (direct or tunnel per `FLEETOPS_IT_TUNNEL`); if they are not yet helpers in `tests/common/mod.rs`, add them there: `kube_client()` mirrors `ops()` returning the `kube::Client`, and `narrow_client()` uses `tunnel_client_with("narrow-token")` under the tunnel or a kubeconfig built from `token_kubeconfig(read("narrow-token"))` directly (see `a_narrow_token_gets_the_rbac_reason`).

- [ ] **Step 2: Run against kind**

Run: `make test-kube`
Expected: PASS in both the direct and tunnel runs. Failures here are real bugs in Tasks 4–5: debug with `superpowers:systematic-debugging` before changing tests.

- [ ] **Step 3: Commit**

```bash
git add crates/fleetops-kube/tests
git commit -m "test(kube): watch, apply, conflicts and Secret edits against kind"
```

---

### Task 12: Web — the YAML editor (edit, create, apply)

**Files:**
- Modify: `web/package.json` (add `@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/lang-yaml`, `yaml`, `diff`, and dev `@types/diff` if `diff` lacks types)
- Create: `web/src/components/yaml-editor/{CodeMirrorField.tsx,DiffView.tsx,check.ts,YamlDialog.tsx,applyYaml.ts,YamlDialog.test.tsx}`
- Modify: `web/src/pages/kube/KubeActions.tsx`, `KubeListPage.tsx`, `web/src/components/Shell.tsx`, `web/src/components/CommandPalette.tsx`

**Interfaces:**
- Produces:
  ```ts
  export type YamlTarget =
    | { mode: "edit"; kind: KubeKind; ns: string; name: string }
    | { mode: "create"; kind: KubeKind; namespace: string }
    | { mode: "apply"; namespace: string };
  export default function YamlDialog(props: { env: string; target: YamlTarget; onClose(): void }): JSX.Element;
  export function openApplyYaml(env: string): void;          // applyYaml.ts
  export function useApplyYamlRequests(env: string | undefined): [boolean, (open: boolean) => void]; // applyYaml.ts
  export function yamlErrors(text: string): string[];         // check.ts
  ```

- [ ] **Step 1: Install**

Run: `pnpm -C web add @codemirror/state @codemirror/view @codemirror/commands @codemirror/lang-yaml yaml diff`
Expected: installs; `pnpm -C web build` still succeeds.

- [ ] **Step 2: Write the failing tests** (`YamlDialog.test.tsx`)

```tsx
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { mockFetch } from "@/test/fetch";
import YamlDialog from "./YamlDialog";

vi.mock("./CodeMirrorField", () => ({
  default: ({ value, onChange, label }: { value: string; onChange: (v: string) => void; label: string }) => (
    <textarea aria-label={label} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

const wrap = (ui: React.ReactNode) => render(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);
const result = (over = {}) => ({ index: 0, api_version: "apps/v1", kind: "Deployment", namespace: "apps", name: "web",
  action: "configured", before: "replicas: 1\n", after: "replicas: 2\n", error: null, ...over });

describe("YamlDialog", () => {
  it("edits: review shows the dry-run diff, apply writes", async () => {
    const calls = mockFetch({
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": { json: { yaml: "replicas: 1\n", resource_version: "5" } },
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": { json: result() },
    });
    const onClose = vi.fn();
    wrap(<YamlDialog env="k" target={{ mode: "edit", kind: "deployments", ns: "apps", name: "web" }} onClose={onClose} />);
    const editor = await screen.findByRole("textbox", { name: "YAML" });
    await waitFor(() => expect(editor).toHaveValue("replicas: 1\n"));
    await userEvent.clear(editor);
    await userEvent.type(editor, "replicas: 2\n");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    const diff = await screen.findByLabelText("Changes to Deployment web");
    expect(diff).toHaveTextContent("- replicas: 1");
    expect(diff).toHaveTextContent("+ replicas: 2");
    expect(calls.find((c) => c.method === "PUT")!.body).toMatchObject({ resource_version: "5", dry_run: true });
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(calls.filter((c) => c.method === "PUT").at(-1)!.body).toMatchObject({ dry_run: false });
  });

  it("refuses to review YAML that does not parse", async () => {
    mockFetch({ "GET /api/v1/envs/k/k8s/templates": { json: { configmaps: "kind: ConfigMap\nmetadata:\n  namespace: NAMESPACE\n" } } });
    wrap(<YamlDialog env="k" target={{ mode: "create", kind: "configmaps", namespace: "apps" }} onClose={() => {}} />);
    const editor = await screen.findByRole("textbox", { name: "YAML" });
    await waitFor(() => expect(editor).toHaveValue("kind: ConfigMap\nmetadata:\n  namespace: apps\n"));
    await userEvent.type(editor, "{{bad: [");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/line/);
  });

  it("shows the live YAML on a conflict and can reapply on top of it", async () => {
    let n = 0;
    mockFetch({
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": () => ({ json: { yaml: n++ ? "replicas: 3\n" : "replicas: 1\n", resource_version: n > 1 ? "9" : "5" } }),
      "PUT /api/v1/envs/k/k8s/deployments/apps/web": (c) =>
        (c.body as { resource_version: string }).resource_version === "5"
          ? { status: 409, json: { code: "conflict", message: "changed since you opened it", request_id: null } }
          : { json: result() },
    });
    wrap(<YamlDialog env="k" target={{ mode: "edit", kind: "deployments", ns: "apps", name: "web" }} onClose={() => {}} />);
    await screen.findByRole("textbox", { name: "YAML" });
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/changed since you opened it/)).toBeInTheDocument();
    expect(screen.getByLabelText("Live YAML")).toHaveTextContent("replicas: 3");
    await userEvent.click(screen.getByRole("button", { name: "Reload and reapply my edit" }));
    expect(await screen.findByLabelText("Changes to Deployment web")).toBeInTheDocument();
  });

  it("applies several documents, lists failures, and offers force on a conflict", async () => {
    const calls = mockFetch({
      "POST /api/v1/envs/k/k8s/apply": (c) => ({
        json: (c.body as { force: boolean }).force
          ? [result({ action: "configured" })]
          : [result({ action: null, after: null, error: { code: "conflict", message: "conflict with \"kubectl\": .spec.replicas" } })],
      }),
    });
    wrap(<YamlDialog env="k" target={{ mode: "apply", namespace: "apps" }} onClose={() => {}} />);
    await userEvent.type(await screen.findByRole("textbox", { name: "YAML" }), "kind: Deployment\n");
    await userEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/conflict with "kubectl"/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Force apply" }));
    await userEvent.click(await screen.findByRole("button", { name: "Force apply anyway" }));
    await waitFor(() => expect(calls.at(-1)!.body).toMatchObject({ force: true, dry_run: false }));
  });
});
```

- [ ] **Step 3: Run to verify failure**

Run: `pnpm -C web exec vitest run src/components/yaml-editor`
Expected: FAIL — module `./YamlDialog` not found.

- [ ] **Step 4: Implement the pieces**

`check.ts`:

```ts
import { parseAllDocuments } from "yaml";

/** Parse errors, one line each, so nothing malformed is sent. */
export function yamlErrors(text: string): string[] {
  return Array.from(parseAllDocuments(text)).flatMap((d) =>
    d.errors.map((e) => `line ${e.linePos?.[0].line ?? "?"}: ${e.message.split("\n")[0]}`),
  );
}
```

`DiffView.tsx`:

```tsx
import { diffLines } from "diff";

/** A unified line diff; added and removed lines are <ins>/<del> for screen readers. */
export function DiffView({ before, after, label }: { before?: string | null; after?: string | null; label: string }) {
  const parts = diffLines(before ?? "", after ?? "");
  return (
    <pre aria-label={label} className="max-h-[50vh] overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
      {parts.flatMap((p, i) =>
        p.value.replace(/\n$/, "").split("\n").map((line, j) => {
          const k = `${i}-${j}`;
          if (p.added) return <ins key={k} className="block bg-state-ok/15 no-underline">+ {line}</ins>;
          if (p.removed) return <del key={k} className="block bg-state-fail/15 no-underline">- {line}</del>;
          return <span key={k} className="block text-muted-foreground">  {line}</span>;
        }),
      )}
    </pre>
  );
}
```

`CodeMirrorField.tsx`:

```tsx
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { yaml } from "@codemirror/lang-yaml";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { useEffect, useRef } from "react";

/** CodeMirror 6 for YAML. Loaded only with the editor dialog. */
export default function CodeMirrorField({ value, onChange, label }: { value: string; onChange: (v: string) => void; label: string }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const change = useRef(onChange);
  useEffect(() => {
    change.current = onChange;
  });
  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          yaml(),
          EditorView.contentAttributes.of({ "aria-label": label }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) change.current(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => v.destroy();
    // The editor is created once; `value` changes are pushed in below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    const v = view.current;
    if (v && v.state.doc.toString() !== value) v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: value } });
  }, [value]);
  return <div ref={host} className="max-h-[60vh] overflow-auto rounded-md border font-mono text-xs" />;
}
```

`applyYaml.ts`:

```ts
import { useEffect, useState } from "react";

const EVENT = "fleetops:apply-yaml";

/** Opens the Apply YAML dialog for `env` from anywhere (command palette, top bar). */
export function openApplyYaml(env: string) {
  window.dispatchEvent(new CustomEvent(EVENT, { detail: env }));
}

/** Whether the Apply YAML dialog was asked for on `env`; the setter closes it. */
export function useApplyYamlRequests(env: string | undefined): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const on = (e: Event) => (e as CustomEvent<string>).detail === env && setOpen(true);
    window.addEventListener(EVENT, on);
    return () => window.removeEventListener(EVENT, on);
  }, [env]);
  return [open, setOpen];
}
```

`YamlDialog.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useState } from "react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { applyKube, editableKube, kubeTemplates, replaceKube } from "@/api/gen/sdk.gen";
import type { ApplyResult, KubeKind } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { KIND_LABEL } from "@/pages/kube/kinds";
import { yamlErrors } from "./check";
import { DiffView } from "./DiffView";

const CodeMirrorField = lazy(() => import("./CodeMirrorField"));

export type YamlTarget =
  | { mode: "edit"; kind: KubeKind; ns: string; name: string }
  | { mode: "create"; kind: KubeKind; namespace: string }
  | { mode: "apply"; namespace: string };

type Stage = "edit" | "review" | "failed";

const title = (t: YamlTarget) =>
  t.mode === "edit" ? `Edit ${t.name}` : t.mode === "create" ? `Create ${KIND_LABEL[t.kind].replace(/s$/, "")}` : "Apply YAML";

export default function YamlDialog({ env, target, onClose }: { env: string; target: YamlTarget; onClose: () => void }) {
  const qc = useQueryClient();
  const [text, setText] = useState("");
  const [loaded, setLoaded] = useState("");
  const [rv, setRv] = useState("");
  const [stage, setStage] = useState<Stage>("edit");
  const [results, setResults] = useState<ApplyResult[]>([]);
  const [problems, setProblems] = useState<string[]>([]);
  const [conflict, setConflict] = useState<{ yaml: string; rv: string } | null>(null);
  const [confirmForce, setConfirmForce] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        if (target.mode === "edit") {
          const { data } = await editableKube({ path: { env, kind: target.kind, ns: target.ns, name: target.name }, throwOnError: true });
          setText(data.yaml); setLoaded(data.yaml); setRv(data.resource_version);
        } else if (target.mode === "create") {
          const { data } = await kubeTemplates({ path: { env }, throwOnError: true });
          const t = (data[target.kind] ?? "").replaceAll("NAMESPACE", target.namespace);
          setText(t); setLoaded(t);
        }
      } catch (e) {
        setProblems([errorMessage(e)]);
      }
    })();
  }, [env, target]);

  const live = async () => {
    if (target.mode !== "edit") return;
    const { data } = await editableKube({ path: { env, kind: target.kind, ns: target.ns, name: target.name }, throwOnError: true });
    setConflict({ yaml: data.yaml, rv: data.resource_version });
  };

  const send = async (dryRun: boolean, force = false, version = rv): Promise<ApplyResult[] | null> => {
    if (target.mode === "edit") {
      try {
        const { data } = await replaceKube({
          path: { env, kind: target.kind, ns: target.ns, name: target.name },
          body: { yaml: text, resource_version: version, dry_run: dryRun },
          throwOnError: true,
        });
        return [data];
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) {
          await live();
          return null;
        }
        throw e;
      }
    }
    const { data } = await applyKube({ path: { env }, body: { yaml: text, namespace: target.namespace, dry_run: dryRun, force }, throwOnError: true });
    return data;
  };

  const run = async (dryRun: boolean, force = false, version = rv) => {
    const errs = yamlErrors(text);
    if (errs.length) return setProblems(errs);
    setProblems([]); setBusy(true);
    try {
      const out = await send(dryRun, force, version);
      if (!out) return;
      setResults(out);
      const failed = out.some((r) => r.error);
      if (dryRun) return setStage("review");
      await qc.invalidateQueries({ queryKey: ["env", env, "k8s"] });
      if (failed) return setStage("failed");
      toast.success(out.length === 1 ? `${out[0].kind} ${out[0].name} ${out[0].action}` : `${out.length} objects applied`);
      onClose();
    } catch (e) {
      setProblems([errorMessage(e)]);
    } finally {
      setBusy(false);
    }
  };

  const close = () => {
    if (text !== loaded && stage !== "failed" && !window.confirm("Discard your changes?")) return;
    onClose();
  };

  const forceable = target.mode !== "edit" && results.some((r) => r.error?.code === "conflict");

  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent className="max-w-4xl">
        <DialogTitle>{title(target)}</DialogTitle>
        <DialogDescription>
          {stage === "edit" ? "Review shows what would change before anything is written." : "Nothing is written until you apply."}
        </DialogDescription>
        {problems.length > 0 && (
          <ul role="alert" className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
            {problems.map((p) => <li key={p}>{p}</li>)}
          </ul>
        )}
        {conflict ? (
          <div className="grid gap-2">
            <p className="text-sm">This object changed since you opened it.</p>
            <div className="grid gap-2 md:grid-cols-2">
              <pre aria-label="Your edit" className="max-h-[40vh] overflow-auto rounded-md border p-2 font-mono text-xs">{text}</pre>
              <pre aria-label="Live YAML" className="max-h-[40vh] overflow-auto rounded-md border p-2 font-mono text-xs">{conflict.yaml}</pre>
            </div>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => { setText(conflict.yaml); setLoaded(conflict.yaml); setRv(conflict.rv); setConflict(null); setStage("edit"); }}>Discard</Button>
              <Button onClick={() => { const v = conflict.rv; setRv(v); setConflict(null); void run(true, false, v); }}>Reload and reapply my edit</Button>
            </div>
          </div>
        ) : stage === "edit" ? (
          <>
            <Suspense fallback={<p className="text-sm text-muted-foreground">Loading editor…</p>}>
              <CodeMirrorField label="YAML" value={text} onChange={setText} />
            </Suspense>
            <p className="text-xs text-muted-foreground">
              Secret values are hidden; leave them unchanged to keep them. Ctrl+M (Shift+Alt+M on macOS) lets Tab leave the editor.
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={close}>Cancel</Button>
              <Button disabled={busy || !text.trim()} onClick={() => void run(true)}>Review</Button>
            </div>
          </>
        ) : (
          <>
            <ul className="grid gap-3">
              {results.map((r) => (
                <li key={r.index} className="grid gap-1">
                  <p className="text-sm font-medium">
                    {r.kind} {r.namespace ? `${r.namespace}/` : ""}{r.name}{" "}
                    <span className="text-muted-foreground">{r.error ? "failed" : r.action}</span>
                  </p>
                  {r.error ? (
                    <p className="text-sm text-destructive">{r.error.message}</p>
                  ) : (
                    <DiffView label={`Changes to ${r.kind} ${r.name}`} before={r.before} after={r.after} />
                  )}
                </li>
              ))}
            </ul>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setStage("edit")}>Back to editing</Button>
              {forceable && <Button variant="outline" onClick={() => setConfirmForce(true)}>Force apply</Button>}
              {stage === "review" && <Button disabled={busy} onClick={() => void run(false)}>Apply</Button>}
            </div>
          </>
        )}
        {confirmForce && (
          <ConfirmDialog
            open
            onOpenChange={(o) => !o && setConfirmForce(false)}
            title="Force apply?"
            description="FleetOps takes ownership of the conflicting fields. The other manager may change them back."
            confirmLabel="Force apply anyway"
            onConfirm={() => run(false, true)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
```

(Check `ConfirmDialog`'s prop names in `web/src/components/ConfirmDialog.tsx` — use its actual confirm-label and action props.)

- [ ] **Step 5: Entry points**

`KubeActions.tsx`: add `"edit"` to the `open` union, a `<Button variant="outline" size="sm" onClick={() => setOpen("edit")}>Edit YAML</Button>` first in the row, and

```tsx
      {open === "edit" && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "edit", kind, ns, name }} onClose={() => setOpen(null)} />
        </Suspense>
      )}
```

with `const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));` at module level.

`KubeListPage.tsx`: for every kind except `nodes`, a `Create` button in `actions` (next to the namespace picker) opening `YamlDialog` with `{ mode: "create", kind, namespace: ns || "default" }`.

`Shell.tsx` `TopBar`: when `kube && env`, a `Button` "Apply YAML" calling `openApplyYaml(env)`; in `Shell`, `const [applying, setApplying] = useApplyYamlRequests(kube ? env : undefined);` and render `{applying && env && <Suspense fallback={null}><YamlDialog env={env} target={{ mode: "apply", namespace: localStorage.getItem(`fleetops.ns.${env}`) || "default" }} onClose={() => setApplying(false)} /></Suspense>}` (the key matches `NamespacePicker`'s storage; read it inside try/catch).

`CommandPalette.tsx`: when the current environment is Kubernetes, a `CommandItem value="Apply YAML"` in the Pages group with `onSelect={() => go(async () => openApplyYaml(e))}`.

Add one Shell test to `web/src/pages/kube/kube.test.tsx`:

```tsx
  it("opens Apply YAML from the top bar of a Kubernetes environment", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("button", { name: "Apply YAML" }));
    expect(await screen.findByRole("dialog", { name: "Apply YAML" })).toBeInTheDocument();
  });
```

- [ ] **Step 6: Run tests and the bundle check**

Run: `pnpm -C web exec vitest run && pnpm -C web exec tsc --noEmit && pnpm -C web lint && pnpm -C web build`
Expected: PASS; in `web/dist/assets`, `@codemirror` code appears only in a chunk that is not the entry (`grep -l "EditorView" web/dist/assets/*.js` names a non-`index-*.js` file).

- [ ] **Step 7: Commit**

```bash
git add web
git commit -m "feat(web): YAML editor to edit, create and apply with a dry-run diff"
```

---

### Task 13: Web — access level when adding an in-cluster agent

**Files:**
- Modify: `web/src/pages/admin/EnvironmentsPage.tsx`
- Test: `web/src/pages/admin/admin.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
  it("asks for the agent's access level and sends it", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/envs": { json: ENVS },
      "POST /api/v1/envs": { status: 201, json: { ...KUBE_ENV, kind: "kube-agent", enrollment: { join_token: "fj_x", expires_at: 9, command: "kubectl apply" } } },
    });
    renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /Kubernetes/ }));
    await userEvent.click(within(dialog).getByRole("radio", { name: /In-cluster agent/ }));
    expect(within(dialog).getByRole("radio", { name: /Standard/ })).toBeChecked();
    await userEvent.click(within(dialog).getByRole("radio", { name: /Cluster admin/ }));
    expect(within(dialog).getByText(/full control of the cluster/)).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "ka");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ kind: "kube-agent", access: "cluster-admin" }));
  });
```

(import `KUBE_ENV`, `ENVS` from `@/test/app` as needed.)

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C web exec vitest run src/pages/admin`
Expected: FAIL — no "Standard" radio.

- [ ] **Step 3: Implement**

In `EnvForm`: `const [access, setAccess] = useState<KubeAccess>("standard");`. When `kind === "kube-agent"` and adding, render a `fieldset` with legend "Access in the cluster" and three radio inputs (same visual pattern as `KindPicker`), labelled and described:

```tsx
const ACCESS: [KubeAccess, string, string][] = [
  ["read-only", "Read-only", "Browse, read logs. No changes, no terminal."],
  ["standard", "Standard", "Change the common kinds, scale, restart, delete, terminal."],
  ["cluster-admin", "Cluster admin", "Apply any kind, including CRDs and RBAC: full control of the cluster."],
];
```

and send `{ name, kind, access }` in the agent `createEnv` body when `kind === "kube-agent"`. In `AgentCell`, after the version, show `a.access` (e.g. `<span className="text-xs text-muted-foreground">{a.access}</span>`) when set.

- [ ] **Step 4: Run tests**

Run: `pnpm -C web exec vitest run && pnpm -C web lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add web
git commit -m "feat(web): pick an in-cluster agent's access level"
```

---

### Task 14: End to end, memory, and docs

**Files:**
- Modify: `web/e2e/kube.spec.ts`
- Create: `scripts/kube-watch-load.mjs`
- Modify: `scripts/rss-bench.sh`, `README.md`

- [ ] **Step 1: Playwright — create from a template, see a kubectl change live**

Append a step to the existing kubeconfig test in `web/e2e/kube.spec.ts`, after "browse deployments in a namespace" (the page is on the deployments list in `fleetops-it`):

```ts
  await test.step("create a deployment from its template and watch kubectl scale it", async () => {
    const name = `e2e-${Date.now().toString(36)}`;
    await page.goto(`/envs/${ENV}/k8s/deployments`);
    await page.getByRole("button", { name: "Create" }).click();
    const dialog = page.getByRole("dialog");
    const editor = dialog.getByRole("textbox", { name: "YAML" });
    await expect(editor).toContainText("kind: Deployment");
    await editor.click();
    await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
    await page.keyboard.insertText(
      `apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: ${name}\n  namespace: fleetops-it\nspec:\n  replicas: 1\n  selector:\n    matchLabels: {app: ${name}}\n  template:\n    metadata:\n      labels: {app: ${name}}\n    spec:\n      containers:\n        - name: main\n          image: busybox:1.37\n          command: ["sh", "-c", "sleep 3600"]\n`,
    );
    await dialog.getByRole("button", { name: "Review" }).click();
    await expect(dialog.getByLabel(`Changes to Deployment ${name}`)).toBeVisible();
    await dialog.getByRole("button", { name: "Apply" }).click();
    await expect(dialog).toBeHidden();
    const row = page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });
    await expect(row).toBeVisible();
    const kubeconfig = join(process.env.FLEETOPS_KUBE_IT ?? "../target/kube-it", "kubeconfig");
    execFileSync("kubectl", ["--kubeconfig", kubeconfig, "-n", "fleetops-it", "scale", `deploy/${name}`, "--replicas=2"]);
    await expect(row.getByText(/\/2$/)).toBeVisible({ timeout: 10_000 });
    execFileSync("kubectl", ["--kubeconfig", kubeconfig, "-n", "fleetops-it", "delete", `deploy/${name}`]);
    await expect(row).toBeHidden({ timeout: 10_000 });
  });
```

(`import { execFileSync } from "node:child_process";`). No `page.reload()` anywhere in this step: the point is that the list updates on its own.

Run: `make e2e-kube` → PASS.

- [ ] **Step 2: rss-bench watch phase**

`scripts/kube-watch-load.mjs` (Node 22+, global `WebSocket` from undici accepts `headers`):

```js
// Holds `per` kube-watch subscriptions open on each of the named
// environments until killed: node kube-watch-load.mjs <base> <cookie> <per> env...
const [base, cookie, per, ...envs] = process.argv.slice(2);
const kinds = ["pods", "deployments", "services", "configmaps", "secrets"].slice(0, Number(per));
const ws = new WebSocket(base.replace(/^http/, "ws") + "/api/v1/ws", { headers: { cookie } });
let sub = 0;
ws.onopen = () => {
  for (const env of envs) for (const kind of kinds) ws.send(JSON.stringify({ op: "sub", sub: ++sub, topic: "kube-watch", env, id: `${kind}/-` }));
};
ws.onmessage = () => {};
ws.onclose = () => process.exit(1);
```

In `scripts/rss-bench.sh`, inside the existing kube phase after the list polling and before measuring: start `node scripts/kube-watch-load.mjs "$base" "fleetops_session=$(session cookie from $jar)" 3 rss-kube-0 … rss-kube-$((kube_envs-1)) &`, sleep 10, measure with `peak_rss`, then kill the loader. Keep the limit `limit_mib + agents + 5`, and print "with $kube_envs kube environments × 3 watches". Read the cookie from the curl jar with `awk '$6=="fleetops_session"{print $7}' "$jar"` (use the cookie name the server sets; see `SESSION_COOKIE`).

Run: `make rss-bench KUBE_IT_RSS=target/kube-it` → passes the limit. Record the measured MiB in the commit message.

- [ ] **Step 3: README**

In the "Kubernetes clusters" section add:
- **Live updates:** lists and object pages update from watches; when a watch fails the page says "Live updates paused".
- **Editing:** "Edit YAML" on object pages, "Create" from templates, "Apply YAML" (top bar / command palette); every write shows a dry-run diff first; edits refuse if the object changed; Secret values stay hidden and unchanged masked values are kept.
- **Agent access levels:** the three levels and their rules; raising is a re-apply; lowering from cluster-admin also needs `kubectl delete clusterrolebinding fleetops-agent-cluster-admin`; the old README read-only ClusterRole is now the generated `read-only` level.

Update the roadmap table row 3 to "(slice 3b done: live and writable)" and list 3c RBAC (Docker + Kubernetes, namespace scoping) and 3d Helm before sub-project 4; update the status paragraph and `PRODUCT.md` roadmap order (3 Kubernetes, 4 Stacks + GitOps) to match.

- [ ] **Step 4: Full verification**

Run: `make lint && make test && make test-web && make test-kube && make e2e-kube`
Expected: all PASS. Use `superpowers:verification-before-completion` before reporting.

- [ ] **Step 5: Commit**

```bash
git add web/e2e scripts README.md PRODUCT.md
git commit -m "test: live and writable Kubernetes end to end, watch memory phase; docs"
```

---

## Self-Review Notes

- **Spec coverage:** §2.2 types → Task 1; §2.3 watch → 4, apply/replace/cleaning/unmask/templates → 2, 3, 5, 8; §2.4 registry → 6; §2.5 access levels → 9, 13; §3 data model → 9; §4 flows → 10, 12; §5 API and topic → 7, 8; §6 web → 10, 12, 13; §7 errors → 4 (watch), 5 (per-document), 8 (409, size, hints), 12 (parse errors, conflict view); §8 security → 2, 8, 11; §9 testing → 4, 6, 7, 8, 10, 11, 12, 14; §10 delivery → 4 (feature), 12 (deps), 14 (README).
- **Types:** `WatchEvent` variants (`Snapshot{rows}`, `Added{row}`, `Modified{row}`, `Deleted{namespace,name}`, `Resync`) are used identically in Tasks 1, 4, 6, 7, 10, 11. `ApplyResult.error: Option<ApplyError{code,message}>` is used identically in 1, 5, 8, 12. `useApplyYamlRequests` returns `[open, setOpen]` in both the Task 12 interface block and its code.
- **Known judgment calls:** kube-rs 4.2 watcher error variant names are flagged in Task 4 with where to check; route ordering for static `/k8s/apply` and `/k8s/templates` is checked in Task 8 Step 5.
