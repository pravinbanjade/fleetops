# Dockerer P1 — Backend Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a single `dockerer` Rust binary that serves `/api/v1` with first-run setup, login/logout, revocable cookie sessions with CSRF, admin user management, audit logging, security headers, request ids, an embedded SPA fallback, and optional TLS. All state lives in SQLite.

**Architecture:** This is a Cargo workspace with four crates:
- `dockerer-core`: domain types and `Error`; no I/O.
- `dockerer-store`: sqlx/SQLite repositories and embedded migrations.
- `dockerer-api`: axum router, auth extractors, handlers.
- `dockerer-server`: CLI/config, static assets, TLS, and the process lifecycle; builds the `dockerer` bin.

Handlers talk to `Store` only. P2 adds the Docker environment layer behind the same `AppState`.

**Tech Stack:** Rust stable (edition 2021), tokio 1, axum 0.8, sqlx 0.8 (SQLite), argon2 0.5, rand 0.8, sha2 0.10, subtle 2, rust-embed 8, axum-server 0.7 + rustls 0.23 (ring) + rcgen 0.13, clap 4, tracing.

**Spec:** `docs/superpowers/specs/2026-09-29-dockerer-core-docker-design.md`

**Plan series for sub-project 1:**
- **P1 (this plan):** backend foundation.
- **P2:** Docker environment layer, resource REST endpoints, websocket hub, OpenAPI via utoipa (P2 also annotates P1's handlers).
- **P3:** React web UI, e2e, image build, CI.

## Global Constraints

- API base path is `/api/v1`. All responses carry `X-Request-Id`. Error body is `{code, message, request_id}`.
- Passwords are hashed with argon2id (argon2 crate defaults). Hashing and verification run in `spawn_blocking`.
- Session cookie is `dockerer_session`, a random 256-bit hex token. Only its SHA-256 is stored.
  - Flags: `HttpOnly; SameSite=Strict; Path=/`, plus `Secure` only when the request arrived over TLS.
  - Sessions expire after 7 days idle (`SESSION_IDLE_SECS = 604800`) or 30 days absolute (`SESSION_ABSOLUTE_SECS = 2592000`).
- Every authenticated non-GET/HEAD/OPTIONS request must send an `X-CSRF-Token` header equal to the session's CSRF token. Missing or wrong gives 403.
- Login rate limit is 5 failures per lowercased username + client IP per 5 minutes, then 429 with `Retry-After`.
- First-run setup is only possible while no users exist and within 10 minutes of process start. After that it is locked until restart.
- Roles are `admin` and `user`. Every user has a global `role_bindings` row mirroring `users.role`.
- Audit rows are written for every mutating auth/admin action. Retention defaults to 90 days (`settings` key `audit.retention_days`) and pruning runs hourly.
- SQLite runs in WAL mode with foreign keys on. The DB file is `<data>/dockerer.db`.
- CLI flags and env vars exactly as in spec §2.3: `--data/DOCKERER_DATA=/data`, `--bind/DOCKERER_BIND=0.0.0.0:9000`, `--tls-bind/DOCKERER_TLS_BIND`, `--tls-cert/DOCKERER_TLS_CERT`, `--tls-key/DOCKERER_TLS_KEY`, `--docker-host/DOCKER_HOST=unix:///var/run/docker.sock`, `--log-level/DOCKERER_LOG=info`.
- No telemetry, no outbound network calls.
- **Spec deltas (intentional, small):**
  - `Error::Unauthorized` and `Error::Forbidden` carry a message.
  - New `Error::RateLimited { retry_after_secs }` maps to 429.
  - New endpoints: `GET /api/v1/health`, `GET /api/v1/auth/status` (lets the UI route to `/setup`), and `PUT /api/v1/auth/password` (self-service password change).
  - `Secure` is set per request (by listener) instead of globally, so the HTTP and HTTPS listeners both work.

## Review Focus

1. **Concurrent first-run setup requests**: exactly one admin is created and the rest get 409. Pinned in Task 7 (`concurrent_setup_creates_exactly_one_admin`) and at the store level in Task 2 (`first_admin_only_once`).
2. **Removing or demoting the last admin** is refused with 409 rather than locking everyone out. Pinned in Task 2 (`cannot_demote_or_delete_last_admin`) and Task 9 (`last_admin_is_protected`).
3. **Username case variants** (`Admin` vs `admin`) are the same account for uniqueness, login and rate-limit keys. Pinned in Task 2 (`duplicate_username_conflicts_case_insensitively`) and Task 7 (`login_is_case_insensitive_and_rate_limit_shares_key`).
4. **HTTP and HTTPS listeners enabled together**: login works on both, and the `Secure` cookie flag and HSTS appear only on TLS. Pinned in Task 7 (`cookie_is_secure_only_over_tls`) and Task 5 (`hsts_only_over_tls`).
5. **Username-spraying the login endpoint** keeps limiter memory bounded (stale entries pruned at 10k keys), and oversize passwords (>256 chars) are rejected before hashing. Pinned in Task 6 (`map_is_bounded`) and Task 1 (`password_length_bounds`).

---

## File Structure

```
Cargo.toml                                   workspace + shared dependency versions
rust-toolchain.toml                          stable + rustfmt + clippy
Makefile                                     dev-server / test / lint / fmt / build
.gitignore                                   (modify) add .dev-data/
crates/dockerer-core/
  Cargo.toml
  src/lib.rs                                 re-exports, now_unix()
  src/error.rs                               Error enum, code(), Result
  src/user.rs                                Role, User, validate_username/password
crates/dockerer-store/
  Cargo.toml
  migrations/0001_init.sql                   users, sessions, environments, role_bindings, settings, audit_log
  src/lib.rs                                 Store (open, in_memory), db_err
  src/users.rs                               user repository
  src/sessions.rs                            NewSession, SessionRecord, session repository
  src/audit.rs                               NewAudit, AuditEntry, audit repository
  src/settings.rs                            settings repository, audit_retention_days
  tests/users.rs  tests/sessions.rs  tests/audit_settings.rs  tests/open.rs
crates/dockerer-api/
  Cargo.toml
  src/lib.rs                                 app(), IsTls, re-exports
  src/state.rs                               ApiConfig, AppState
  src/error.rs                               ApiError, ApiResult, IntoResponse
  src/request_id.rs                          task-local request id + middleware
  src/headers.rs                             security headers middleware
  src/auth/mod.rs                            constants
  src/auth/password.rs                       hash/verify/burn (argon2id)
  src/auth/token.rs                          generate_token, hash_token, constant_time_eq
  src/auth/cookie.rs                         read_cookie, session_cookie, clear_cookie
  src/auth/limiter.rs                        LoginLimiter, limiter_key
  src/auth/setup.rs                          SetupGuard
  src/auth/extract.rs                        ClientMeta, CurrentUser, AdminUser
  src/routes/mod.rs                          api() router, audit() helper, health, fallback
  src/routes/auth.rs                         status/setup/login/logout/me/sessions/password
  src/routes/users.rs                        admin user CRUD
  src/routes/audit_log.rs                    admin audit list
  tests/common/mod.rs  tests/basics.rs  tests/auth.rs  tests/sessions.rs  tests/users.rs
crates/dockerer-server/
  Cargo.toml
  build.rs                                   ensures web/dist/index.html placeholder exists
  src/lib.rs                                 run(), serve(), shutdown
  src/main.rs                                parse Config, init tracing, run
  src/config.rs                              clap Config
  src/static_files.rs                        rust-embed SPA handler
  src/maintenance.rs                         hourly prune of sessions/audit
  src/tls.rs                                 load_or_generate, generate_self_signed
  tests/static_files.rs  tests/maintenance.rs  tests/tls.rs
```

---

### Task 1: Toolchain, workspace, and `dockerer-core`

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `Makefile`
- Modify: `.gitignore`
- Create: `crates/dockerer-core/Cargo.toml`, `crates/dockerer-core/src/lib.rs`, `crates/dockerer-core/src/error.rs`, `crates/dockerer-core/src/user.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `dockerer_core::Error` with variants `NotFound(String)`, `Conflict(String)`, `Invalid(String)`, `Unauthorized(String)`, `Forbidden(String)`, `RateLimited { retry_after_secs: u64 }`, `EnvUnavailable(String)`, `Upstream { status: u16, message: String }`, `Internal(String)`.
  - `Error::code(&self) -> &'static str`.
  - `dockerer_core::Result<T>`.
  - `Role { Admin, User }` with `as_str()`, `FromStr`, and serde lowercase.
  - `User { id: i64, username: String, role: Role, created_at: i64, updated_at: i64 }` (Serialize).
  - `validate_username(&str) -> Result<()>`, `validate_password(&str) -> Result<()>`, `now_unix() -> i64`.

- [ ] **Step 1: Install the Rust toolchain (not present on this machine)**

Run:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
rustup component add rustfmt clippy
cargo --version
```
Expected: prints `cargo 1.x.y`.

- [ ] **Step 2: Create workspace files**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.85"

[workspace.dependencies]
dockerer-core = { path = "crates/dockerer-core" }
dockerer-store = { path = "crates/dockerer-store" }
dockerer-api = { path = "crates/dockerer-api" }

anyhow = "1"
argon2 = { version = "0.5", features = ["std"] }
axum = "0.8"
axum-server = { version = "0.7", features = ["tls-rustls-no-provider"] }
clap = { version = "4", features = ["derive", "env"] }
hex = "0.4"
http-body-util = "0.1"
mime_guess = "2"
rand = "0.8"
rcgen = "0.13"
rust-embed = { version = "8", features = ["interpolate-folder-path"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "migrate", "macros"] }
subtle = "2"
tempfile = "3"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tower = { version = "0.5", features = ["util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
uuid = { version = "1", features = ["v4"] }
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`Makefile` (recipe lines are indented with a TAB):
```make
.PHONY: dev-server test lint fmt build

dev-server:
	cargo run -p dockerer-server -- --data ./.dev-data --bind 127.0.0.1:9000

test:
	cargo test --workspace

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

build:
	cargo build --release -p dockerer-server
```

Append to `.gitignore`:
```
.dev-data/
```

- [ ] **Step 3: Create `dockerer-core` with failing tests**

`crates/dockerer-core/Cargo.toml`:
```toml
[package]
name = "dockerer-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

`crates/dockerer-core/src/lib.rs`:
```rust
//! Domain types shared by every dockerer crate. No I/O lives here.

pub mod error;
pub mod user;

pub use error::{Error, Result};
pub use user::{validate_password, validate_username, Role, User};

/// Current wall-clock time as unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

`crates/dockerer-core/src/error.rs` (tests first; the enum is added in Step 5):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Error::NotFound("x".into()).code(), "not_found");
        assert_eq!(Error::Conflict("x".into()).code(), "conflict");
        assert_eq!(Error::Invalid("x".into()).code(), "invalid");
        assert_eq!(Error::Unauthorized("x".into()).code(), "unauthorized");
        assert_eq!(Error::Forbidden("x".into()).code(), "forbidden");
        assert_eq!(Error::RateLimited { retry_after_secs: 1 }.code(), "rate_limited");
        assert_eq!(Error::EnvUnavailable("x".into()).code(), "env_unavailable");
        assert_eq!(Error::Upstream { status: 409, message: "x".into() }.code(), "upstream");
        assert_eq!(Error::Internal("x".into()).code(), "internal");
    }

    #[test]
    fn display_keeps_upstream_message_verbatim() {
        let e = Error::Upstream { status: 409, message: "container is running".into() };
        assert_eq!(e.to_string(), "container is running");
        assert_eq!(Error::NotFound("user 5".into()).to_string(), "not found: user 5");
        assert_eq!(
            Error::RateLimited { retry_after_secs: 30 }.to_string(),
            "too many attempts; retry in 30s"
        );
    }
}
```

`crates/dockerer-core/src/user.rs` (tests first; the code is added in Step 5):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip() {
        assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("user".parse::<Role>().unwrap(), Role::User);
        assert_eq!(Role::Admin.as_str(), "admin");
        assert!(matches!("root".parse::<Role>(), Err(Error::Invalid(_))));
    }

    #[test]
    fn role_serde_is_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        let r: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(r, Role::User);
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("a.b-c_1").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username("ünicode").is_err());
        assert!(validate_username(&"a".repeat(64)).is_ok());
        assert!(validate_username(&"a".repeat(65)).is_err());
    }

    #[test]
    fn password_length_bounds() {
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password(&"x".repeat(256)).is_ok());
        assert!(validate_password(&"x".repeat(257)).is_err());
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p dockerer-core`
Expected: FAIL to compile with `cannot find type Error` / `Role` / `validate_username`.

- [ ] **Step 5: Implement**

Prepend to `crates/dockerer-core/src/error.rs` (above the tests module):
```rust
/// The single error type crossing crate boundaries. The API layer maps each
/// variant to an HTTP status and a stable `code` string.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("too many attempts; retry in {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("environment unavailable: {0}")]
    EnvUnavailable(String),
    /// An error reported by Docker; `message` is shown to the user verbatim.
    #[error("{message}")]
    Upstream { status: u16, message: String },
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Conflict(_) => "conflict",
            Error::Invalid(_) => "invalid",
            Error::Unauthorized(_) => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::RateLimited { .. } => "rate_limited",
            Error::EnvUnavailable(_) => "env_unavailable",
            Error::Upstream { .. } => "upstream",
            Error::Internal(_) => "internal",
        }
    }
}
```

Prepend to `crates/dockerer-core/src/user.rs`:
```rust
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const USERNAME_MAX: usize = 64;
pub const PASSWORD_MIN: usize = 8;
pub const PASSWORD_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

