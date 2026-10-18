-- Scheduled backups (backup spec): the schedule, where backups go, and
-- what each run did.
CREATE TABLE ee_backup_schedule (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    enabled         INTEGER NOT NULL DEFAULT 0,
    hour_utc        INTEGER NOT NULL DEFAULT 2 CHECK (hour_utc BETWEEN 0 AND 23),
    retention_kind  TEXT    NOT NULL DEFAULT 'count' CHECK (retention_kind IN ('count', 'days')),
    retention_value INTEGER NOT NULL DEFAULT 7 CHECK (retention_value >= 1),
    -- Sealed with the server key (aad "backup:1").
    passphrase      TEXT,
    updated_at      INTEGER NOT NULL
);
CREATE TABLE ee_backup_targets (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL UNIQUE CHECK (kind IN ('s3', 'local')),
    -- JSON, without secrets.
    settings   TEXT    NOT NULL,
    -- A JSON map of secrets, sealed with the server key (aad "backup-target:<id>").
    secrets    TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE ee_backup_runs (
    id          INTEGER PRIMARY KEY,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    trigger     TEXT    NOT NULL CHECK (trigger IN ('schedule', 'manual', 'catch_up')),
    target_id   INTEGER REFERENCES ee_backup_targets(id) ON DELETE SET NULL,
    target_kind TEXT,
    result      TEXT    NOT NULL CHECK (result IN ('success', 'failure', 'skipped')),
    file_name   TEXT,
    size        INTEGER,
    error       TEXT
);
CREATE INDEX ee_backup_runs_started ON ee_backup_runs(started_at);
