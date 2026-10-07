# FleetOps — Sub-project 3d: Helm releases

- **Date:** 2026-10-07
- **Status:** Draft
- **Builds on:** [Sub-project 3b spec](./2026-10-06-fleetops-kube-live-writable-design.md) (apply, diff review, masking), [Sub-project 3c spec](./2026-10-06-fleetops-rbac-design.md) (grants, `check_ns`)

## 1. Intent

Kubernetes environments show the Helm releases installed in them: their chart, status, revision history, values, rendered manifest and notes. An operator can roll a release back to an earlier revision or uninstall it. Everything is native Rust over the existing kube client, through a kubeconfig or the agent tunnel; there is no `helm` binary. Releases written by FleetOps are ordinary Helm 3 release records, so the Helm CLI keeps working on them.

Install and upgrade (charts, repositories, OCI, values editing) come in a later slice.

### Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Scope | Read (list, history, values, manifest, notes) plus rollback and uninstall. No install or upgrade. |
| Implementation | Native: decode and write `sh.helm.release.v1` Secrets; change objects with a three-way merge patch. No bundled `helm` binary. Hooks are not run, and the UI says so. |
| Values | Shown to viewers and operators alike, with any leaf whose key looks like a credential masked; Secret objects in the manifest masked with `mask.rs`. |
| Storage drivers | Helm 3 Secrets driver (Helm's default) only. |
| Live updates | The releases list polls every 10 s while visible and refreshes after a FleetOps action. No watch stream. |

### Success criteria

1. A release installed with the Helm CLI appears in FleetOps within 10 s with the right chart, revision and status, through both a kubeconfig and an agent environment.
2. After a FleetOps rollback, `helm history` shows a new `deployed` revision described as a rollback, fields and objects added after the target revision are gone, and a later `helm upgrade` from the CLI succeeds.
3. After a FleetOps uninstall, `helm list` no longer shows the release and its objects are gone, except those marked `helm.sh/resource-policy: keep`.
4. No value under a masked key and no Secret data reaches the browser, a log line or an audit row.
5. Nothing is written without a dry run reviewed and confirmed in the UI.
6. Every existing Docker and Kubernetes test passes.

## 2. Architecture

### 2.1 Layout changes

```
crates/
  fleetops-core/  + helm.rs: HelmRelease, HelmRevision, ReleaseDetail, RollbackPlan, UninstallPlan;
                  KubeOps gains helm_list, helm_get, helm_history, helm_rollback, helm_uninstall
  fleetops-kube/  + helm/storage.rs  decode and encode sh.helm.release.v1 Secrets; labels
                  + helm/release.rs  latest revision per release; status; rollback and uninstall plans
                  + helm/patch.rs    three-way JSON merge patch
                  + helm/values.rs   key-name masking of values
  fleetops-api/   + routes/helm.rs; OpenAPI entries (tag `helm`); audit actions
web/src/pages/helm/   ReleasesPage, ReleasePage (tabs: Overview, History, Values, Manifest, Notes)
```

The agent binary and its access levels do not change.

### 2.2 Release storage

Helm 3 stores each revision as a Secret named `sh.helm.release.v1.<name>.v<revision>` in the release namespace, of type `helm.sh/release.v1`, labelled `owner=helm`, `name`, `status`, `version`. Its `data.release` is base64 of gzip of the release JSON (`name`, `namespace`, `version`, `info` {`status`, `description`, `first_deployed`, `last_deployed`, `deleted`, `notes`}, `chart` {`metadata`, …}, `config` (user values), `manifest`, `hooks`).

`storage.rs` decodes this into a typed struct that keeps unknown fields (`serde_json::Value` flatten), so a re-encoded record loses nothing Helm wrote. Encoding uses the same layers and the same labels. The decoded JSON never leaves `fleetops-kube` unmasked: the API types carry only masked strings.

### 2.3 Reading

- **List:** metadata-only list of Secrets with label `owner=helm` (one namespace or all), grouped by the `name` label and namespace, the highest `version` kept; only those Secrets are fetched and decoded. A row: name, namespace, chart name and version, app version, status, revision, last deployed. A Secret that fails to decode gives a row with status `unreadable` and the reason.
- **Detail:** one revision (latest by default): chart, status, description, notes, values (masked, §2.4), manifest (masked, §2.4).
- **History:** every revision's number, status, chart version, app version, description and time; decoded from the metadata list plus each Secret's `info` (fetched per revision; a release keeps at most Helm's `--history-max`, 10 by default).

### 2.4 Masking

- **Values:** any leaf (scalar, or a list or map in place of one) whose key matches `password|passwd|secret|token|key|credential|auth|cert|private` (case-insensitive substring) is replaced with `MASK`. Applied to the values YAML shown and to any description or error text built from it.
- **Manifest:** each document is parsed and Secret objects go through `mask.rs` as in 3b; other documents are shown as rendered.
- Values are never written to audit rows or log lines.

## 3. Rollback

Rollback to revision N, with the latest revision C. Operator role.

### 3.1 Plan (dry run)

1. Fetch and decode C and N. Refuse with 409 if C's status is `pending-install`, `pending-upgrade` or `pending-rollback`, or N is C, or N does not exist.
2. Split both manifests (3b `apply::split`), key objects by (apiVersion group, kind, namespace defaulted to the release namespace, name).
3. For each object in N:
   - also in C: the patch is `merge_patch(C_obj → N_obj)`: keys that differ take N's value, keys in C and not in N are set to `null`, keys equal in both are left out. Lists are compared whole and, when they differ, replaced by N's list. The patch is sent as `application/merge-patch+json` with `dryRun=All`.
   - not in C: a dry-run create.
4. Each object in C and not in N goes on the delete list, unless annotated `helm.sh/resource-policy: keep`.
5. The reply: C's revision number; per object the action (`patch`, `create`, `delete`), masked before and after YAML, or the error the cluster returned; and the note that hooks will not run.

Why a three-way patch and not 3b's server-side apply: SSA only removes fields owned by the `fleetops` manager, while a Helm release's fields are owned by `helm`. A field added in C and absent in N would survive an SSA rollback. The three-way patch is what Helm computes and it leaves fields set by controllers and other managers alone.

### 3.2 Commit

The request carries `revision: N` and `expected_current: C`.

1. Re-read the latest revision. If it is not C, or is `pending-*`, return 409 `release changed; review again`.
2. Create Secret `…v{C+1}` from N's record with `version = C+1`, status `pending-rollback`, description `Rollback to N`. If the create fails because the Secret exists, return 409; this is the lock against a concurrent Helm or FleetOps operation.
3. Apply each patch and create, then delete each object on the delete list. Every step runs; failures are collected per object.
4. All succeeded: update C+1 to `deployed`, description `Rollback to N (FleetOps; hooks not run)`, `last_deployed` now; update C to `superseded`. Any failed: update C+1 to `failed` with the description listing the failed objects (masked); C stays `deployed`, as Helm leaves it after a failed rollback.
5. Reply with the per-object results in 3b's `ApplyResult` shape.

## 4. Uninstall

Operator role. The request carries `expected_current: C` and `dry_run`.

- **Plan:** the objects of C's manifest that would be deleted (in Helm's uninstall order: the reverse of Helm's install kind order, then name), the objects kept by `helm.sh/resource-policy: keep`, and the note that hooks will not run.
- **Commit:** the 409 check of §3.2 step 1; update C to `uninstalling`; delete each object (a 404 counts as done); then delete every `sh.helm.release.v1.<name>.v*` Secret. If any object delete failed, the release Secrets are kept, C is set to `failed` with the errors in its description, and the reply lists them.

The web confirmation asks the user to type the release name.

## 5. Access (3c)

| Route | Rule |
|---|---|
| list with `namespace` | `check_ns`. |
| list, all namespaces | Rows outside the grant's set dropped before the reply. |
| detail, history | `check_ns` on the release namespace. |
| rollback, uninstall | Operator; `check_ns` on the release namespace. For a namespace-limited grant, the whole action is refused before any dry run (403, audited `denied`) when any object in C or N is cluster-scoped or in a namespace outside the set. A partial rollback would leave the release half-reverted. |

A write the cluster refuses (for example an agent at access level `read-only`, or `standard` with a chart containing RBAC objects) comes back as that object's error in the plan, before anything is committed.

## 6. API

All under the existing prefix, tag `helm`. A Docker environment gets 409, as other `k8s` routes do.

| Method and path | Role | Body / query | Reply |
|---|---|---|---|
| `GET /envs/{env}/k8s/helm/releases` | viewer | `?namespace=` | `HelmRelease[]` |
| `GET /envs/{env}/k8s/helm/releases/{ns}/{name}` | viewer | `?revision=` | `ReleaseDetail` |
| `GET /envs/{env}/k8s/helm/releases/{ns}/{name}/history` | viewer | | `HelmRevision[]` |
| `POST /envs/{env}/k8s/helm/releases/{ns}/{name}/rollback` | operator | `{revision, expected_current, dry_run}` | `RollbackPlan` or results |
| `POST /envs/{env}/k8s/helm/releases/{ns}/{name}/uninstall` | operator | `{expected_current, dry_run}` | `UninstallPlan` or results |

Audit: `helm.rollback` (env, namespace, release, from C, to N, outcome) and `helm.uninstall` (env, namespace, release, revision, outcome). Dry runs are not audited.

## 7. Web UI

- **Navigation:** a *Helm releases* entry in the Kubernetes environment navigation, using the existing namespace picker.
- **Releases page:** table of releases with a status badge; `failed`, `pending-*` and `unreadable` highlighted. Polls every 10 s while visible.
- **Release page:** tabs Overview (chart, app version, status, description, last deployed), History (each older revision has *Roll back to this*), Values (read-only CodeMirror, masked), Manifest (read-only, masked), Notes (plain text). Operator actions hidden for viewers.
- **Rollback:** the 3b review dialog with a diff per object, the delete list, the hooks note, and a confirm. Results shown per object, as for apply.
- **Uninstall:** the plan's delete and keep lists and a type-the-release-name confirmation.

## 8. Error handling

| Situation | Status |
|---|---|
| Release not found | 404 `release not found` |
| Release Secret cannot be decoded | list row `unreadable`; detail 422 with the reason |
| Latest revision changed or pending | 409 `release changed; review again` |
| Namespace or cluster-scoped object outside the grant | 403, audited `denied` (3c wording) |
| Cluster refuses an object | that object's error in the plan or results |

## 9. Testing

- **Unit (`fleetops-kube`):** decode and re-encode round trip against fixtures written by Helm 3 (unknown fields kept, labels right); latest-revision picking; status mapping; the three-way patch (changed field, removed field, unchanged field left out, list replaced only when changed, nested maps); delete list honouring `keep`; uninstall order; values masking (nested keys, lists, case).
- **API (FakeKube):** every route; role gating; the 3c whole-action refusal; 409 for a changed and a pending release and for the lock; audit rows; masked values absent from audit rows and logs; OpenAPI operation count.
- **`kube_it`** (kind, through a kubeconfig and through the tunnel): install a fixture chart with the `helm` CLI; upgrade to revision 2, which adds a field to the Deployment and a ConfigMap; roll back to 1 through FleetOps; check the field and the ConfigMap are gone, `helm history` shows revision 3 `deployed`, and `helm upgrade` from the CLI succeeds. Then uninstall through FleetOps and check `helm list` is empty and the objects are gone. The CI `kube` job gains a pinned `helm` binary for tests only; it is not shipped in any image.
- **Web (vitest):** releases table and badges, tabs, masked values, rollback review dialog, uninstall confirmation, operator actions hidden for viewers.
- **e2e:** one step in `e2e/kube.spec.ts`: open a release, roll back, see the new revision.

## 10. Build and delivery

- README: a *Helm releases* section (what is shown, masking, rollback and uninstall, hooks not run, Secrets driver only) replacing "Helm comes later"; the roadmap marks 3d done.
- No new runtime dependencies beyond `flate2` (gzip) if not already in the workspace.

## 11. Out of scope

Install and upgrade, chart repositories and OCI, values editing, running hooks, `helm test`, the ConfigMap and SQL storage drivers, Helm 2 releases, keep-history uninstall, live watch streams for releases.

## 12. Risks

| Risk | Mitigation |
|---|---|
| The merge patch differs from Helm's strategic merge for lists: a list changed between C and N is replaced whole, which can drop entries injected by others (for example sidecars added to `containers` by a webhook) | Shown in the dry-run diff before commit; `kube_it` interop test; documented in the README. |
| A re-encoded release record loses fields Helm needs | Typed decode keeps unknown fields; round-trip tests on real Helm fixtures; `helm history` and `helm upgrade` checked in `kube_it`. |
| Concurrent Helm CLI operation during a rollback | `expected_current` check plus the create-only `v{C+1}` lock; pending statuses refused. |
| Credentials in values under unexpected key names | Key-name masking is best effort; documented. Values are never logged or audited. |
| Large release Secrets (up to 1 MiB each) make the list slow | Metadata-only list; only the latest revision per release decoded. |