impl FromStr for Role {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            other => Err(Error::Invalid(format!("unknown role '{other}'"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > USERNAME_MAX {
        return Err(Error::Invalid(format!("username must be 1-{USERNAME_MAX} characters")));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err(Error::Invalid(
            "username may only contain letters, digits, '_', '-' and '.'".into(),
        ));
    }
    Ok(())
}

/// Upper bound keeps attacker-supplied input to the hasher small.
pub fn validate_password(password: &str) -> Result<()> {
    let n = password.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&n) {
        return Err(Error::Invalid(format!(
            "password must be {PASSWORD_MIN}-{PASSWORD_MAX} characters"
        )));
    }
    Ok(())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p dockerer-core && make lint`
Expected: 6 tests PASS; fmt and clippy clean.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml Makefile .gitignore crates/dockerer-core
git commit -m "feat(core): workspace scaffold, Error, Role, User and validation"
```

---

### Task 2: `dockerer-store`: schema, open, and users

**Files:**
- Create: `crates/dockerer-store/Cargo.toml`, `crates/dockerer-store/migrations/0001_init.sql`, `crates/dockerer-store/src/lib.rs`, `crates/dockerer-store/src/users.rs`
- Create (stubs, filled by Tasks 3–4): `crates/dockerer-store/src/sessions.rs`, `crates/dockerer-store/src/audit.rs`, `crates/dockerer-store/src/settings.rs`, each containing only `use crate::Store;` plus a comment `// implemented in a later task`, so `mod` declarations compile. Add `#[allow(unused_imports)]` on that `use` line.
- Test: `crates/dockerer-store/tests/users.rs`, `crates/dockerer-store/tests/open.rs`

**Interfaces:**
- Consumes: `dockerer_core::{Error, Result, Role, User}`.
- Produces:
  - `Store` (Clone): `Store::open(&Path) -> Result<Store>`, `Store::in_memory() -> Result<Store>`.
  - `create_user(&self, username: &str, password_hash: &str, role: Role, now: i64) -> Result<User>` (Conflict on duplicate, case-insensitive).
  - `create_first_admin(&self, username: &str, password_hash: &str, now: i64) -> Result<User>` (Conflict if any user exists).
  - `count_users(&self) -> Result<i64>`.
  - `find_credentials(&self, username: &str) -> Result<Option<(User, String)>>` (case-insensitive).
  - `get_user(&self, id: i64) -> Result<User>` (NotFound), `list_users(&self) -> Result<Vec<User>>`.
  - `set_user_role(&self, id: i64, role: Role, now: i64) -> Result<User>` (Conflict when demoting the last admin).
  - `set_password_hash(&self, id: i64, hash: &str, now: i64) -> Result<()>`.
  - `delete_user(&self, id: i64) -> Result<()>` (Conflict when deleting the last admin).
  - `global_role_binding(&self, user_id: i64) -> Result<Option<Role>>`.
  - `pub(crate) fn db_err(sqlx::Error) -> Error`.

- [ ] **Step 1: Create crate, migration and failing tests**

`crates/dockerer-store/Cargo.toml`:
```toml
[package]
name = "dockerer-store"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
dockerer-core.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true

[dev-dependencies]
tempfile.workspace = true
tokio.workspace = true
```

`crates/dockerer-store/migrations/0001_init.sql`:
```sql
CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('admin', 'user')),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT    NOT NULL UNIQUE,
    csrf_token   TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    user_agent   TEXT,
    ip           TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);

CREATE TABLE environments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    kind       TEXT    NOT NULL,
    url        TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- environment_id NULL means a global binding. v1 keeps exactly one global
-- binding per user mirroring users.role; sub-project 5 adds per-env rows.
CREATE TABLE role_bindings (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    environment_id INTEGER REFERENCES environments(id) ON DELETE CASCADE,
    role           TEXT    NOT NULL
);
CREATE UNIQUE INDEX role_bindings_uniq ON role_bindings(user_id, IFNULL(environment_id, 0));

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE audit_log (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             INTEGER NOT NULL,
    user_id        INTEGER REFERENCES users(id) ON DELETE SET NULL,
    environment_id INTEGER REFERENCES environments(id) ON DELETE SET NULL,
    action         TEXT    NOT NULL,
    target_type    TEXT,
    target_id      TEXT,
    result         TEXT    NOT NULL,
    detail         TEXT
);
CREATE INDEX audit_ts ON audit_log(ts);
```

`crates/dockerer-store/tests/users.rs`:
```rust
use dockerer_core::{Error, Role};
use dockerer_store::Store;

async fn store() -> Store {
    Store::in_memory().await.unwrap()
}

#[tokio::test]
async fn create_and_find_user_case_insensitively() {
    let s = store().await;
    let u = s.create_user("Alice", "hash1", Role::User, 100).await.unwrap();
    assert_eq!(u.username, "Alice");
    assert_eq!(u.role, Role::User);
    assert_eq!(u.created_at, 100);

    let (found, hash) = s.find_credentials("alice").await.unwrap().unwrap();
    assert_eq!(found.id, u.id);
    assert_eq!(hash, "hash1");
    assert!(s.find_credentials("bob").await.unwrap().is_none());
}

#[tokio::test]
async fn duplicate_username_conflicts_case_insensitively() {
    let s = store().await;
    s.create_user("alice", "h", Role::User, 0).await.unwrap();
    let err = s.create_user("ALICE", "h", Role::User, 0).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn first_admin_only_once() {
    let s = store().await;
    let a = s.create_first_admin("admin", "h", 1).await.unwrap();
    assert_eq!(a.role, Role::Admin);
    let err = s.create_first_admin("other", "h", 1).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    assert_eq!(s.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn first_admin_refused_when_any_user_exists() {
    let s = store().await;
    s.create_user("bob", "h", Role::User, 0).await.unwrap();
    assert!(matches!(s.create_first_admin("admin", "h", 1).await, Err(Error::Conflict(_))));
}

#[tokio::test]
async fn cannot_demote_or_delete_last_admin() {
    let s = store().await;
    let a = s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    assert!(matches!(s.set_user_role(a.id, Role::User, 1).await, Err(Error::Conflict(_))));
    assert!(matches!(s.delete_user(a.id).await, Err(Error::Conflict(_))));

    let b = s.create_user("b", "h", Role::Admin, 0).await.unwrap();
    let demoted = s.set_user_role(a.id, Role::User, 2).await.unwrap();
    assert_eq!(demoted.role, Role::User);
    assert_eq!(demoted.updated_at, 2);
    assert!(matches!(s.delete_user(b.id).await, Err(Error::Conflict(_))));
    s.delete_user(a.id).await.unwrap();
    assert_eq!(s.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn missing_user_is_not_found() {
    let s = store().await;
    assert!(matches!(s.get_user(999).await, Err(Error::NotFound(_))));
    assert!(matches!(s.set_user_role(999, Role::User, 0).await, Err(Error::NotFound(_))));
    assert!(matches!(s.set_password_hash(999, "h", 0).await, Err(Error::NotFound(_))));
    assert!(matches!(s.delete_user(999).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn set_password_hash_updates_hash_and_timestamp() {
    let s = store().await;
    let u = s.create_user("alice", "old", Role::User, 0).await.unwrap();
    s.set_password_hash(u.id, "new", 50).await.unwrap();
    let (found, hash) = s.find_credentials("alice").await.unwrap().unwrap();
    assert_eq!(hash, "new");
    assert_eq!(found.updated_at, 50);
}

#[tokio::test]
async fn list_users_ordered_by_id() {
    let s = store().await;
    s.create_user("b", "h", Role::User, 0).await.unwrap();
    s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    let names: Vec<_> = s.list_users().await.unwrap().into_iter().map(|u| u.username).collect();
    assert_eq!(names, vec!["b", "a"]);
}

#[tokio::test]
async fn global_role_binding_mirrors_role() {
    let s = store().await;
    let a = s.create_user("a", "h", Role::Admin, 0).await.unwrap();
    let u = s.create_user("u", "h", Role::User, 0).await.unwrap();
    assert_eq!(s.global_role_binding(u.id).await.unwrap(), Some(Role::User));
    s.set_user_role(u.id, Role::Admin, 1).await.unwrap();
    assert_eq!(s.global_role_binding(u.id).await.unwrap(), Some(Role::Admin));
    let first = s.create_first_admin("x", "h", 0).await;
    assert!(first.is_err());
    assert_eq!(s.global_role_binding(a.id).await.unwrap(), Some(Role::Admin));
}
```

`crates/dockerer-store/tests/open.rs`:
```rust
use dockerer_core::Role;
use dockerer_store::Store;

#[tokio::test]
async fn file_store_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dockerer.db");
    {
        let s = Store::open(&path).await.unwrap();
        s.create_user("alice", "h", Role::Admin, 0).await.unwrap();
    }
    let s = Store::open(&path).await.unwrap();
    assert_eq!(s.count_users().await.unwrap(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-store`
Expected: FAIL to compile (`Store` not found).

- [ ] **Step 3: Implement `lib.rs` and `users.rs`**

`crates/dockerer-store/src/lib.rs`:
```rust
//! SQLite persistence for dockerer. All timestamps are unix seconds passed
//! in by the caller so that behaviour is deterministic under test.

mod audit;
mod sessions;
mod settings;
mod users;

use dockerer_core::{Error, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::{path::Path, str::FromStr, time::Duration};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::init(pool).await
    }

    /// Single-connection in-memory database for tests. The connection must
    /// never be recycled or the database disappears.
    pub async fn in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(db_err)?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::init(pool).await
    }

    async fn init(pool: SqlitePool) -> Result<Self> {
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| Error::Internal(format!("migration failed: {e}")))?;
        Ok(Self { pool })
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::RowNotFound => Error::NotFound("record not found".into()),
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict("already exists".into())
        }
        _ => Error::Internal(format!("database error: {e}")),
    }
}
```

`crates/dockerer-store/src/users.rs`:
```rust
use crate::{db_err, Store};
use dockerer_core::{Error, Result, Role, User};
use sqlx::SqliteConnection;

const COLS: &str = "id, username, password_hash, role, created_at, updated_at";

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    password_hash: String,
    role: String,
    created_at: i64,
    updated_at: i64,
}

impl UserRow {
    fn into_parts(self) -> Result<(User, String)> {
        let role = self.role.parse::<Role>()?;
        let user = User {
            id: self.id,
            username: self.username,
            role,
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        Ok((user, self.password_hash))
    }

    fn into_user(self) -> Result<User> {
        self.into_parts().map(|(u, _)| u)
    }
}

async fn insert_global_binding(conn: &mut SqliteConnection, user_id: i64, role: Role) -> Result<()> {
    sqlx::query("INSERT INTO role_bindings (user_id, environment_id, role) VALUES (?, NULL, ?)")
        .bind(user_id)
        .bind(role.as_str())
        .execute(conn)
        .await
        .map_err(db_err)?;
    Ok(())
}

impl Store {
    pub async fn create_user(&self, username: &str, password_hash: &str, role: Role, now: i64) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query_as::<_, UserRow>(&format!(
            "INSERT INTO users (username, password_hash, role, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?) RETURNING {COLS}"
        ))
        .bind(username)
        .bind(password_hash)
        .bind(role.as_str())
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        insert_global_binding(&mut tx, row.id, role).await?;
        tx.commit().await.map_err(db_err)?;
        row.into_user()
    }

    /// Creates the initial admin in a single conditional INSERT so that two
    /// concurrent setup requests can never both succeed.
    pub async fn create_first_admin(&self, username: &str, password_hash: &str, now: i64) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query_as::<_, UserRow>(&format!(
            "INSERT INTO users (username, password_hash, role, created_at, updated_at)
             SELECT ?, ?, 'admin', ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)
             RETURNING {COLS}"
        ))
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(Error::Conflict("setup already completed".into()));
        };
        insert_global_binding(&mut tx, row.id, Role::Admin).await?;
        tx.commit().await.map_err(db_err)?;
        row.into_user()
    }

    pub async fn count_users(&self) -> Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn find_credentials(&self, username: &str) -> Result<Option<(User, String)>> {
        let row = sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users WHERE username = ?"))
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(UserRow::into_parts).transpose()
    }

    pub async fn get_user(&self, id: i64) -> Result<User> {
        let row = sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound(format!("user {id}")))?.into_user()
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
            .into_iter()
            .map(UserRow::into_user)
            .collect()
    }

    /// The last-admin guard is part of the UPDATE statement itself, so it is
    /// atomic with respect to concurrent demotions.
    pub async fn set_user_role(&self, id: i64, role: Role, now: i64) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let res = sqlx::query(
            "UPDATE users SET role = ?1, updated_at = ?2
             WHERE id = ?3
               AND NOT (role = 'admin' AND ?1 <> 'admin'
                        AND (SELECT COUNT(*) FROM users WHERE role = 'admin') <= 1)",
        )
        .bind(role.as_str())
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            drop(tx);
            self.get_user(id).await?;
            return Err(Error::Conflict("cannot demote the last admin".into()));
        }
        sqlx::query("UPDATE role_bindings SET role = ? WHERE user_id = ? AND environment_id IS NULL")
            .bind(role.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.get_user(id).await
    }

    pub async fn set_password_hash(&self, id: i64, password_hash: &str, now: i64) -> Result<()> {
        let res = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("user {id}")));
        }
        Ok(())
    }

    pub async fn delete_user(&self, id: i64) -> Result<()> {
        let res = sqlx::query(
            "DELETE FROM users WHERE id = ?1
               AND NOT (role = 'admin' AND (SELECT COUNT(*) FROM users WHERE role = 'admin') <= 1)",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            self.get_user(id).await?;
            return Err(Error::Conflict("cannot delete the last admin".into()));
        }
        Ok(())
    }

    pub async fn global_role_binding(&self, user_id: i64) -> Result<Option<Role>> {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM role_bindings WHERE user_id = ? AND environment_id IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        role.map(|r| r.parse()).transpose()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dockerer-store && make lint`
Expected: 10 tests PASS; lint clean.

- [ ] **Step 5: Commit**

```bash
git add crates/dockerer-store Cargo.lock
git commit -m "feat(store): sqlite schema, Store::open, user repository with last-admin guard"
```

---

### Task 3: Store sessions

**Files:**
- Modify: `crates/dockerer-store/src/sessions.rs` (replace stub)
- Modify: `crates/dockerer-store/src/lib.rs` (add `pub use sessions::{NewSession, SessionRecord};`)
- Test: `crates/dockerer-store/tests/sessions.rs`

**Interfaces:**
- Consumes: `Store`, `db_err`.
- Produces:
  - `NewSession<'a> { user_id: i64, token_hash: &'a str, csrf_token: &'a str, now: i64, expires_at: i64, user_agent: Option<&'a str>, ip: Option<&'a str> }`.
  - `SessionRecord { id, user_id, csrf_token: String, created_at, expires_at, last_seen_at, user_agent: Option<String>, ip: Option<String> }` with `is_active(&self, now: i64, idle_timeout: i64) -> bool`.
  - `Store` methods:
    - `create_session(&NewSession) -> Result<i64>`
    - `find_session(token_hash) -> Result<Option<SessionRecord>>`
    - `touch_session(id, now) -> Result<()>`
    - `list_sessions(user_id) -> Result<Vec<SessionRecord>>`
    - `delete_session(id) -> Result<()>`
    - `delete_user_session(user_id, id) -> Result<bool>`
    - `delete_user_sessions(user_id, except: Option<i64>) -> Result<u64>`
    - `delete_expired_sessions(now, idle_timeout) -> Result<u64>`

- [ ] **Step 1: Write failing tests**

`crates/dockerer-store/tests/sessions.rs`:
```rust
use dockerer_core::Role;
use dockerer_store::{NewSession, SessionRecord, Store};

async fn setup() -> (Store, i64) {
    let s = Store::in_memory().await.unwrap();
    let id = s.create_user("alice", "h", Role::User, 0).await.unwrap().id;
    (s, id)
}

fn new(user_id: i64, hash: &str, now: i64) -> NewSession<'_> {
    NewSession {
        user_id,
        token_hash: hash,
        csrf_token: "csrf",
        now,
        expires_at: now + 1000,
        user_agent: Some("ua"),
        ip: Some("1.2.3.4"),
    }
}

#[tokio::test]
async fn create_find_touch() {
    let (s, uid) = setup().await;
    let id = s.create_session(&new(uid, "h1", 10)).await.unwrap();
    let rec = s.find_session("h1").await.unwrap().unwrap();
    assert_eq!(
        rec,
        SessionRecord {
            id,
            user_id: uid,
            csrf_token: "csrf".into(),
            created_at: 10,
            expires_at: 1010,
            last_seen_at: 10,
            user_agent: Some("ua".into()),
            ip: Some("1.2.3.4".into()),
        }
    );
    assert!(s.find_session("nope").await.unwrap().is_none());
    s.touch_session(id, 50).await.unwrap();
    assert_eq!(s.find_session("h1").await.unwrap().unwrap().last_seen_at, 50);
}

#[test]
fn is_active_respects_absolute_and_idle_expiry() {
    let rec = SessionRecord {
        id: 1,
        user_id: 1,
        csrf_token: String::new(),
        created_at: 0,
        expires_at: 1000,
        last_seen_at: 0,
        user_agent: None,
        ip: None,
    };
    assert!(rec.is_active(10, 100));
    assert!(!rec.is_active(100, 100), "idle exactly at timeout is expired");
    assert!(!rec.is_active(1000, 5000), "absolute expiry");
}

#[tokio::test]
async fn delete_user_session_is_scoped_to_owner() {
    let (s, a) = setup().await;
    let b = s.create_user("bob", "h", Role::User, 0).await.unwrap().id;
    let sid = s.create_session(&new(a, "ha", 0)).await.unwrap();
    assert!(!s.delete_user_session(b, sid).await.unwrap());
    assert!(s.delete_user_session(a, sid).await.unwrap());
    assert!(s.find_session("ha").await.unwrap().is_none());
}

#[tokio::test]
async fn delete_user_sessions_except_current() {
    let (s, uid) = setup().await;
    let keep = s.create_session(&new(uid, "h1", 0)).await.unwrap();
    s.create_session(&new(uid, "h2", 0)).await.unwrap();
    s.create_session(&new(uid, "h3", 0)).await.unwrap();
    assert_eq!(s.delete_user_sessions(uid, Some(keep)).await.unwrap(), 2);
    let left: Vec<_> = s.list_sessions(uid).await.unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(left, vec![keep]);
    assert_eq!(s.delete_user_sessions(uid, None).await.unwrap(), 1);
}

#[tokio::test]
async fn delete_expired_sessions_removes_absolute_and_idle() {
    let (s, uid) = setup().await;
    s.create_session(&NewSession { expires_at: 1000, ..new(uid, "expired", 0) }).await.unwrap();
    let idle = s.create_session(&NewSession { expires_at: 9000, ..new(uid, "idle", 0) }).await.unwrap();
    let fresh = s.create_session(&NewSession { expires_at: 9000, ..new(uid, "fresh", 0) }).await.unwrap();
    s.touch_session(idle, 400).await.unwrap();
    s.touch_session(fresh, 900).await.unwrap();
    assert_eq!(s.delete_expired_sessions(1000, 500).await.unwrap(), 2);
    assert!(s.find_session("fresh").await.unwrap().is_some());
}

#[tokio::test]
async fn deleting_user_cascades_sessions() {
    let (s, uid) = setup().await;
    s.create_session(&new(uid, "h1", 0)).await.unwrap();
    s.delete_user(uid).await.unwrap();
    assert!(s.find_session("h1").await.unwrap().is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-store --test sessions`
Expected: FAIL to compile (`NewSession` not found in `dockerer_store`).

- [ ] **Step 3: Implement**

`crates/dockerer-store/src/sessions.rs`:
```rust
use crate::{db_err, Store};
use dockerer_core::Result;

pub struct NewSession<'a> {
    pub user_id: i64,
    pub token_hash: &'a str,
    pub csrf_token: &'a str,
    pub now: i64,
    pub expires_at: i64,
    pub user_agent: Option<&'a str>,
    pub ip: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: i64,
    pub user_id: i64,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

impl SessionRecord {
    pub fn is_active(&self, now: i64, idle_timeout: i64) -> bool {
        now < self.expires_at && now - self.last_seen_at < idle_timeout
    }
}

const COLS: &str = "id, user_id, csrf_token, created_at, expires_at, last_seen_at, user_agent, ip";

impl Store {
    pub async fn create_session(&self, s: &NewSession<'_>) -> Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO sessions (user_id, token_hash, csrf_token, created_at, expires_at, last_seen_at, user_agent, ip)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(s.user_id)
        .bind(s.token_hash)
        .bind(s.csrf_token)
        .bind(s.now)
        .bind(s.expires_at)
        .bind(s.now)
        .bind(s.user_agent)
        .bind(s.ip)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn find_session(&self, token_hash: &str) -> Result<Option<SessionRecord>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM sessions WHERE token_hash = ?"))
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn touch_session(&self, id: i64, now: i64) -> Result<()> {
        sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn list_sessions(&self, user_id: i64) -> Result<Vec<SessionRecord>> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM sessions WHERE user_id = ? ORDER BY last_seen_at DESC, id DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn delete_session(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Returns false when the session does not exist or belongs to someone else.
    pub async fn delete_user_session(&self, user_id: i64, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM sessions WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn delete_user_sessions(&self, user_id: i64, except: Option<i64>) -> Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND (? IS NULL OR id <> ?)")
            .bind(user_id)
            .bind(except)
            .bind(except)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    pub async fn delete_expired_sessions(&self, now: i64, idle_timeout: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR last_seen_at <= ?")
            .bind(now)
            .bind(now - idle_timeout)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }
}
```

Add to `crates/dockerer-store/src/lib.rs` below the `mod` lines:
```rust
pub use sessions::{NewSession, SessionRecord};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dockerer-store && make lint`
Expected: all store tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dockerer-store
git commit -m "feat(store): session repository with idle/absolute expiry"
```

---

### Task 4: Store audit log and settings

**Files:**
- Modify: `crates/dockerer-store/src/audit.rs`, `crates/dockerer-store/src/settings.rs` (replace stubs)
- Modify: `crates/dockerer-store/src/lib.rs` (add `pub use audit::{AuditEntry, NewAudit}; pub use settings::AUDIT_RETENTION_KEY;`)
- Test: `crates/dockerer-store/tests/audit_settings.rs`

**Interfaces:**
- Consumes: `Store`, `db_err`.
- Produces:
  - `NewAudit<'a> { ts: i64, user_id: Option<i64>, environment_id: Option<i64>, action: &'a str, target_type: Option<&'a str>, target_id: Option<&'a str>, result: &'a str, detail: Option<serde_json::Value> }`.
  - `AuditEntry { id, ts, user_id: Option<i64>, username: Option<String>, environment_id: Option<i64>, action: String, target_type: Option<String>, target_id: Option<String>, result: String, detail: Option<serde_json::Value> }` (Serialize).
  - `Store` methods:
    - `insert_audit(&NewAudit) -> Result<i64>`
    - `list_audit(limit: i64, before_id: Option<i64>) -> Result<Vec<AuditEntry>>` (newest first)
    - `prune_audit(older_than_ts: i64) -> Result<u64>`
    - `get_setting(&str) -> Result<Option<Value>>`
    - `set_setting(&str, &Value) -> Result<()>`
    - `audit_retention_days() -> Result<i64>` (default 90)
  - `AUDIT_RETENTION_KEY = "audit.retention_days"`.

- [ ] **Step 1: Write failing tests**

`crates/dockerer-store/tests/audit_settings.rs`:
```rust
use dockerer_core::Role;
use dockerer_store::{NewAudit, Store, AUDIT_RETENTION_KEY};
use serde_json::json;

fn entry(ts: i64, user_id: Option<i64>, action: &str) -> NewAudit<'_> {
    NewAudit {
        ts,
        user_id,
        environment_id: None,
        action,
        target_type: Some("user"),
        target_id: Some("7"),
        result: "success",
        detail: None,
    }
}

#[tokio::test]
async fn list_audit_is_newest_first_with_cursor_and_username() {
    let s = Store::in_memory().await.unwrap();
    let uid = s.create_user("alice", "h", Role::Admin, 0).await.unwrap().id;
    let id1 = s.insert_audit(&entry(1, Some(uid), "a.one")).await.unwrap();
    let id2 = s.insert_audit(&entry(2, None, "a.two")).await.unwrap();
    let id3 = s
        .insert_audit(&NewAudit { detail: Some(json!({"k": 1})), ..entry(3, Some(uid), "a.three") })
        .await
        .unwrap();

    let page = s.list_audit(2, None).await.unwrap();
    assert_eq!(page.iter().map(|e| e.id).collect::<Vec<_>>(), vec![id3, id2]);
    assert_eq!(page[0].username.as_deref(), Some("alice"));
    assert_eq!(page[0].detail, Some(json!({"k": 1})));
    assert_eq!(page[1].username, None);

    let next = s.list_audit(10, Some(id2)).await.unwrap();
    assert_eq!(next.iter().map(|e| e.id).collect::<Vec<_>>(), vec![id1]);
    assert_eq!(next[0].action, "a.one");
    assert_eq!(next[0].target_id.as_deref(), Some("7"));
}

#[tokio::test]
async fn prune_audit_removes_older_rows() {
    let s = Store::in_memory().await.unwrap();
    s.insert_audit(&entry(10, None, "old")).await.unwrap();
    s.insert_audit(&entry(20, None, "new")).await.unwrap();
    assert_eq!(s.prune_audit(15).await.unwrap(), 1);
    let left = s.list_audit(10, None).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].action, "new");
}

#[tokio::test]
async fn audit_survives_user_deletion() {
    let s = Store::in_memory().await.unwrap();
    s.create_user("admin", "h", Role::Admin, 0).await.unwrap();
    let uid = s.create_user("bob", "h", Role::User, 0).await.unwrap().id;
    s.insert_audit(&entry(1, Some(uid), "x")).await.unwrap();
    s.delete_user(uid).await.unwrap();
    let rows = s.list_audit(10, None).await.unwrap();
    assert_eq!(rows[0].user_id, None);
}

#[tokio::test]
async fn settings_roundtrip_and_retention_default() {
    let s = Store::in_memory().await.unwrap();
    assert_eq!(s.get_setting("x").await.unwrap(), None);
    s.set_setting("x", &json!({"a": 1})).await.unwrap();
    s.set_setting("x", &json!({"a": 2})).await.unwrap();
    assert_eq!(s.get_setting("x").await.unwrap(), Some(json!({"a": 2})));

    assert_eq!(s.audit_retention_days().await.unwrap(), 90);
    s.set_setting(AUDIT_RETENTION_KEY, &json!(30)).await.unwrap();
    assert_eq!(s.audit_retention_days().await.unwrap(), 30);
    s.set_setting(AUDIT_RETENTION_KEY, &json!("bogus")).await.unwrap();
    assert_eq!(s.audit_retention_days().await.unwrap(), 90);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-store --test audit_settings`
Expected: FAIL to compile (`NewAudit` not found).

- [ ] **Step 3: Implement**

`crates/dockerer-store/src/audit.rs`:
```rust
use crate::{db_err, Store};
use dockerer_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;

pub struct NewAudit<'a> {
    pub ts: i64,
    pub user_id: Option<i64>,
    pub environment_id: Option<i64>,
    pub action: &'a str,
    pub target_type: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub result: &'a str,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub environment_id: Option<i64>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub result: String,
    pub detail: Option<Value>,
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    ts: i64,
    user_id: Option<i64>,
    username: Option<String>,
    environment_id: Option<i64>,
    action: String,
    target_type: Option<String>,
    target_id: Option<String>,
    result: String,
    detail: Option<String>,
}

impl TryFrom<AuditRow> for AuditEntry {
    type Error = Error;

    fn try_from(r: AuditRow) -> Result<Self> {
        let detail = r
            .detail
            .map(|d| serde_json::from_str(&d))
            .transpose()
            .map_err(|e| Error::Internal(format!("corrupt audit detail: {e}")))?;
        Ok(AuditEntry {
            id: r.id,
            ts: r.ts,
            user_id: r.user_id,
            username: r.username,
            environment_id: r.environment_id,
            action: r.action,
            target_type: r.target_type,
            target_id: r.target_id,
            result: r.result,
            detail,
        })
    }
}

impl Store {
    pub async fn insert_audit(&self, a: &NewAudit<'_>) -> Result<i64> {
        sqlx::query_scalar(
            "INSERT INTO audit_log (ts, user_id, environment_id, action, target_type, target_id, result, detail)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(a.ts)
        .bind(a.user_id)
        .bind(a.environment_id)
        .bind(a.action)
        .bind(a.target_type)
        .bind(a.target_id)
        .bind(a.result)
        .bind(a.detail.as_ref().map(Value::to_string))
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    pub async fn list_audit(&self, limit: i64, before_id: Option<i64>) -> Result<Vec<AuditEntry>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT a.id, a.ts, a.user_id, u.username, a.environment_id, a.action,
                    a.target_type, a.target_id, a.result, a.detail
             FROM audit_log a LEFT JOIN users u ON u.id = a.user_id
             WHERE (? IS NULL OR a.id < ?)
             ORDER BY a.id DESC LIMIT ?",
        )
        .bind(before_id)
        .bind(before_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(AuditEntry::try_from).collect()
    }

    pub async fn prune_audit(&self, older_than_ts: i64) -> Result<u64> {
        let res = sqlx::query("DELETE FROM audit_log WHERE ts < ?")
            .bind(older_than_ts)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }
}
```

`crates/dockerer-store/src/settings.rs`:
```rust
use crate::{db_err, Store};
use dockerer_core::{Error, Result};
use serde_json::Value;

pub const AUDIT_RETENTION_KEY: &str = "audit.retention_days";
const DEFAULT_AUDIT_RETENTION_DAYS: i64 = 90;

impl Store {
    pub async fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        raw.map(|v| serde_json::from_str(&v))
            .transpose()
            .map_err(|e| Error::Internal(format!("corrupt setting {key}: {e}")))
    }

    pub async fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value.to_string())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn audit_retention_days(&self) -> Result<i64> {
        Ok(self
            .get_setting(AUDIT_RETENTION_KEY)
            .await?
            .and_then(|v| v.as_i64())
            .filter(|d| *d > 0)
            .unwrap_or(DEFAULT_AUDIT_RETENTION_DAYS))
    }
}
```

Add to `crates/dockerer-store/src/lib.rs`:
```rust
pub use audit::{AuditEntry, NewAudit};
pub use settings::AUDIT_RETENTION_KEY;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dockerer-store && make lint`
Expected: all store tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dockerer-store
git commit -m "feat(store): audit log with cursor paging and settings"
```

---

### Task 5: `dockerer-api` skeleton: error mapping, request ids, security headers, health

**Files:**
- Create: `crates/dockerer-api/Cargo.toml`, `src/lib.rs`, `src/state.rs`, `src/error.rs`, `src/request_id.rs`, `src/headers.rs`, `src/routes/mod.rs`
- Create stub: `src/auth/mod.rs` containing `pub mod limiter; pub mod setup;`, with `limiter.rs`/`setup.rs` holding the minimal types from Task 6 Step 3. **Do Task 6 Step 3 for those two files now** so `AppState` compiles; their tests are added in Task 6.
- Test: `crates/dockerer-api/tests/common/mod.rs`, `crates/dockerer-api/tests/basics.rs`

**Interfaces:**
- Consumes: `Store`, `dockerer_core::Error`, `auth::limiter::LoginLimiter`, `auth::setup::SetupGuard`.
- Produces:
  - `dockerer_api::app(state: AppState, static_files: axum::Router) -> axum::Router`.
  - `AppState { store: Store, config: Arc<ApiConfig>, limiter: Arc<LoginLimiter>, setup: Arc<SetupGuard> }` with `AppState::new(Store, ApiConfig)`.
  - `ApiConfig { setup_window: Duration }` (Default: 600 s).
  - `IsTls(pub bool)`: a request extension set by the server per listener.
  - `ApiError(pub Error)`: IntoResponse, `From<Error>`.
  - `ApiResult<T> = Result<T, ApiError>`.
  - `routes::api() -> Router<AppState>` and `routes::audit(&AppState, Option<i64>, action, target_type, target_id, result)`.

- [ ] **Step 1: Create crate manifest and test helpers**

`crates/dockerer-api/Cargo.toml`:
```toml
[package]
name = "dockerer-api"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
dockerer-core.workspace = true
dockerer-store.workspace = true
argon2.workspace = true
axum.workspace = true
hex.workspace = true
rand.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
subtle.workspace = true
tokio.workspace = true
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
http-body-util.workspace = true
tower.workspace = true
```

`crates/dockerer-api/tests/common/mod.rs`:
```rust
#![allow(dead_code)]

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
    Router,
};
use dockerer_api::{ApiConfig, AppState};
use dockerer_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower::ServiceExt;

pub const ADMIN: &str = "admin";
pub const ADMIN_PW: &str = "correct-horse";

pub struct TestApp {
    pub router: Router,
    pub store: Store,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub json: Value,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

pub async fn app() -> TestApp {
    app_with(ApiConfig::default()).await
}

pub async fn app_with(config: ApiConfig) -> TestApp {
    let store = Store::in_memory().await.unwrap();
    let router = dockerer_api::app(AppState::new(store.clone(), config), Router::new());
    TestApp { router, store }
}

pub fn req(method: &str, uri: &str, body: Option<Value>, session: Option<&Session>) -> Request<Body> {
    req_from([127, 0, 0, 1], method, uri, body, session)
}

pub fn req_from(
    ip: [u8; 4],
    method: &str,
    uri: &str,
    body: Option<Value>,
    session: Option<&Session>,
) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(s) = session {
        b = b.header("cookie", &s.cookie).header("x-csrf-token", &s.csrf);
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut r = b.body(body).unwrap();
    r.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip, 40000))));
    r
}

