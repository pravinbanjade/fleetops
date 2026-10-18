-- Single sign-on (SSO spec): identity provider connections, the users they
-- manage, and group-to-team mappings.
CREATE TABLE ee_sso_connections (
    id                   INTEGER PRIMARY KEY,
    name                 TEXT    NOT NULL COLLATE NOCASE UNIQUE,
    kind                 TEXT    NOT NULL CHECK (kind IN ('microsoft', 'google', 'github', 'oidc', 'ldap')),
    enabled              INTEGER NOT NULL DEFAULT 0,
    -- JSON, without secrets.
    settings             TEXT    NOT NULL,
    -- A JSON map of secrets, sealed with the server key (aad "sso:<id>").
    secrets              TEXT,
    require_mapped_group INTEGER NOT NULL DEFAULT 0,
    session_hours        INTEGER NOT NULL DEFAULT 12,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL
);
-- The identity behind an SSO user; at most one per user.
CREATE TABLE ee_sso_identities (
    user_id       INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    connection_id INTEGER NOT NULL REFERENCES ee_sso_connections(id) ON DELETE CASCADE,
    subject       TEXT    NOT NULL,
    email         TEXT,
    -- JSON array: the groups at the last login.
    groups        TEXT    NOT NULL DEFAULT '[]',
    last_login_at INTEGER NOT NULL,
    UNIQUE (connection_id, subject)
);
-- A group maps to a team or to the admin role, never both.
CREATE TABLE ee_sso_mappings (
    id            INTEGER PRIMARY KEY,
    connection_id INTEGER NOT NULL REFERENCES ee_sso_connections(id) ON DELETE CASCADE,
    group_value   TEXT    NOT NULL COLLATE NOCASE,
    team_id       INTEGER REFERENCES ee_teams(id) ON DELETE CASCADE,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    CHECK ((team_id IS NULL) <> (is_admin = 0))
);
CREATE UNIQUE INDEX ee_sso_mappings_uniq
    ON ee_sso_mappings(connection_id, group_value, IFNULL(team_id, 0), is_admin);
-- Sign-in redirects in flight; each row is used once.
CREATE TABLE ee_sso_states (
    state         TEXT    PRIMARY KEY,
    connection_id INTEGER NOT NULL REFERENCES ee_sso_connections(id) ON DELETE CASCADE,
    nonce         TEXT    NOT NULL,
    pkce_verifier TEXT    NOT NULL,
    binding_hash  TEXT    NOT NULL,
    return_to     TEXT,
    expires_at    INTEGER NOT NULL
);
-- NULL: added by an admin. Otherwise the connection whose group sync added it.
ALTER TABLE ee_team_members ADD COLUMN source_connection_id INTEGER
    REFERENCES ee_sso_connections(id) ON DELETE SET NULL;
