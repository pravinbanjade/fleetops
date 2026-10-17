-- Custom roles (custom roles spec): a role is a set of grid permissions,
-- granted to a user or a team on an environment like a built-in role.
CREATE TABLE ee_roles (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL COLLATE NOCASE UNIQUE,
    description TEXT,
    -- JSON array of `row.verb` permissions.
    permissions TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
-- A custom role granted straight to a user. A (user, environment) has at
-- most one grant: this row or a `role_bindings` row, never both.
CREATE TABLE ee_user_role_bindings (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role_id        INTEGER NOT NULL REFERENCES ee_roles(id) ON DELETE CASCADE,
    -- JSON array of namespace names; NULL is the whole environment.
    namespaces     TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (user_id, environment_id)
);
CREATE INDEX ee_user_role_bindings_env ON ee_user_role_bindings(environment_id);
CREATE INDEX ee_user_role_bindings_role ON ee_user_role_bindings(role_id);
-- A custom role granted to a team; at most one grant per (team,
-- environment) across this table and `ee_team_bindings`.
CREATE TABLE ee_team_role_bindings (
    team_id        INTEGER NOT NULL REFERENCES ee_teams(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role_id        INTEGER NOT NULL REFERENCES ee_roles(id) ON DELETE CASCADE,
    namespaces     TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (team_id, environment_id)
);
CREATE INDEX ee_team_role_bindings_env ON ee_team_role_bindings(environment_id);
CREATE INDEX ee_team_role_bindings_role ON ee_team_role_bindings(role_id);
-- Every team grant, built-in or custom, in one shape.
CREATE VIEW ee_team_grants AS
    SELECT team_id, environment_id, role, NULL AS role_id, NULL AS role_name,
           NULL AS permissions, namespaces, updated_at
    FROM ee_team_bindings
    UNION ALL
    SELECT b.team_id, b.environment_id, NULL, r.id, r.name, r.permissions,
           b.namespaces, b.updated_at
    FROM ee_team_role_bindings b JOIN ee_roles r ON r.id = b.role_id;