pub fn session_from(r: &Resp) -> Session {
    let set = r.headers.get("set-cookie").expect("set-cookie").to_str().unwrap();
    Session {
        cookie: set.split(';').next().unwrap().to_string(),
        csrf: r.json["csrf_token"].as_str().expect("csrf_token").to_string(),
    }
}

impl TestApp {
    pub async fn send(&self, r: Request<Body>) -> Resp {
        let res = self.router.clone().oneshot(r).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        Resp { status, headers, json }
    }

    pub async fn setup_admin(&self) -> Session {
        let r = self
            .send(req(
                "POST",
                "/api/v1/auth/setup",
                Some(json!({"username": ADMIN, "password": ADMIN_PW})),
                None,
            ))
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json);
        session_from(&r)
    }

    pub async fn login(&self, username: &str, password: &str) -> Resp {
        self.send(req(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": username, "password": password})),
            None,
        ))
        .await
    }

    pub async fn create_user(&self, admin: &Session, username: &str, password: &str, role: &str) -> Resp {
        self.send(req(
            "POST",
            "/api/v1/users",
            Some(json!({"username": username, "password": password, "role": role})),
            Some(admin),
        ))
        .await
    }
}
```

- [ ] **Step 2: Write failing tests**

`crates/dockerer-api/tests/basics.rs`:
```rust
mod common;

