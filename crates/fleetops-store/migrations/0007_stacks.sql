-- Stacks (stacks spec §1): a Compose project FleetOps deploys to a Docker
-- environment, its unsaved draft, and one row per deploy.
CREATE TABLE stacks (
    id                INTEGER PRIMARY KEY,
    environment_id    INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    name              TEXT    NOT NULL,
    source            TEXT    NOT NULL CHECK (source IN ('inline', 'git')),
    deployed_revision INTEGER,
    created_at        INTEGER NOT NULL,
    created_by        INTEGER REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE (environment_id, name)
);
-- `variables` is a JSON array of KEY=value lines, sealed (AAD `stack:<stack_id>`).
CREATE TABLE stack_drafts (
    stack_id   INTEGER PRIMARY KEY REFERENCES stacks(id) ON DELETE CASCADE,
    compose    TEXT    NOT NULL,
    variables  TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL
);
-- `variables` is sealed with AAD `stack-revision:<id>`.
CREATE TABLE stack_revisions (
    id          INTEGER PRIMARY KEY,
    stack_id    INTEGER NOT NULL REFERENCES stacks(id) ON DELETE CASCADE,
    number      INTEGER NOT NULL,
    compose     TEXT    NOT NULL,
    variables   TEXT    NOT NULL,
    git_commit  TEXT,
    status      TEXT    NOT NULL CHECK (status IN ('deploying', 'deployed', 'failed', 'superseded')),
    error       TEXT,
    kind        TEXT    NOT NULL CHECK (kind IN ('deploy', 'rollback')),
    rollback_of INTEGER,
    created_at  INTEGER NOT NULL,
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE (stack_id, number)
);
CREATE INDEX stack_revisions_status ON stack_revisions(status);
