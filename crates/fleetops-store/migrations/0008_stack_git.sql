-- Git sources for stacks (stacks spec §8). `token` and `webhook_secret` are
-- sealed (AAD `stack-git-token:<stack_id>` and `stack-git-hook:<stack_id>`).
-- The fetched Compose file lives in the stack's draft; `pending_commit` is
-- set while it differs from the deployed revision.
CREATE TABLE stack_git (
    stack_id            INTEGER PRIMARY KEY REFERENCES stacks(id) ON DELETE CASCADE,
    url                 TEXT    NOT NULL,
    branch              TEXT    NOT NULL,
    path                TEXT    NOT NULL DEFAULT 'compose.yaml',
    token               TEXT,
    webhook_secret      TEXT    NOT NULL,
    sync                TEXT    NOT NULL CHECK (sync IN ('manual', 'poll', 'webhook')),
    poll_seconds        INTEGER NOT NULL DEFAULT 300 CHECK (poll_seconds >= 60),
    auto_deploy         INTEGER NOT NULL DEFAULT 0,
    paused              INTEGER NOT NULL DEFAULT 0,
    last_fetched_commit TEXT,
    pending_commit      TEXT,
    last_sync_at        INTEGER,
    last_sync_error     TEXT,
    updated_at          INTEGER NOT NULL,
    updated_by          INTEGER REFERENCES users(id) ON DELETE SET NULL
);