use axum::http::StatusCode;
use common::*;
use dockerer_api::IsTls;

#[tokio::test]
async fn health_ok_with_request_id_and_security_headers() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/health", None, None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["status"], "ok");
    assert!(r.headers.get("x-request-id").is_some());
    let csp = r.headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
    assert_eq!(r.headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(r.headers.get("cache-control").unwrap(), "no-store");
    assert!(r.headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn hsts_only_over_tls() {
    let app = app().await;
    let mut request = req("GET", "/api/v1/health", None, None);
    request.extensions_mut().insert(IsTls(true));
    let r = app.send(request).await;
    assert!(r.headers.get("strict-transport-security").is_some());
}

#[tokio::test]
async fn unknown_api_route_is_json_404_with_matching_request_id() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/nope", None, None)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json["code"], "not_found");
    assert_eq!(
        r.json["request_id"].as_str().unwrap(),
        r.headers.get("x-request-id").unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn client_request_id_is_propagated_when_safe() {
    let app = app().await;
    let mut request = req("GET", "/api/v1/health", None, None);
    request.headers_mut().insert("x-request-id", "abc-123".parse().unwrap());
    let r = app.send(request).await;
    assert_eq!(r.headers.get("x-request-id").unwrap(), "abc-123");

    let mut request = req("GET", "/api/v1/health", None, None);
    request.headers_mut().insert("x-request-id", "bad%20id".parse().unwrap());
    let r = app.send(request).await;
    assert_ne!(r.headers.get("x-request-id").unwrap(), "bad%20id");
}
```

Unit tests at the bottom of `crates/dockerer-api/src/error.rs` (the file body is added in Step 4):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn status_of(e: Error) -> StatusCode {
        ApiError(e).into_response().status()
    }

    #[test]
    fn maps_variants_to_status() {
        assert_eq!(status_of(Error::NotFound("x".into())), StatusCode::NOT_FOUND);
        assert_eq!(status_of(Error::Conflict("x".into())), StatusCode::CONFLICT);
        assert_eq!(status_of(Error::Invalid("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(status_of(Error::Unauthorized("x".into())), StatusCode::UNAUTHORIZED);
        assert_eq!(status_of(Error::Forbidden("x".into())), StatusCode::FORBIDDEN);
        assert_eq!(status_of(Error::RateLimited { retry_after_secs: 5 }), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(status_of(Error::EnvUnavailable("x".into())), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(status_of(Error::Upstream { status: 409, message: "x".into() }), StatusCode::CONFLICT);
        assert_eq!(status_of(Error::Upstream { status: 200, message: "x".into() }), StatusCode::BAD_GATEWAY);
        assert_eq!(status_of(Error::Internal("x".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn rate_limited_sets_retry_after() {
        let res = ApiError(Error::RateLimited { retry_after_secs: 42 }).into_response();
        assert_eq!(res.headers().get("retry-after").unwrap(), "42");
    }

    #[tokio::test]
    async fn internal_details_are_not_leaked() {
        use http_body_util::BodyExt;
        let res = ApiError(Error::Internal("db password is hunter2".into())).into_response();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("internal server error"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p dockerer-api`
Expected: FAIL to compile (`dockerer_api::app` / `ApiError` missing).

- [ ] **Step 4: Implement**

`crates/dockerer-api/src/lib.rs`:
```rust
//! HTTP API for dockerer. `app()` builds the complete router; the server
//! crate supplies the static-file router and sets `IsTls` per listener.

pub mod auth;
pub mod error;
mod headers;
mod request_id;
mod routes;
pub mod state;

use axum::{middleware::from_fn, Router};

pub use error::{ApiError, ApiResult};
pub use state::{ApiConfig, AppState};

/// Request extension telling handlers whether the connection is TLS.
/// Controls the cookie `Secure` flag and HSTS.
#[derive(Debug, Clone, Copy)]
pub struct IsTls(pub bool);

pub fn app(state: AppState, static_files: Router) -> Router {
    Router::new()
        .nest("/api/v1", routes::api().with_state(state))
        .merge(static_files)
        .layer(from_fn(headers::security_headers))
        .layer(from_fn(request_id::middleware))
}
```

`crates/dockerer-api/src/state.rs`:
```rust
use crate::auth::{limiter::LoginLimiter, setup::SetupGuard};
use dockerer_store::Store;
use std::{sync::Arc, time::{Duration, Instant}};

pub const LOGIN_MAX_FAILURES: u32 = 5;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// How long after process start first-run setup stays open.
    pub setup_window: Duration,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { setup_window: Duration::from_secs(600) }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: Arc<ApiConfig>,
    pub limiter: Arc<LoginLimiter>,
    pub setup: Arc<SetupGuard>,
}

impl AppState {
    pub fn new(store: Store, config: ApiConfig) -> Self {
        let setup = SetupGuard::new(Instant::now(), config.setup_window);
        Self {
            store,
            config: Arc::new(config),
            limiter: Arc::new(LoginLimiter::new(LOGIN_MAX_FAILURES, LOGIN_WINDOW)),
            setup: Arc::new(setup),
        }
    }
}
```

Prepend to `crates/dockerer-api/src/error.rs`:
```rust
use axum::{
    http::{header::RETRY_AFTER, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dockerer_core::Error;
use serde_json::json;

/// Wraps the domain error so it can be returned from handlers.
#[derive(Debug)]
pub struct ApiError(pub Error);

pub type ApiResult<T> = Result<T, ApiError>;

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Error::EnvUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Upstream { status, .. } => StatusCode::from_u16(*status)
                .ok()
                .filter(|s| s.is_client_error() || s.is_server_error())
                .unwrap_or(StatusCode::BAD_GATEWAY),
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match &self.0 {
            Error::Internal(detail) => {
                tracing::error!(error = %detail, "internal error");
                "internal server error".to_string()
            }
            other => other.to_string(),
        };
        let body = json!({
            "code": self.0.code(),
            "message": message,
            "request_id": crate::request_id::current(),
        });
        let mut res = (status, Json(body)).into_response();
        if let Error::RateLimited { retry_after_secs } = self.0 {
            res.headers_mut().insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
        }
        res
    }
}
```

Add `http-body-util` and `tokio` test usage: `http-body-util` is already a dev-dependency, so the `#[tokio::test]` unit test compiles.

`crates/dockerer-api/src/request_id.rs`:
```rust
use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use tracing::Instrument;

tokio::task_local! {
    static REQUEST_ID: String;
}

/// The id of the request being handled on this task, if any.
pub fn current() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

fn is_safe(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub async fn middleware(req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| is_safe(s))
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let span = tracing::info_span!("request", request_id = %id, method = %req.method(), path = %req.uri().path());
    let mut res = REQUEST_ID.scope(id.clone(), next.run(req).instrument(span)).await;
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut().insert("x-request-id", v);
    }
    res
}
```

`crates/dockerer-api/src/headers.rs`:
```rust
use crate::IsTls;
use axum::{
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; connect-src 'self'; font-src 'self' data:; frame-ancestors 'none'; \
base-uri 'self'; form-action 'self'";

pub async fn security_headers(req: Request, next: Next) -> Response {
    let tls = req.extensions().get::<IsTls>().is_some_and(|t| t.0);
    let is_api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if is_api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if tls {
        h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=31536000"));
    }
    res
}
```

`crates/dockerer-api/src/routes/mod.rs` (Tasks 7–9 add more modules and routes here):
```rust
use crate::{error::ApiError, AppState};
use axum::{routing::get, Json, Router};
use dockerer_core::{now_unix, Error};
use dockerer_store::NewAudit;
use serde_json::{json, Value};

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .fallback(not_found)
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn not_found() -> ApiError {
    ApiError(Error::NotFound("no such API route".into()))
}

/// Records an audit row. Failures are logged, never surfaced to the caller.
#[allow(dead_code)] // first used in Task 7
pub(crate) async fn audit(
    s: &AppState,
    user_id: Option<i64>,
    action: &str,
    target_type: &str,
    target_id: &str,
    result: &str,
) {
    let entry = NewAudit {
        ts: now_unix(),
        user_id,
        environment_id: None,
        action,
        target_type: Some(target_type),
        target_id: Some(target_id),
        result,
        detail: None,
    };
    if let Err(e) = s.store.insert_audit(&entry).await {
        tracing::error!(error = %e, action, "failed to write audit log");
    }
}
```

`crates/dockerer-api/src/auth/mod.rs` (Tasks 6–7 add more):
```rust
pub mod limiter;
pub mod setup;
```

Now create `src/auth/limiter.rs` and `src/auth/setup.rs` exactly as in **Task 6 Step 3**.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p dockerer-api && make lint`
Expected: `basics` (4) and `error` unit tests (3) PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/dockerer-api Cargo.lock
git commit -m "feat(api): router skeleton with error mapping, request ids and security headers"
```

---

### Task 6: Auth primitives: password, token, cookie, limiter, setup guard

**Files:**
- Create: `crates/dockerer-api/src/auth/password.rs`, `token.rs`, `cookie.rs`
- Modify: `crates/dockerer-api/src/auth/limiter.rs`, `setup.rs` (created in Task 5; add tests here)
- Modify: `crates/dockerer-api/src/auth/mod.rs`

**Interfaces:**
- Consumes: `dockerer_core::{Error, Result}`.
- Produces:
  - `password::{hash_password(String) -> Result<String>, verify_password(String, String) -> bool, burn_verify(String)}` (all async).
  - `token::{generate_token() -> String, hash_token(&str) -> String, constant_time_eq(&str, &str) -> bool}`.
  - `cookie::{SESSION_COOKIE, read_cookie(&HeaderMap, &str) -> Option<String>, session_cookie(&str, i64, bool) -> HeaderValue, clear_cookie(bool) -> HeaderValue}`.
  - `limiter::{LoginLimiter::new(u32, Duration), check(&self, &str, Instant) -> Result<()>, record_failure(&self, &str, Instant), reset(&self, &str), limiter_key(&str, &str) -> String}`.
  - `setup::SetupGuard::{new(Instant, Duration), is_open(&self, Instant) -> bool}`.
  - Constants in `auth`: `SESSION_IDLE_SECS`, `SESSION_ABSOLUTE_SECS`, `SESSION_TOUCH_SECS`, `CSRF_HEADER`.

- [ ] **Step 1: Write failing tests**

Append to `crates/dockerer-api/src/auth/limiter.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(300);

    #[test]
    fn blocks_after_max_failures_until_window_passes() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(l.check("k", t0).is_ok());
            l.record_failure("k", t0);
        }
        match l.check("k", t0 + Duration::from_secs(10)) {
            Err(Error::RateLimited { retry_after_secs }) => assert_eq!(retry_after_secs, 290),
            other => panic!("expected rate limit, got {other:?}"),
        }
        assert!(l.check("other", t0).is_ok());
        assert!(l.check("k", t0 + W).is_ok());
    }

    #[test]
    fn reset_clears_failures() {
        let l = LoginLimiter::new(1, W);
        let t0 = Instant::now();
        l.record_failure("k", t0);
        assert!(l.check("k", t0).is_err());
        l.reset("k");
        assert!(l.check("k", t0).is_ok());
    }

    #[test]
    fn failures_outside_window_restart_count() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for _ in 0..4 {
            l.record_failure("k", t0);
        }
        l.record_failure("k", t0 + W + Duration::from_secs(1));
        assert!(l.check("k", t0 + W + Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn map_is_bounded() {
        let l = LoginLimiter::new(5, W);
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_KEYS {
            l.record_failure(&format!("k{i}"), t0);
        }
        l.record_failure("new", t0 + W + Duration::from_secs(1));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn key_is_case_insensitive_on_username() {
        assert_eq!(limiter_key("Admin", "1.2.3.4"), limiter_key("admin", "1.2.3.4"));
        assert_ne!(limiter_key("admin", "1.2.3.4"), limiter_key("admin", "5.6.7.8"));
    }
}
```

Append to `crates/dockerer-api/src/auth/setup.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_only_within_window() {
        let t0 = Instant::now();
        let g = SetupGuard::new(t0, Duration::from_secs(600));
        assert!(g.is_open(t0));
        assert!(g.is_open(t0 + Duration::from_secs(599)));
        assert!(!g.is_open(t0 + Duration::from_secs(600)));
        assert!(!SetupGuard::new(t0, Duration::ZERO).is_open(t0));
    }
}
```

`crates/dockerer-api/src/auth/token.rs` (tests first):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex_256_bit() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_deterministic_sha256_hex() {
        assert_eq!(hash_token("abc"), hash_token("abc"));
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
    }
}
```

`crates/dockerer-api/src/auth/cookie.rs` (tests first):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;

    #[test]
    fn reads_named_cookie_across_headers() {
        let mut h = HeaderMap::new();
        h.append(COOKIE, "a=1; other=2".parse().unwrap());
        h.append(COOKIE, "b=3; dockerer_session=abc".parse().unwrap());
        assert_eq!(read_cookie(&h, SESSION_COOKIE).as_deref(), Some("abc"));
        assert_eq!(read_cookie(&h, "missing"), None);
    }

    #[test]
    fn cookie_flags() {
        assert_eq!(
            session_cookie("tok", 60, false),
            "dockerer_session=tok; Path=/; HttpOnly; SameSite=Strict; Max-Age=60"
        );
        assert!(session_cookie("tok", 60, true).to_str().unwrap().ends_with("; Secure"));
        assert!(clear_cookie(false).to_str().unwrap().contains("Max-Age=0"));
    }
}
```

`crates/dockerer-api/src/auth/password.rs` (tests first):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_then_verify() {
        let h = hash_password("s3cret-pass".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("s3cret-pass".into(), h.clone()).await);
        assert!(!verify_password("wrong".into(), h).await);
        assert!(!verify_password("x".into(), "not-a-hash".into()).await);
    }
}
```

Update `crates/dockerer-api/src/auth/mod.rs`:
```rust
pub mod cookie;
pub mod limiter;
pub mod password;
pub mod setup;
pub mod token;

pub const SESSION_IDLE_SECS: i64 = 7 * 86_400;
pub const SESSION_ABSOLUTE_SECS: i64 = 30 * 86_400;
/// Minimum gap between `last_seen_at` writes, to avoid a DB write per request.
pub const SESSION_TOUCH_SECS: i64 = 60;
pub const CSRF_HEADER: &str = "x-csrf-token";
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-api --lib`
Expected: FAIL to compile (`generate_token`, `read_cookie`, `hash_password` not found). The limiter/setup tests would already pass because their code was written in Task 5, which is expected; they pin that behaviour.

- [ ] **Step 3: Implement**

Prepend to `crates/dockerer-api/src/auth/limiter.rs` (replacing any Task 5 body above the tests):
```rust
use dockerer_core::{Error, Result};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Beyond this many tracked keys, stale entries are pruned on insert so an
/// attacker spraying usernames cannot grow memory without bound.
pub const MAX_TRACKED_KEYS: usize = 10_000;

struct Entry {
    failures: u32,
    first: Instant,
}

/// Fixed-window failure counter keyed by `limiter_key(username, ip)`.
pub struct LoginLimiter {
    max_failures: u32,
    window: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

pub fn limiter_key(username: &str, ip: &str) -> String {
    format!("{}|{}", username.to_lowercase(), ip)
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self { max_failures, window, entries: Mutex::new(HashMap::new()) }
    }

    pub fn check(&self, key: &str, now: Instant) -> Result<()> {
        let map = self.entries.lock().unwrap();
        if let Some(e) = map.get(key) {
            let elapsed = now.duration_since(e.first);
            if elapsed < self.window && e.failures >= self.max_failures {
                let retry_after_secs = (self.window - elapsed).as_secs().max(1);
                return Err(Error::RateLimited { retry_after_secs });
            }
        }
        Ok(())
    }

    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut map = self.entries.lock().unwrap();
        if map.len() >= MAX_TRACKED_KEYS {
            map.retain(|_, e| now.duration_since(e.first) < self.window);
        }
        let e = map.entry(key.to_string()).or_insert(Entry { failures: 0, first: now });
        if now.duration_since(e.first) >= self.window {
            *e = Entry { failures: 0, first: now };
        }
        e.failures += 1;
    }

    pub fn reset(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}
```

Prepend to `crates/dockerer-api/src/auth/setup.rs`:
```rust
use std::time::{Duration, Instant};

/// First-run setup is only accepted for a short window after process start,
/// so a stranger cannot claim a forgotten, exposed instance.
pub struct SetupGuard {
    started: Instant,
    window: Duration,
}

impl SetupGuard {
    pub fn new(started: Instant, window: Duration) -> Self {
        Self { started, window }
    }

    pub fn is_open(&self, now: Instant) -> bool {
        now.duration_since(self.started) < self.window
    }
}
```

Prepend to `crates/dockerer-api/src/auth/token.rs`:
```rust
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// 256 bits from the OS RNG, hex encoded.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Sessions are looked up by this hash so a DB leak does not leak live tokens.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
```

Prepend to `crates/dockerer-api/src/auth/cookie.rs`:
```rust
use axum::http::{header::COOKIE, HeaderMap, HeaderValue};

pub const SESSION_COOKIE: &str = "dockerer_session";

pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
}

