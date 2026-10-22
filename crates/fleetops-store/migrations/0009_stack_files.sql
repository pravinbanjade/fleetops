-- Secrets and configs of stacks (stack secrets spec §2). `value` is sealed
-- with AAD `stack-file:<id>` / `stack-revision-file:<id>`. `source` is
-- `fleetops` for a Secrets-tab value and `repo` for a config file fetched
-- from a git stack's repository.
CREATE TABLE stack_files (
    id          INTEGER PRIMARY KEY,
    stack_id    INTEGER NOT NULL REFERENCES stacks(id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('secret', 'config')),
    source      TEXT    NOT NULL CHECK (source IN ('fleetops', 'repo')),
    name        TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE (stack_id, kind, source, name)
);

-- Every file content a revision deployed, so a rollback puts it back.
CREATE TABLE stack_revision_files (
    id          INTEGER PRIMARY KEY,
    revision_id INTEGER NOT NULL REFERENCES stack_revisions(id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('secret', 'config')),
    name        TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    UNIQUE (revision_id, kind, name)
);
