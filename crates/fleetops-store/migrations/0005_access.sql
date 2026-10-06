-- Sub-project 3c: per-environment roles. The global rows that mirrored
-- users.role are dropped; users.role stays the only global role.
DROP TABLE role_bindings;
CREATE TABLE role_bindings (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role           TEXT    NOT NULL CHECK (role IN ('viewer', 'operator')),
    -- JSON array of namespace names; NULL is the whole environment.
    namespaces     TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (user_id, environment_id)
);
CREATE INDEX role_bindings_env ON role_bindings(environment_id);
-- Nobody loses access on upgrade.
INSERT INTO role_bindings (user_id, environment_id, role, namespaces, created_at, updated_at)
SELECT u.id, e.id, 'operator', NULL, CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER)
FROM users u CROSS JOIN environments e
WHERE u.role = 'user';
