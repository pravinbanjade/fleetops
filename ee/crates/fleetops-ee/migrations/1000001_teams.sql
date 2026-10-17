-- Teams (teams spec): groups of users that carry environment access.
CREATE TABLE ee_teams (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL COLLATE NOCASE UNIQUE,
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE ee_team_members (
    team_id    INTEGER NOT NULL REFERENCES ee_teams(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (team_id, user_id)
);
CREATE INDEX ee_team_members_user ON ee_team_members(user_id);
CREATE TABLE ee_team_bindings (
    team_id        INTEGER NOT NULL REFERENCES ee_teams(id) ON DELETE CASCADE,
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role           TEXT    NOT NULL CHECK (role IN ('viewer', 'operator')),
    -- JSON array of namespace names; NULL is the whole environment.
    namespaces     TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (team_id, environment_id)
);
CREATE INDEX ee_team_bindings_env ON ee_team_bindings(environment_id);
