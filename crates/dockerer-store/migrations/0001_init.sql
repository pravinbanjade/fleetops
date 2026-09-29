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