pub fn session_cookie(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    ))
    .expect("token is hex, so the cookie is a valid header value")
}

pub fn clear_cookie(secure: bool) -> HeaderValue {
    session_cookie("", 0, secure)
}
```

Prepend to `crates/dockerer-api/src/auth/password.rs`:
```rust
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use dockerer_core::{Error, Result};
use std::sync::OnceLock;

/// argon2id with the crate's default (OWASP-recommended) parameters.
/// Runs on the blocking pool because hashing takes tens of milliseconds.
pub async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_blocking(&password))
        .await
        .map_err(|e| Error::Internal(format!("hash task failed: {e}")))?
}

fn hash_blocking(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("password hashing failed: {e}")))
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_blocking(&password, &hash))
        .await
        .unwrap_or(false)
}

fn verify_blocking(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

/// Spends the same time as a real verification so response timing does not
/// reveal whether a username exists.
pub async fn burn_verify(password: String) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let _ = tokio::task::spawn_blocking(move || {
        let dummy = DUMMY.get_or_init(|| hash_blocking("dockerer-dummy-password").unwrap_or_default());
        verify_blocking(&password, dummy)
    })
    .await;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dockerer-api && make lint`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dockerer-api
git commit -m "feat(api): auth primitives (argon2id, tokens, cookies, login limiter, setup guard)"
```

---

### Task 7: Auth routes: status, setup, login, logout, me, plus session/CSRF extractors

**Files:**
- Create: `crates/dockerer-api/src/auth/extract.rs`, `crates/dockerer-api/src/routes/auth.rs`
- Modify: `crates/dockerer-api/src/auth/mod.rs` (add `pub mod extract;`), `crates/dockerer-api/src/routes/mod.rs` (add routes; remove `#[allow(dead_code)]` on `audit`)
- Test: `crates/dockerer-api/tests/auth.rs`

**Interfaces:**
- Consumes: Task 6 primitives, `Store` sessions/users, `AppState`, `IsTls`, `routes::audit`.
- Produces:
  - `ClientMeta { ip: String, user_agent: Option<String>, tls: bool }` (infallible extractor).
  - `CurrentUser { user: User, session: SessionRecord }`: validates the session, enforces CSRF on unsafe methods, and touches `last_seen_at`.
  - `AdminUser(pub CurrentUser)`: 403 unless the user is admin.
  - `routes::auth::{Credentials, SessionInfo { user, csrf_token }}`.
  - Routes: `GET /auth/status`, `POST /auth/setup`, `POST /auth/login`, `POST /auth/logout`, `GET /auth/me`.

- [ ] **Step 1: Write failing tests**

`crates/dockerer-api/tests/auth.rs`:
```rust
mod common;

use axum::http::StatusCode;
use common::*;
use dockerer_api::{auth::token::hash_token, ApiConfig, IsTls};
use dockerer_core::now_unix;
use dockerer_store::NewSession;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn status_reports_setup_required_then_done() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/auth/status", None, None)).await;
    assert_eq!(r.json, json!({"setup_required": true, "setup_open": true}));
    app.setup_admin().await;
    let r = app.send(req("GET", "/api/v1/auth/status", None, None)).await;
    assert_eq!(r.json, json!({"setup_required": false, "setup_open": false}));
}

#[tokio::test]
async fn setup_creates_admin_and_logs_in() {
    let app = app().await;
    let s = app.setup_admin().await;
    let me = app.send(req("GET", "/api/v1/auth/me", None, Some(&s))).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json["user"]["username"], ADMIN);
    assert_eq!(me.json["user"]["role"], "admin");
    assert_eq!(me.json["csrf_token"], s.csrf.as_str());
    assert!(me.json["user"].get("password_hash").is_none());
}

#[tokio::test]
async fn second_setup_conflicts() {
    let app = app().await;
    app.setup_admin().await;
    let r = app
        .send(req("POST", "/api/v1/auth/setup", Some(json!({"username": "x", "password": "password1"})), None))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn setup_locked_after_window() {
    let app = app_with(ApiConfig { setup_window: Duration::ZERO }).await;
    let r = app.send(req("GET", "/api/v1/auth/status", None, None)).await;
    assert_eq!(r.json["setup_open"], false);
    let r = app
        .send(req("POST", "/api/v1/auth/setup", Some(json!({"username": ADMIN, "password": ADMIN_PW})), None))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.json["message"].as_str().unwrap().contains("restart"));
}

#[tokio::test]
async fn setup_validates_input() {
    let app = app().await;
    let r = app
        .send(req("POST", "/api/v1/auth/setup", Some(json!({"username": ADMIN, "password": "short"})), None))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json["code"], "invalid");
    let r = app
        .send(req("POST", "/api/v1/auth/setup", Some(json!({"username": "bad name", "password": ADMIN_PW})), None))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn concurrent_setup_creates_exactly_one_admin() {
    let app = app().await;
    let mut handles = Vec::new();
    for i in 0..8 {
        let router = app.router.clone();
        handles.push(tokio::spawn(async move {
            let r = req(
                "POST",
                "/api/v1/auth/setup",
                Some(json!({"username": format!("admin{i}"), "password": ADMIN_PW})),
                None,
            );
            tower::ServiceExt::oneshot(router, r).await.unwrap().status()
        }));
    }
    let mut created = 0;
    for h in handles {
        match h.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => {}
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(app.store.count_users().await.unwrap(), 1);
}

#[tokio::test]
async fn login_success_and_failures() {
    let app = app().await;
    app.setup_admin().await;
    let ok = app.login(ADMIN, ADMIN_PW).await;
    assert_eq!(ok.status, StatusCode::OK);
    session_from(&ok);
    assert_eq!(app.login(ADMIN, "wrong-password").await.status, StatusCode::UNAUTHORIZED);
    let unknown = app.login("ghost", "whatever-pw").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.json["message"], "invalid username or password");
}

#[tokio::test]
async fn login_requires_json_content_type() {
    let app = app().await;
    app.setup_admin().await;
    let mut r = req("POST", "/api/v1/auth/login", Some(json!({"username": ADMIN, "password": ADMIN_PW})), None);
    r.headers_mut().insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(app.send(r).await.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn login_is_case_insensitive_and_rate_limit_shares_key() {
    let app = app().await;
    app.setup_admin().await;
    assert_eq!(app.login("ADMIN", ADMIN_PW).await.status, StatusCode::OK);
    for i in 0..5 {
        let name = if i % 2 == 0 { "Admin" } else { "admin" };
        assert_eq!(app.login(name, "nope-nope").await.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = app.login(ADMIN, ADMIN_PW).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers.get("retry-after").is_some());

    let other_ip = app
        .send(req_from(
            [10, 0, 0, 9],
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": ADMIN, "password": ADMIN_PW})),
            None,
        ))
        .await;
    assert_eq!(other_ip.status, StatusCode::OK);
}

#[tokio::test]
async fn me_requires_session() {
    let app = app().await;
    let r = app.send(req("GET", "/api/v1/auth/me", None, None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let bogus = Session { cookie: "dockerer_session=deadbeef".into(), csrf: String::new() };
    let r = app.send(req("GET", "/api/v1/auth/me", None, Some(&bogus))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutations_require_matching_csrf() {
    let app = app().await;
    let s = app.setup_admin().await;
    let missing = Session { csrf: String::new(), ..s.clone() };
    let r = app.send(req("POST", "/api/v1/auth/logout", None, Some(&missing))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let wrong = Session { csrf: "0".repeat(64), ..s.clone() };
    let r = app.send(req("POST", "/api/v1/auth/logout", None, Some(&wrong))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app.send(req("POST", "/api/v1/auth/logout", None, Some(&s))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn logout_invalidates_session_and_clears_cookie() {
    let app = app().await;
    let s = app.setup_admin().await;
    let r = app.send(req("POST", "/api/v1/auth/logout", None, Some(&s))).await;
    assert!(r.headers.get("set-cookie").unwrap().to_str().unwrap().contains("Max-Age=0"));
    let r = app.send(req("GET", "/api/v1/auth/me", None, Some(&s))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_and_idle_sessions_are_rejected_and_deleted() {
    let app = app().await;
    app.setup_admin().await;
    let uid = app.store.find_credentials(ADMIN).await.unwrap().unwrap().0.id;
    let now = now_unix();
    for (token, created, expires) in [("expired-tok", now - 10, now - 1), ("idle-tok", now - 8 * 86_400, now + 86_400)] {
        let hash = hash_token(token);
        app.store
            .create_session(&NewSession {
                user_id: uid,
                token_hash: &hash,
                csrf_token: "c",
                now: created,
                expires_at: expires,
                user_agent: None,
                ip: None,
            })
            .await
            .unwrap();
        let s = Session { cookie: format!("dockerer_session={token}"), csrf: "c".into() };
        let r = app.send(req("GET", "/api/v1/auth/me", None, Some(&s))).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{token}");
        assert!(app.store.find_session(&hash).await.unwrap().is_none(), "{token} not deleted");
    }
}

#[tokio::test]
async fn cookie_is_secure_only_over_tls() {
    let app = app().await;
    app.setup_admin().await;
    let plain = app.login(ADMIN, ADMIN_PW).await;
    let cookie = plain.headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    assert!(!cookie.contains("Secure"));

    let mut r = req("POST", "/api/v1/auth/login", Some(json!({"username": ADMIN, "password": ADMIN_PW})), None);
    r.extensions_mut().insert(IsTls(true));
    let tls = app.send(r).await;
    assert!(tls.headers.get("set-cookie").unwrap().to_str().unwrap().contains("; Secure"));
}

#[tokio::test]
async fn login_and_setup_are_audited() {
    let app = app().await;
    app.setup_admin().await;
    app.login(ADMIN, "wrong-password").await;
    let actions: Vec<_> = app
        .store
        .list_audit(10, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.action, e.result))
        .collect();
    assert_eq!(
        actions,
        vec![("auth.login".to_string(), "failure".to_string()), ("auth.setup".into(), "success".into())]
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-api --test auth`
Expected: FAIL. Most tests get 404 because the routes don't exist yet.

- [ ] **Step 3: Implement extractors**

`crates/dockerer-api/src/auth/extract.rs`:
```rust
use super::{
    cookie::{read_cookie, SESSION_COOKIE},
    token::{constant_time_eq, hash_token},
    CSRF_HEADER, SESSION_IDLE_SECS, SESSION_TOUCH_SECS,
};
use crate::{ApiError, AppState, IsTls};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header::USER_AGENT, request::Parts, Method},
};
use dockerer_core::{now_unix, Error, Role, User};
use dockerer_store::SessionRecord;
use std::{convert::Infallible, net::SocketAddr};

/// Connection metadata recorded on sessions and used for rate limiting.
pub struct ClientMeta {
    pub ip: String,
    pub user_agent: Option<String>,
    pub tls: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for ClientMeta {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        let user_agent = parts
            .headers
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.chars().take(256).collect());
        let tls = parts.extensions.get::<IsTls>().is_some_and(|t| t.0);
        Ok(ClientMeta { ip, user_agent, tls })
    }
}

/// An authenticated user. Extracting this enforces CSRF on every method
/// other than GET/HEAD/OPTIONS, so handlers cannot forget it.
pub struct CurrentUser {
    pub user: User,
    pub session: SessionRecord,
}

fn is_safe_method(m: &Method) -> bool {
    matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn unauthorized() -> ApiError {
    Error::Unauthorized("authentication required".into()).into()
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = read_cookie(&parts.headers, SESSION_COOKIE).ok_or_else(unauthorized)?;
        let session = state
            .store
            .find_session(&hash_token(&token))
            .await?
            .ok_or_else(unauthorized)?;
        let now = now_unix();
        if !session.is_active(now, SESSION_IDLE_SECS) {
            state.store.delete_session(session.id).await?;
            return Err(unauthorized());
        }
        if !is_safe_method(&parts.method) {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !constant_time_eq(sent, &session.csrf_token) {
                return Err(Error::Forbidden("missing or invalid CSRF token".into()).into());
            }
        }
        if now - session.last_seen_at >= SESSION_TOUCH_SECS {
            state.store.touch_session(session.id, now).await?;
        }
        let user = match state.store.get_user(session.user_id).await {
            Ok(u) => u,
            Err(Error::NotFound(_)) => return Err(unauthorized()),
            Err(e) => return Err(e.into()),
        };
        Ok(CurrentUser { user, session })
    }
}

pub struct AdminUser(pub CurrentUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let cu = CurrentUser::from_request_parts(parts, state).await?;
        if cu.user.role != Role::Admin {
            return Err(Error::Forbidden("admin role required".into()).into());
        }
        Ok(AdminUser(cu))
    }
}
```

Add `pub mod extract;` to `crates/dockerer-api/src/auth/mod.rs`.

- [ ] **Step 4: Implement auth routes**

`crates/dockerer-api/src/routes/auth.rs`:
```rust
use crate::{
    auth::{
        cookie::{clear_cookie, session_cookie},
        extract::{ClientMeta, CurrentUser},
        limiter::limiter_key,
        password, token, SESSION_ABSOLUTE_SECS,
    },
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::State,
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dockerer_core::{now_unix, validate_password, validate_username, Error, User};
use dockerer_store::NewSession;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub user: User,
    pub csrf_token: String,
}

pub async fn status(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let users = s.store.count_users().await?;
    Ok(Json(json!({
        "setup_required": users == 0,
        "setup_open": users == 0 && s.setup.is_open(Instant::now()),
    })))
}

pub async fn setup(
    State(s): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<Credentials>,
) -> ApiResult<Response> {
    if s.store.count_users().await? > 0 {
        return Err(Error::Conflict("setup already completed".into()).into());
    }
    if !s.setup.is_open(Instant::now()) {
        return Err(Error::Forbidden(
            "setup window expired; restart dockerer to create the admin account".into(),
        )
        .into());
    }
    validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = password::hash_password(body.password).await?;
    let user = s.store.create_first_admin(&body.username, &hash, now_unix()).await?;
    audit(&s, Some(user.id), "auth.setup", "user", &user.id.to_string(), "success").await;
    start_session(&s, user, &meta, StatusCode::CREATED).await
}

pub async fn login(
    State(s): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<Credentials>,
) -> ApiResult<Response> {
    let key = limiter_key(&body.username, &meta.ip);
    let now = Instant::now();
    s.limiter.check(&key, now)?;

    let verified = match s.store.find_credentials(&body.username).await? {
        Some((user, hash)) => password::verify_password(body.password, hash).await.then_some(user),
        None => {
            password::burn_verify(body.password).await;
            None
        }
    };
    let Some(user) = verified else {
        s.limiter.record_failure(&key, now);
        audit(&s, None, "auth.login", "user", &body.username, "failure").await;
        return Err(Error::Unauthorized("invalid username or password".into()).into());
    };
    s.limiter.reset(&key);
    audit(&s, Some(user.id), "auth.login", "user", &user.id.to_string(), "success").await;
    start_session(&s, user, &meta, StatusCode::OK).await
}

pub async fn logout(State(s): State<AppState>, meta: ClientMeta, cu: CurrentUser) -> ApiResult<Response> {
    s.store.delete_session(cu.session.id).await?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(SET_COOKIE, clear_cookie(meta.tls));
    Ok(res)
}

pub async fn me(cu: CurrentUser) -> Json<SessionInfo> {
    Json(SessionInfo { user: cu.user, csrf_token: cu.session.csrf_token })
}

async fn start_session(s: &AppState, user: User, meta: &ClientMeta, status: StatusCode) -> ApiResult<Response> {
    let session_token = token::generate_token();
    let csrf_token = token::generate_token();
    let now = now_unix();
    s.store
        .create_session(&NewSession {
            user_id: user.id,
            token_hash: &token::hash_token(&session_token),
            csrf_token: &csrf_token,
            now,
            expires_at: now + SESSION_ABSOLUTE_SECS,
            user_agent: meta.user_agent.as_deref(),
            ip: Some(&meta.ip),
        })
        .await?;
    let mut res = (status, Json(SessionInfo { user, csrf_token })).into_response();
    res.headers_mut()
        .insert(SET_COOKIE, session_cookie(&session_token, SESSION_ABSOLUTE_SECS, meta.tls));
    Ok(res)
}
```

Replace `api()` in `crates/dockerer-api/src/routes/mod.rs`, add `pub mod auth;` at the top, and remove the `#[allow(dead_code)]` line above `audit`:
```rust
pub mod auth;

use axum::routing::post;

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/auth/status", get(auth::status))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .fallback(not_found)
}
```
(Merge the `use axum::routing::post;` into the existing `use axum::{routing::get, ...}` line as `routing::{get, post}`.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p dockerer-api && make lint`
Expected: all `auth` tests (15) PASS, earlier tests still PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/dockerer-api
git commit -m "feat(api): setup/login/logout/me with cookie sessions, CSRF and rate limiting"
```

---

### Task 8: Self-service sessions and password change

**Files:**
- Modify: `crates/dockerer-api/src/routes/auth.rs` (add handlers), `crates/dockerer-api/src/routes/mod.rs` (add routes)
- Test: `crates/dockerer-api/tests/sessions.rs`

**Interfaces:**
- Consumes: `CurrentUser`, the `Store` session methods, `password`.
- Produces:
  - `GET /auth/sessions` returns `[{id, created_at, last_seen_at, expires_at, user_agent, ip, current}]`.
  - `DELETE /auth/sessions/{id}` returns 204, or 404 when the session is missing or not owned.
  - `PUT /auth/password {current_password, new_password}` returns 204 and revokes the user's other sessions.

- [ ] **Step 1: Write failing tests**

`crates/dockerer-api/tests/sessions.rs`:
```rust
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn list_marks_current_session() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let _s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);
    let r = app.send(req("GET", "/api/v1/auth/sessions", None, Some(&s1))).await;
    assert_eq!(r.status, StatusCode::OK);
    let list = r.json.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    assert_eq!(list[0]["ip"], "127.0.0.1");
}

#[tokio::test]
async fn revoke_other_session() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);
    let list = app.send(req("GET", "/api/v1/auth/sessions", None, Some(&s1))).await.json;
    let other = list.as_array().unwrap().iter().find(|s| s["current"] == false).unwrap()["id"].as_i64().unwrap();

    let r = app.send(req("DELETE", &format!("/api/v1/auth/sessions/{other}"), None, Some(&s1))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&s2))).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&s1))).await.status, StatusCode::OK);
}

#[tokio::test]
async fn cannot_revoke_someone_elses_session() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    let admin_sessions = app.send(req("GET", "/api/v1/auth/sessions", None, Some(&admin))).await.json;
    let admin_sid = admin_sessions[0]["id"].as_i64().unwrap();
    let r = app.send(req("DELETE", &format!("/api/v1/auth/sessions/{admin_sid}"), None, Some(&bob))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn change_password_flow() {
    let app = app().await;
    let s1 = app.setup_admin().await;
    let s2 = session_from(&app.login(ADMIN, ADMIN_PW).await);

    let wrong = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": "nope-nope", "new_password": "brand-new-pw"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);

    let short = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": ADMIN_PW, "new_password": "short"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);

    let ok = app
        .send(req(
            "PUT",
            "/api/v1/auth/password",
            Some(json!({"current_password": ADMIN_PW, "new_password": "brand-new-pw"})),
            Some(&s1),
        ))
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&s1))).await.status, StatusCode::OK);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&s2))).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.login(ADMIN, ADMIN_PW).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.login(ADMIN, "brand-new-pw").await.status, StatusCode::OK);
}
```

Note: `cannot_revoke_someone_elses_session` needs `POST /users` from Task 9. Mark it `#[ignore = "needs Task 9"]` now and remove the attribute in Task 9 Step 4.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-api --test sessions`
Expected: FAIL with 404 status on `/auth/sessions` and `/auth/password`.

- [ ] **Step 3: Implement**

Append to `crates/dockerer-api/src/routes/auth.rs`:
```rust
use crate::auth::SESSION_IDLE_SECS;
use axum::extract::Path;

#[derive(Serialize)]
pub struct SessionView {
    pub id: i64,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub current: bool,
}

pub async fn list_sessions(State(s): State<AppState>, cu: CurrentUser) -> ApiResult<Json<Vec<SessionView>>> {
    let now = now_unix();
    let views = s
        .store
        .list_sessions(cu.user.id)
        .await?
        .into_iter()
        .filter(|r| r.is_active(now, SESSION_IDLE_SECS))
        .map(|r| SessionView {
            current: r.id == cu.session.id,
            id: r.id,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
            user_agent: r.user_agent,
            ip: r.ip,
        })
        .collect();
    Ok(Json(views))
}

pub async fn revoke_session(
    State(s): State<AppState>,
    cu: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    if !s.store.delete_user_session(cu.user.id, id).await? {
        return Err(Error::NotFound(format!("session {id}")).into());
    }
    audit(&s, Some(cu.user.id), "auth.session.revoke", "session", &id.to_string(), "success").await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(s): State<AppState>,
    cu: CurrentUser,
    Json(body): Json<ChangePassword>,
) -> ApiResult<StatusCode> {
    let (_, hash) = s
        .store
        .find_credentials(&cu.user.username)
        .await?
        .ok_or_else(|| Error::Unauthorized("authentication required".into()))?;
    if !password::verify_password(body.current_password, hash).await {
        return Err(Error::Invalid("current password is incorrect".into()).into());
    }
    validate_password(&body.new_password)?;
    let new_hash = password::hash_password(body.new_password).await?;
    s.store.set_password_hash(cu.user.id, &new_hash, now_unix()).await?;
    s.store.delete_user_sessions(cu.user.id, Some(cu.session.id)).await?;
    audit(&s, Some(cu.user.id), "auth.password.change", "user", &cu.user.id.to_string(), "success").await;
    Ok(StatusCode::NO_CONTENT)
}
```
(Move the new `use` lines to the top of the file with the other imports.)

Add to `api()` in `routes/mod.rs` (import `delete` and `put` from `axum::routing`):
```rust
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/{id}", delete(auth::revoke_session))
        .route("/auth/password", put(auth::change_password))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dockerer-api && make lint`
Expected: PASS (1 ignored).

- [ ] **Step 5: Commit**

```bash
git add crates/dockerer-api
git commit -m "feat(api): list/revoke own sessions and change password"
```

---

### Task 9: Admin users CRUD and audit list

**Files:**
- Create: `crates/dockerer-api/src/routes/users.rs`, `crates/dockerer-api/src/routes/audit_log.rs`
- Modify: `crates/dockerer-api/src/routes/mod.rs`, `crates/dockerer-api/tests/sessions.rs` (remove the `#[ignore]`)
- Test: `crates/dockerer-api/tests/users.rs`

**Interfaces:**
- Consumes: `AdminUser`, the `Store` user and audit methods.
- Produces:
  - `GET /users` → `[User]`
  - `POST /users {username, password, role}` → 201 `User`
  - `PATCH /users/{id} {role?, password?}` → `User`
  - `DELETE /users/{id}` → 204
  - `GET /audit?limit=&before=` → `[AuditEntry]` (limit defaults to 100, clamped to 1..=500)

- [ ] **Step 1: Write failing tests**

`crates/dockerer-api/tests/users.rs`:
```rust
mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn admin_creates_and_lists_users() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app.create_user(&admin, "bob", "bob-password", "user").await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json["username"], "bob");
    assert_eq!(r.json["role"], "user");
    let list = app.send(req("GET", "/api/v1/users", None, Some(&admin))).await;
    assert_eq!(list.json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn create_validates_and_rejects_duplicates() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let dup = app.create_user(&admin, "BOB", "bob-password", "user").await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert!(dup.json["message"].as_str().unwrap().contains("already taken"));
    assert_eq!(app.create_user(&admin, "carol", "short", "user").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        app.create_user(&admin, "carol", "carol-password", "root").await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn non_admin_is_forbidden() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    let bob = session_from(&app.login("bob", "bob-password").await);
    assert_eq!(app.send(req("GET", "/api/v1/users", None, Some(&bob))).await.status, StatusCode::FORBIDDEN);
    assert_eq!(app.create_user(&bob, "eve", "eve-password", "admin").await.status, StatusCode::FORBIDDEN);
    assert_eq!(app.send(req("GET", "/api/v1/audit", None, Some(&bob))).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn role_change_takes_effect_immediately() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app.create_user(&admin, "bob", "bob-password", "user").await.json["id"].as_i64().unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req("PATCH", &format!("/api/v1/users/{id}"), Some(json!({"role": "admin"})), Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["role"], "admin");
    assert_eq!(app.send(req("GET", "/api/v1/users", None, Some(&bob))).await.status, StatusCode::OK);
}

#[tokio::test]
async fn admin_password_reset_revokes_target_sessions() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app.create_user(&admin, "bob", "bob-password", "user").await.json["id"].as_i64().unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app
        .send(req("PATCH", &format!("/api/v1/users/{id}"), Some(json!({"password": "reset-password"})), Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&bob))).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.login("bob", "reset-password").await.status, StatusCode::OK);
}

#[tokio::test]
async fn admin_resetting_own_password_keeps_current_session() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app.send(req("GET", "/api/v1/auth/me", None, Some(&admin))).await.json["user"]["id"].as_i64().unwrap();
    let r = app
        .send(req("PATCH", &format!("/api/v1/users/{id}"), Some(json!({"password": "another-password"})), Some(&admin)))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&admin))).await.status, StatusCode::OK);
}

#[tokio::test]
async fn patch_validates() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app.send(req("PATCH", "/api/v1/users/999", Some(json!({"role": "user"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.send(req("PATCH", "/api/v1/users/1", Some(json!({})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app.send(req("PATCH", "/api/v1/users/1", Some(json!({"password": "short"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn last_admin_is_protected() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let r = app.send(req("PATCH", "/api/v1/users/1", Some(json!({"role": "user"})), Some(&admin))).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = app.send(req("DELETE", "/api/v1/users/1", None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_user_kills_their_sessions() {
    let app = app().await;
    let admin = app.setup_admin().await;
    let id = app.create_user(&admin, "bob", "bob-password", "user").await.json["id"].as_i64().unwrap();
    let bob = session_from(&app.login("bob", "bob-password").await);
    let r = app.send(req("DELETE", &format!("/api/v1/users/{id}"), None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.send(req("GET", "/api/v1/auth/me", None, Some(&bob))).await.status, StatusCode::UNAUTHORIZED);
    let r = app.send(req("DELETE", &format!("/api/v1/users/{id}"), None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn audit_lists_admin_actions_with_paging() {
    let app = app().await;
    let admin = app.setup_admin().await;
    app.create_user(&admin, "bob", "bob-password", "user").await;
    app.create_user(&admin, "carol", "carol-password", "user").await;
    let r = app.send(req("GET", "/api/v1/audit?limit=2", None, Some(&admin))).await;
    assert_eq!(r.status, StatusCode::OK);
    let page = r.json.as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["action"], "user.create");
    assert_eq!(page[0]["username"], ADMIN);
    let before = page[1]["id"].as_i64().unwrap();
    let rest = app.send(req("GET", &format!("/api/v1/audit?before={before}"), None, Some(&admin))).await;
    assert_eq!(rest.json.as_array().unwrap().last().unwrap()["action"], "auth.setup");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-api --test users`
Expected: FAIL with 404 on `/users` and `/audit`.

- [ ] **Step 3: Implement**

`crates/dockerer-api/src/routes/users.rs`:
```rust
use crate::{auth::{extract::AdminUser, password}, routes::audit, ApiResult, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use dockerer_core::{now_unix, validate_password, validate_username, Error, Role, User};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    pub role: Role,
}

#[derive(Deserialize)]
pub struct UpdateUser {
    pub role: Option<Role>,
    pub password: Option<String>,
}

pub async fn list(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(s.store.list_users().await?))
}

pub async fn create(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<User>)> {
    validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = password::hash_password(body.password).await?;
    let user = s
        .store
        .create_user(&body.username, &hash, body.role, now_unix())
        .await
        .map_err(|e| match e {
            Error::Conflict(_) => Error::Conflict(format!("username '{}' is already taken", body.username)),
            other => other,
        })?;
    audit(&s, Some(admin.user.id), "user.create", "user", &user.id.to_string(), "success").await;
    Ok((StatusCode::CREATED, Json(user)))
}

pub async fn update(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUser>,
) -> ApiResult<Json<User>> {
    if body.role.is_none() && body.password.is_none() {
        return Err(Error::Invalid("nothing to update".into()).into());
    }
    if let Some(pw) = &body.password {
        validate_password(pw)?;
    }
    s.store.get_user(id).await?;
    let now = now_unix();
    if let Some(role) = body.role {
        s.store.set_user_role(id, role, now).await?;
    }
    if let Some(pw) = body.password {
        let hash = password::hash_password(pw).await?;
        s.store.set_password_hash(id, &hash, now).await?;
        let keep = (id == admin.user.id).then_some(admin.session.id);
        s.store.delete_user_sessions(id, keep).await?;
    }
    audit(&s, Some(admin.user.id), "user.update", "user", &id.to_string(), "success").await;
    Ok(Json(s.store.get_user(id).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    s.store.delete_user(id).await?;
    audit(&s, Some(admin.user.id), "user.delete", "user", &id.to_string(), "success").await;
    Ok(StatusCode::NO_CONTENT)
}
```

`crates/dockerer-api/src/routes/audit_log.rs`:
```rust
use crate::{auth::extract::AdminUser, ApiResult, AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use dockerer_store::AuditEntry;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i64>,
    pub before: Option<i64>,
}

pub async fn list(
    State(s): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(s.store.list_audit(limit, q.before).await?))
}
```

In `routes/mod.rs`, add `pub mod audit_log; pub mod users;` (import `patch` too) and extend `api()`:
```rust
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", patch(users::update).delete(users::remove))
        .route("/audit", get(audit_log::list))
```

- [ ] **Step 4: Un-ignore the Task 8 test**

Remove the `#[ignore = "needs Task 9"]` line from `cannot_revoke_someone_elses_session` in `crates/dockerer-api/tests/sessions.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p dockerer-api && make lint`
Expected: all PASS, 0 ignored.

- [ ] **Step 6: Commit**

```bash
git add crates/dockerer-api
git commit -m "feat(api): admin user management and audit log listing"
```

---

### Task 10: `dockerer-server`: config, embedded SPA, maintenance, `dockerer` binary

**Files:**
- Create: `crates/dockerer-server/Cargo.toml`, `build.rs`, `src/lib.rs`, `src/main.rs`, `src/config.rs`, `src/static_files.rs`, `src/maintenance.rs`
- Test: `crates/dockerer-server/tests/static_files.rs`, `crates/dockerer-server/tests/maintenance.rs`

**Interfaces:**
- Consumes: `dockerer_api::{app, AppState, ApiConfig, IsTls, auth::SESSION_IDLE_SECS}`, `Store`.
- Produces:
  - `dockerer_server::config::Config` (clap).
  - `dockerer_server::static_files::router() -> Router`.
  - `dockerer_server::maintenance::prune_once(&Store, now: i64) -> Result<(u64, u64)>`.
  - `dockerer_server::run(Config) -> anyhow::Result<()>`.
  - The `dockerer` binary.

- [ ] **Step 1: Create manifest, build script, and failing tests**

`crates/dockerer-server/Cargo.toml`:
```toml
[package]
name = "dockerer-server"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "dockerer"
path = "src/main.rs"

[dependencies]
dockerer-core.workspace = true
dockerer-store.workspace = true
dockerer-api.workspace = true
anyhow.workspace = true
axum.workspace = true
axum-server.workspace = true
clap.workspace = true
mime_guess.workspace = true
rcgen.workspace = true
rust-embed.workspace = true
rustls.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
http-body-util.workspace = true
tempfile.workspace = true
tower.workspace = true
```

`crates/dockerer-server/build.rs`:
```rust
//! rust-embed needs web/dist to exist at compile time. Until the web UI is
//! built (plan P3), write a placeholder page so the server still compiles.
use std::{fs, path::PathBuf};

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Dockerer</title></head>\
<body><h1>Dockerer</h1><p>The web UI has not been built yet. The API is available under <code>/api/v1</code>.</p></body></html>\n";

fn main() {
    let dist = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../web/dist");
    if !dist.join("index.html").exists() {
        fs::create_dir_all(&dist).expect("create web/dist");
        fs::write(dist.join("index.html"), PLACEHOLDER).expect("write placeholder index.html");
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
```

`crates/dockerer-server/tests/static_files.rs`:
```rust
use axum::{body::Body, http::{Request, StatusCode}};
use dockerer_api::{ApiConfig, AppState};
use dockerer_server::static_files;
use dockerer_store::Store;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(router: axum::Router, uri: &str) -> (StatusCode, String, String) {
    let res = router.oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let ct = res.headers().get("content-type").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    let body = String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned();
    (status, ct, body)
}

#[tokio::test]
async fn root_and_client_routes_serve_index() {
    for uri in ["/", "/envs/local/containers/abc"] {
        let (status, ct, body) = get(static_files::router(), uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ct.starts_with("text/html"), "{uri}: {ct}");
        assert!(body.contains("<title>Dockerer</title>") || body.contains("Dockerer"), "{uri}");
    }
}

#[tokio::test]
async fn missing_asset_is_404_not_index() {
    let (status, _, _) = get(static_files::router(), "/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_api_path_outside_v1_is_json_404() {
    let (status, ct, body) = get(static_files::router(), "/api/v2/things").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(ct.starts_with("application/json"));
    assert!(body.contains("not_found"));
}

#[tokio::test]
async fn full_app_serves_api_and_spa_with_security_headers() {
    let store = Store::in_memory().await.unwrap();
    let app = dockerer_api::app(AppState::new(store, ApiConfig::default()), static_files::router());
    let res = app.clone().oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get("content-security-policy").is_some());
    let (status, _, body) = get(app, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ok"));
}
```

`crates/dockerer-server/tests/maintenance.rs`:
```rust
use dockerer_core::Role;
use dockerer_server::maintenance::prune_once;
use dockerer_store::{NewAudit, NewSession, Store};

#[tokio::test]
async fn prunes_expired_sessions_and_old_audit() {
    let s = Store::in_memory().await.unwrap();
    let uid = s.create_user("a", "h", Role::Admin, 0).await.unwrap().id;
    let now = 100 * 86_400;
    let base = NewSession {
        user_id: uid,
        token_hash: "old",
        csrf_token: "c",
        now: now - 10,
        expires_at: now - 1,
        user_agent: None,
        ip: None,
    };
    s.create_session(&base).await.unwrap();
    s.create_session(&NewSession { token_hash: "live", expires_at: now + 1000, ..base }).await.unwrap();
    let audit = |ts| NewAudit {
        ts,
        user_id: None,
        environment_id: None,
        action: "x",
        target_type: None,
        target_id: None,
        result: "success",
        detail: None,
    };
    s.insert_audit(&audit(now - 91 * 86_400)).await.unwrap();
    s.insert_audit(&audit(now - 89 * 86_400)).await.unwrap();

    assert_eq!(prune_once(&s, now).await.unwrap(), (1, 1));
    assert!(s.find_session("live").await.unwrap().is_some());
    assert_eq!(s.list_audit(10, None).await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-server`
Expected: FAIL to compile (`dockerer_server::static_files` not found).

- [ ] **Step 3: Implement**

`crates/dockerer-server/src/config.rs`:
```rust
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser, Debug, Clone)]
#[command(name = "dockerer", version, about = "Lightweight container management")]
pub struct Config {
    /// Directory for the SQLite database and TLS material.
    #[arg(long, env = "DOCKERER_DATA", default_value = "/data")]
    pub data: PathBuf,
    /// Plain HTTP listen address.
    #[arg(long, env = "DOCKERER_BIND", default_value = "0.0.0.0:9000")]
    pub bind: SocketAddr,
    /// HTTPS listen address; HTTPS is disabled when unset.
    #[arg(long, env = "DOCKERER_TLS_BIND")]
    pub tls_bind: Option<SocketAddr>,
    /// PEM certificate; a self-signed one is generated when unset.
    #[arg(long, env = "DOCKERER_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key matching --tls-cert.
    #[arg(long, env = "DOCKERER_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
    /// Docker endpoint for the local environment (used from plan P2).
    #[arg(long, env = "DOCKER_HOST", default_value = "unix:///var/run/docker.sock")]
    pub docker_host: String,
    /// tracing filter, e.g. `info` or `dockerer_api=debug,info`.
    #[arg(long, env = "DOCKERER_LOG", default_value = "info")]
    pub log_level: String,
}
```

`crates/dockerer-server/src/static_files.rs`:
```rust
use axum::{
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde_json::json;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
struct Assets;

pub fn router() -> Router {
    Router::new().fallback(serve)
}

async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        let body = json!({"code": "not_found", "message": "no such API route"});
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }
    if let Some(res) = asset(path) {
        return res;
    }
    // Paths with a file extension are real asset requests; everything else
    // is a client-side route handled by the SPA.
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    asset("index.html").unwrap_or_else(|| (StatusCode::NOT_FOUND, "web UI not built").into_response())
}

fn asset(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).ok()?),
        (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
    ];
    Some((headers, file.data).into_response())
}
```

`crates/dockerer-server/src/maintenance.rs`:
```rust
use dockerer_api::auth::SESSION_IDLE_SECS;
use dockerer_core::{now_unix, Result};
use dockerer_store::Store;
use std::time::Duration;

/// Deletes expired sessions and audit rows past retention.
/// Returns `(sessions_deleted, audit_rows_deleted)`.
pub async fn prune_once(store: &Store, now: i64) -> Result<(u64, u64)> {
    let sessions = store.delete_expired_sessions(now, SESSION_IDLE_SECS).await?;
    let days = store.audit_retention_days().await?;
    let audit = store.prune_audit(now - days * 86_400).await?;
    Ok((sessions, audit))
}

pub async fn run(store: Store) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        match prune_once(&store, now_unix()).await {
            Ok((s, a)) => tracing::debug!(sessions = s, audit = a, "maintenance prune"),
            Err(e) => tracing::warn!(error = %e, "maintenance prune failed"),
        }
    }
}
```

`crates/dockerer-server/src/lib.rs`:
```rust
pub mod config;
pub mod maintenance;
pub mod static_files;

use axum::{Extension, Router};
use config::Config;
use dockerer_api::{ApiConfig, AppState, IsTls};
use dockerer_store::Store;
use std::{net::SocketAddr, time::Duration};

pub async fn run(config: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data)?;
    let store = Store::open(&config.data.join("dockerer.db")).await?;
    let app = dockerer_api::app(AppState::new(store.clone(), ApiConfig::default()), static_files::router());
    tokio::spawn(maintenance::run(store));
    serve(&config, app).await
}

async fn serve(config: &Config, app: Router) -> anyhow::Result<()> {
    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_on_signal(handle.clone()));
    tracing::info!(addr = %config.bind, "dockerer listening (http)");
    axum_server::bind(config.bind)
        .handle(handle)
        .serve(
            app.layer(Extension(IsTls(false)))
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    Ok(())
}

async fn shutdown_on_signal(handle: axum_server::Handle) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
    handle.graceful_shutdown(Some(Duration::from_secs(10)));
}
```

`crates/dockerer-server/src/main.rs`:
```rust
use clap::Parser;
use dockerer_server::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    dockerer_server::run(config).await
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace && make lint`
Expected: all PASS. `web/dist/index.html` placeholder now exists (git-ignored).

- [ ] **Step 5: Manual smoke test**

Run:
```bash
cargo run -p dockerer-server -- --data ./.dev-data --bind 127.0.0.1:9000 &
sleep 3
curl -s localhost:9000/api/v1/auth/status
curl -s -c /tmp/dk.cookies -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct-horse"}' localhost:9000/api/v1/auth/setup
curl -s -b /tmp/dk.cookies localhost:9000/api/v1/auth/me
curl -s localhost:9000/ | head -3
kill %1
```
Expected:
- `{"setup_required":true,"setup_open":true}`
- the setup call returns a JSON user plus `csrf_token`
- `me` returns the admin
- `/` returns the placeholder HTML
- the process exits cleanly on kill

Then `rm -rf .dev-data`.

- [ ] **Step 6: Commit**

```bash
git add crates/dockerer-server Cargo.lock
git commit -m "feat(server): dockerer binary with config, embedded SPA fallback and maintenance"
```

---

### Task 11: TLS listener with self-signed fallback

**Files:**
- Create: `crates/dockerer-server/src/tls.rs`
- Modify: `crates/dockerer-server/src/lib.rs` (add `pub mod tls;`, dual listener in `serve`)
- Test: `crates/dockerer-server/tests/tls.rs`

**Interfaces:**
- Consumes: `Config`.
- Produces:
  - `tls::generate_self_signed(cert: &Path, key: &Path) -> anyhow::Result<()>` (key file mode 0600 on unix).
  - `tls::load_or_generate(&Config) -> anyhow::Result<RustlsConfig>`.
  - `tls::install_crypto_provider()`.

- [ ] **Step 1: Write failing tests**

`crates/dockerer-server/tests/tls.rs`:
```rust
use clap::Parser;
use dockerer_server::{config::Config, tls};

#[test]
fn generates_cert_and_private_key() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("tls/cert.pem");
    let key = dir.path().join("tls/key.pem");
    tls::generate_self_signed(&cert, &key).unwrap();
    assert!(std::fs::read_to_string(&cert).unwrap().contains("BEGIN CERTIFICATE"));
    assert!(std::fs::read_to_string(&key).unwrap().contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[tokio::test]
async fn load_or_generate_reuses_existing_self_signed_cert() {
    tls::install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from(["dockerer", "--data", dir.path().to_str().unwrap()]);
    tls::load_or_generate(&config).await.unwrap();
    let first = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
    tls::load_or_generate(&config).await.unwrap();
    let second = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn explicit_cert_paths_must_exist() {
    tls::install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_from([
        "dockerer",
        "--data",
        dir.path().to_str().unwrap(),
        "--tls-cert",
        "/nonexistent/cert.pem",
        "--tls-key",
        "/nonexistent/key.pem",
    ]);
    assert!(tls::load_or_generate(&config).await.is_err());
}

#[test]
fn cert_without_key_is_rejected_by_cli() {
    assert!(Config::try_parse_from(["dockerer", "--tls-cert", "c.pem"]).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dockerer-server --test tls`
Expected: FAIL to compile (`tls` module missing).

- [ ] **Step 3: Implement**

`crates/dockerer-server/src/tls.rs`:
```rust
use crate::config::Config;
use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;
use std::{fs, io::Write, path::Path};

/// rustls needs a process-wide crypto provider; we use ring (no C toolchain
/// needed for musl cross builds). Safe to call more than once.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub async fn load_or_generate(config: &Config) -> anyhow::Result<RustlsConfig> {
    let (cert, key) = match (&config.tls_cert, &config.tls_key) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        _ => {
            let dir = config.data.join("tls");
            let cert = dir.join("cert.pem");
            let key = dir.join("key.pem");
            if !cert.exists() || !key.exists() {
                tracing::info!(path = %dir.display(), "generating self-signed TLS certificate");
                generate_self_signed(&cert, &key)?;
            }
            (cert, key)
        }
    };
    RustlsConfig::from_pem_file(&cert, &key)
        .await
        .with_context(|| format!("loading TLS cert {} / key {}", cert.display(), key.display()))
}

pub fn generate_self_signed(cert: &Path, key: &Path) -> anyhow::Result<()> {
    if let Some(dir) = cert.parent() {
        fs::create_dir_all(dir)?;
    }
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "dockerer".to_string()])?;
    fs::write(cert, generated.cert.pem())?;
    write_private(key, generated.key_pair.serialize_pem().as_bytes())?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)?;
    Ok(())
}
```

In `crates/dockerer-server/src/lib.rs`, add `pub mod tls;` and replace `serve` with:
```rust
async fn serve(config: &Config, app: Router) -> anyhow::Result<()> {
    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_on_signal(handle.clone()));

    let http = axum_server::bind(config.bind).handle(handle.clone()).serve(
        app.clone()
            .layer(Extension(IsTls(false)))
            .into_make_service_with_connect_info::<SocketAddr>(),
    );
    tracing::info!(addr = %config.bind, "dockerer listening (http)");

    match config.tls_bind {
        None => http.await?,
        Some(addr) => {
            tls::install_crypto_provider();
            let rustls = tls::load_or_generate(config).await?;
            let https = axum_server::bind_rustls(addr, rustls).handle(handle).serve(
                app.layer(Extension(IsTls(true)))
                    .into_make_service_with_connect_info::<SocketAddr>(),
            );
            tracing::info!(%addr, "dockerer listening (https)");
            tokio::try_join!(http, https)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace && make lint`
Expected: all PASS.

- [ ] **Step 5: Manual TLS smoke test**

Run:
```bash
cargo run -p dockerer-server -- --data ./.dev-data --bind 127.0.0.1:9000 --tls-bind 127.0.0.1:9443 &
sleep 3
curl -sk https://127.0.0.1:9443/api/v1/health -D - | grep -i strict-transport
curl -s http://127.0.0.1:9000/api/v1/health -D - | grep -ci strict-transport
curl -sk -c /tmp/dk.cookies -D - -H 'content-type: application/json' \
  -d '{"username":"admin","password":"correct-horse"}' https://127.0.0.1:9443/api/v1/auth/setup | grep -i set-cookie
kill %1; rm -rf .dev-data
```
Expected:
- HTTPS returns a `strict-transport-security` header.
- HTTP prints `0`.
- The HTTPS setup cookie includes `Secure`.

- [ ] **Step 6: Commit**

```bash
git add crates/dockerer-server Cargo.lock
git commit -m "feat(server): optional HTTPS listener with self-signed certificate fallback"
```

---

## Deferred to later plans (tracked, not forgotten)

- **P2:**
  - Environment registry and `environments` repository, `Environment`/`DockerOps` traits, `LocalDocker` (bollard), `authorize()`.
  - All `/envs/{env}/...` endpoints, the websocket hub (`events`/`stats`/`logs`/`exec`/`task`), and compose projects.
  - utoipa OpenAPI for all handlers (including P1's), and audit rows for Docker mutations.
- **P3:**
  - React UI (setup/login/users/audit/sessions pages plus all Docker views), the secret-masking UI, and the command palette.
  - Playwright e2e, multi-arch distroless image, GitHub Actions CI.
