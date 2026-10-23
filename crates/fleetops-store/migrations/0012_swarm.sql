-- Swarm environments (swarm spec §2, §3): one shared agent key per
-- environment, rotated with a grace period for the previous one; the swarm
-- each environment is pinned to; and what each node's agent last reported.
ALTER TABLE agent_credentials ADD COLUMN prev_key_hash TEXT;
ALTER TABLE agent_credentials ADD COLUMN prev_key_expires_at INTEGER;
ALTER TABLE agent_credentials ADD COLUMN key_generation INTEGER NOT NULL DEFAULT 1;
CREATE UNIQUE INDEX agent_credentials_prev_key ON agent_credentials(prev_key_hash);

CREATE TABLE swarm_clusters (
    environment_id INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    cluster_id     TEXT NOT NULL,
    trust_root     TEXT NOT NULL,
    pinned_at      INTEGER NOT NULL
);

CREATE TABLE swarm_agents (
    environment_id INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    node_id        TEXT NOT NULL,
    hostname       TEXT,
    manager        INTEGER NOT NULL DEFAULT 0,
    agent_version  TEXT,
    last_seen_at   INTEGER,
    last_error     TEXT,
    PRIMARY KEY (environment_id, node_id)
);
