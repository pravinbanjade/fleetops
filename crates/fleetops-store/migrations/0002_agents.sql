-- Sub-project 2: credentials and last-known state of agent environments.
-- Hashes are hex SHA-256 of the high-entropy tokens; the tokens themselves
-- are never stored.
CREATE TABLE agent_credentials (
    environment_id  INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    key_hash        TEXT UNIQUE,
    join_token_hash TEXT UNIQUE,
    join_expires_at INTEGER,
    last_seen_at    INTEGER,
    agent_version   TEXT,
    host_info       TEXT,
    last_error      TEXT
);
