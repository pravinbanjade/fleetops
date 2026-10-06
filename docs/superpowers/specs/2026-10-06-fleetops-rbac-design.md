# FleetOps — Sub-project 3c: Per-environment RBAC

- **Date:** 2026-10-06
- **Status:** Draft
- **Builds on:** [Sub-project 3b spec](./2026-10-06-fleetops-kube-live-writable-design.md), [Core spec](./2026-09-29-fleetops-core-docker-design.md) §authz
- **Replaces:** the per-environment RBAC part of sub-project 5

## 1. Intent

Today FleetOps has two global roles: `admin` does everything, `user` does everything except manage environments and users. Every user sees and changes every environment. This slice lets an admin decide, per user and per environment, whether that user can see it (**viewer**) or change it (**operator**), and, for Kubernetes environments, limit the grant to a set of namespaces.

### Decisions taken in brainstorming

| Question | Decision |
|---|---|
| Role set | Two per-environment roles, `viewer` and `operator`. The global `admin` is the only one who manages environments, users and access. |
| Bound to | Users directly. No teams or groups. |
| Upgrade | Existing `user` accounts get `operator` on every existing environment. A new environment has no bindings: only admins see it until access is granted. |
| Cluster-scoped kinds for namespace-limited users | Nodes readable; Namespaces readable only for those in the set; nothing cluster-scoped writable or appliable. |
| Enforcement | In FleetOps, one decision point (`authz`). Cluster calls are unchanged; results outside the grant are filtered server-side. No impersonation, no per-namespace fan-out, no agent changes. |

### Success criteria

1. An admin grants "alice: operator on `prod`, namespaces `team-a`, `team-b`". Alice sees only `prod` in the environment list; its lists, live watches, apply, edit, delete, logs and exec work only inside `team-a` and `team-b`.
2. A viewer can read everything their grant covers and is refused every write with a 403 naming the reason; the write buttons are not shown.
3. A user with no binding on an environment gets 404 for it, exactly as for one that does not exist.
4. Changing or revoking a grant takes effect on the next REST request and ends that user's open subscriptions.
5. Upgrading an existing install changes nothing anyone can do.
6. Filtered subscribers share the existing upstream watches: the watch count does not grow with the number of users.

## 2. Model

### 2.1 Storage (migration `0005_access.sql`)

`role_bindings` is rebuilt:

```sql
CREATE TABLE role_bindings (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role           TEXT    NOT NULL CHECK (role IN ('viewer', 'operator')),
    namespaces     TEXT,            -- JSON array of names; NULL = whole environment
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (user_id, environment_id)
);
```

- The old global rows (`environment_id IS NULL`, mirroring `users.role`) are dropped; `users.role` (`admin` | `user`) stays the only global role.
- Backfill: one `operator` row with `namespaces = NULL` for every (`user`-role account, environment) pair.
- An admin's bindings are kept but ignored while they are admin; demoting them brings the bindings back into force. Granting to an admin through the API is refused (§4).
- `namespaces` is non-NULL only for Kubernetes environments (`kube-config`, `kube-agent`); the store refuses it otherwise.

### 2.2 Grants

```rust
pub enum EnvRole { Viewer, Operator }

pub struct Grant {
    pub role: EnvRole,
    /// None = the whole environment.
    pub namespaces: Option<BTreeSet<String>>,
}
```

- `grant(user, env_id) -> Option<Grant>`: an admin gets `Operator` with `None`; a `user` gets its binding, or `None` without one. It is read from the store on every request; there is no cache.
- `authorize(grant: Option<&Grant>, action) -> Result<()>`:
  - no grant → `NotFound("environment …")`, the same error as a missing environment;
  - read actions (`DockerRead`, `KubeRead`) need `Viewer`; write actions (`DockerWrite`, `KubeWrite`) need `Operator` → otherwise `Forbidden("operator role required")`.
- `EnvRead` / `EnvManage` keep their meaning: `EnvManage` is admin-only; `EnvRead` lists only environments the caller has a grant on.
- Kubernetes helpers on `Grant`:
  - `allows_ns(ns) -> bool` — `None` set or `ns ∈ set`.
  - `check_ns(ns) -> Result<()>` — `Forbidden("namespace 'x' is not in your access")`.
  - `check_cluster_scoped(kind, write) -> Result<()>` — for a namespace-limited grant: reads of `Nodes` and `Namespaces` pass, everything else (and every write) is `Forbidden("cluster-scoped objects are outside your access")`. For an unlimited grant, it defers to the role.

### 2.3 Where it applies

- `docker_for` and `kube_for` resolve the grant, call `authorize`, and return the grant alongside the ops handle, so handlers can apply namespace checks.
- `GET /envs` returns only environments with a grant; each `EnvView` gains `access: {role, namespaces}` for the caller.
- The websocket env-status broadcast is filtered per connection the same way.
- The audit log stays admin-only.

## 3. Enforcement

### 3.1 Docker

The whole environment is one scope.

| Role | Allowed |
|---|---|
| viewer | List and inspect containers, images, volumes, networks and Compose projects; container logs and stats; events. |
| operator | Viewer, plus start/stop/restart/kill/pause/remove, create, pull, prune, Compose deploy/stop/down, and exec. |

Exec is a write on both Docker and Kubernetes. Tasks keep their owner check unchanged.

### 3.2 Kubernetes (namespace-limited grants)

A grant with `namespaces = None` behaves as today, subject only to role. With a set:

| Route / topic | Rule |
|---|---|
| `list` with `namespace` | `check_ns`. |
| `list` all namespaces, namespaced kind | Cluster-wide call as today; rows outside the set dropped before the reply. |
| `list` Nodes | All rows. |
| `list` Namespaces | Only rows named in the set. |
| `get`, `events` | `check_ns` on the object's namespace; a Namespace object passes if its name is in the set; a Node passes. |
| `delete`, `scale`, `restart`, `editable`, `replace` | Operator; `check_ns`; cluster-scoped refused. |
| `apply` | Operator; the request's default namespace must pass `check_ns`. Each document is checked after namespace defaulting and **before** dry-run or commit: cluster-scoped, or namespace outside the set, gives that document `error: {code: "forbidden", message}` and it is not sent; the others proceed. |
| `templates` | Unchanged. |
| `kube-watch` | Subscribe: a specific namespace must pass `check_ns`; a cluster-scoped kind follows the `list` rule. All-namespace subscriptions share the existing upstream watch; each subscriber filters `Snapshot` rows and `Added`/`Modified`/`Deleted` events to its set. Namespaces watches filter by name. |
| pod logs, exec | `check_ns` on the pod's namespace; exec needs operator. |

The filter sits in the subscriber (`WatchSub`), not in the shared registry, so the registry and its upstream watch count are unchanged.

### 3.3 Access changes during a session

- REST re-reads the grant every request.
- Any grant change or revocation, user deletion, or role change on a user publishes `ACCESS_CHANGED(user_id)` on the hub. Each of that user's connections ends every open subscription with `End { message: "your access changed" }` and re-sends its env-status list.
- The web client, on that message, invalidates the environment list and resubscribes; topics that are now refused end with the refusal text.

### 3.4 Errors

| Situation | Status | Audited |
|---|---|---|
| No grant on the environment | 404 `environment not found` | no |
| Viewer attempting a write | 403 `operator role required` | yes, `denied` |
| Namespace outside the set | 403 `namespace 'x' is not in your access` | yes, `denied` |
| Cluster-scoped object for a limited grant | 403 `cluster-scoped objects are outside your access` | yes, `denied` |
| Refused topic subscription | `End` with the same text | no |

## 4. Management API (admin only)

New `routes/access.rs`:

| Method | Path | Body / result |
|---|---|---|
| `GET` | `/users/{id}/access` | `[{env_id, env_name, env_kind, role, namespaces}]` |
| `PUT` | `/users/{id}/access/{env_id}` | `{role, namespaces?: [string]}` → the binding (upsert) |
| `DELETE` | `/users/{id}/access/{env_id}` | 204 |
| `GET` | `/envs/{id}/access` | `[{user_id, username, role, namespaces}]` |

Validation:

- `namespaces`: each an RFC 1123 label (the existing `check_namespace`), at most 50, deduplicated and sorted; an empty array is refused ("give at least one namespace, or omit for the whole cluster"); refused for Docker environments.
- The target must be a `user`-role account: granting to an admin is 400 `admins already have full access`.
- Unknown user or environment is 404.

Each change is audited (`access.grant`, `access.revoke`, target `user/<id>`, detail with role and namespaces) and publishes `ACCESS_CHANGED`.

## 5. Web UI

- **Users page** — non-admin rows gain an **Access** action opening a dialog: one row per environment with a role select (None / Viewer / Operator); Kubernetes rows add an **Entire cluster** toggle and, when off, a namespace chip input. Saving issues `PUT`/`DELETE` per changed row.
- **Environments page** — each row's menu gains **Access**, a read-only list of who holds which role there, each linking to that user's dialog.
- **Everywhere else**, using `EnvView.access` (admins get `operator`/whole):
  - The environment switcher lists only granted environments and shows a Viewer badge for viewer grants.
  - Write affordances are hidden for viewers: Docker actions, Compose deploy, exec, Kubernetes Create, Apply YAML (also hidden from the command palette), Edit YAML, Delete, Scale, Restart.
  - Kubernetes namespace pickers offer only the set; "All namespaces" means all of the user's namespaces. For a limited grant the cluster-scoped nav shows Nodes and Namespaces only.
  - A user with no grants sees an empty state: "You don't have access to any environments yet. Ask an admin."

The server enforces everything regardless of the UI.

## 6. Edge cases

- A namespace deleted from the cluster stays in the grant; recreating it restores access. Documented in the README.
- A set naming no existing namespace gives empty lists, not errors.
- The last-admin guards (cannot demote or delete the last admin) are unchanged; `grant()` short-circuits for admins, so binding data can never lock an admin out.
- Environment kind is fixed at creation, so a grant with namespaces never ends up on a Docker environment.
- Tasks started under a grant that is later revoked remain readable by their owner; they carry no environment data beyond what the owner already saw.

## 7. Testing

- **Unit (`authz`)**: matrix of {admin, viewer, operator, none} × {read, write} × {whole, limited} × {in-set, out-of-set, Node, Namespace, other cluster-scoped}.
- **Store**: migration backfill on a populated 0004 database; cascades on user/env delete; upsert; namespaces refused for Docker envs; admin bindings ignored then restored on demotion.
- **API (in-process, fake Docker and fake kube)**: unbound user gets 404 and does not see the env in `/envs`; viewer 403 on each write route family; all-namespace list filtered; apply with mixed documents refuses only the out-of-set ones without sending them; watch subscription filtered and `ACCESS_CHANGED` ends it; management API validation and audit rows.
- **kind cluster (`kube_it`, `e2e-kube`)**: a limited operator sees only their namespace's pods live while a pod is created elsewhere by `kubectl`; a two-document apply across namespaces commits one and refuses one.
- **Web (vitest)**: Access dialog flows; viewer sees no write buttons; empty state; `ACCESS_CHANGED` invalidates and resubscribes.
- **Playwright**: admin grants viewer on the Docker env to a new user; that user logs in, sees the env read-only, and does not see the other env.
- **rss-bench**: watch phase with several limited users on the same kinds shows the same upstream watch count as one user.

## 8. Out of scope

Teams and groups, custom roles, environment-admin delegation, API tokens, SSO/LDAP, non-admin audit log views, Docker sub-scoping (per-container or per-Compose-project), label-selector scoping in Kubernetes, Kubernetes impersonation.

## 9. Risks

| Risk | Mitigation |
|---|---|
| A route forgets the namespace check | All Kubernetes handlers get the grant from `kube_for`; API tests cover each route family with an out-of-set namespace. |
| Cluster-wide fetches return data the user may not see | Filtered before serialization; never logged. Documented that FleetOps, not the cluster, enforces the boundary. |
| Stale grants on open sockets | `ACCESS_CHANGED` ends subscriptions; REST re-reads per request. |
| Upgrade surprises | Backfill preserves today's access exactly; migration test on a populated database. |
