-- Notifications (notifications spec): destinations, rules, SMTP, what each
-- rule has reported, batches waiting to send, and the delivery outbox/log.
CREATE TABLE ee_notify_destinations (
    id            INTEGER PRIMARY KEY,
    name          TEXT    NOT NULL UNIQUE,
    kind          TEXT    NOT NULL CHECK (kind IN ('slack', 'email', 'webhook', 'pagerduty', 'teams', 'discord')),
    -- JSON without secrets: {"host": ..., "recipients": [...]}.
    settings      TEXT    NOT NULL,
    -- Sealed JSON {"url", "signing_secret", "routing_key"} (aad "notify-dest:<id>").
    secrets       TEXT,
    last_error    TEXT,
    last_error_at INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE ee_notify_rules (
    id           INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL UNIQUE,
    enabled      INTEGER NOT NULL DEFAULT 1,
    -- JSON environment ids; [] is every environment.
    envs         TEXT    NOT NULL DEFAULT '[]',
    min_severity TEXT    NOT NULL CHECK (min_severity IN ('critical', 'high', 'medium', 'low')),
    fixable_only INTEGER NOT NULL,
    running_only INTEGER NOT NULL,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE TABLE ee_notify_rule_destinations (
    rule_id        INTEGER NOT NULL REFERENCES ee_notify_rules(id) ON DELETE CASCADE,
    destination_id INTEGER NOT NULL REFERENCES ee_notify_destinations(id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, destination_id)
);
CREATE TABLE ee_notify_smtp (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    host         TEXT    NOT NULL,
    port         INTEGER NOT NULL,
    security     TEXT    NOT NULL CHECK (security IN ('starttls', 'tls', 'none')),
    username     TEXT,
    -- Sealed (aad "notify-smtp:<id>").
    password     TEXT,
    from_address TEXT    NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE TABLE ee_notify_known (
    rule_id          INTEGER NOT NULL REFERENCES ee_notify_rules(id) ON DELETE CASCADE,
    repository       TEXT    NOT NULL,
    -- 1 until its first result is recorded silently (baseline).
    baseline_pending INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (rule_id, repository)
);
CREATE TABLE ee_notify_reported (
    rule_id     INTEGER NOT NULL REFERENCES ee_notify_rules(id) ON DELETE CASCADE,
    repository  TEXT    NOT NULL,
    cve         TEXT    NOT NULL,
    reported_at INTEGER NOT NULL,
    PRIMARY KEY (rule_id, repository, cve)
);
CREATE TABLE ee_notify_pending (
    rule_id   INTEGER NOT NULL REFERENCES ee_notify_rules(id) ON DELETE CASCADE,
    kind      TEXT    NOT NULL CHECK (kind IN ('cve', 'image')),
    group_key TEXT    NOT NULL,
    -- JSON Vec<PendingItem>.
    items     TEXT    NOT NULL,
    first_at  INTEGER NOT NULL,
    last_at   INTEGER NOT NULL,
    PRIMARY KEY (rule_id, kind, group_key)
);
CREATE TABLE ee_notify_deliveries (
    id             INTEGER PRIMARY KEY,
    destination_id INTEGER REFERENCES ee_notify_destinations(id) ON DELETE SET NULL,
    rule_id        INTEGER REFERENCES ee_notify_rules(id) ON DELETE SET NULL,
    title          TEXT    NOT NULL,
    -- JSON Message.
    message        TEXT    NOT NULL,
    status         TEXT    NOT NULL CHECK (status IN ('queued', 'sent', 'failed')),
    attempts       INTEGER NOT NULL DEFAULT 0,
    next_at        INTEGER NOT NULL,
    error          TEXT,
    created_at     INTEGER NOT NULL,
    sent_at        INTEGER
);
CREATE INDEX ee_notify_deliveries_due ON ee_notify_deliveries(status, next_at);
-- A repository's baseline stays open until every digest of it in scope at
-- baseline time has a result.
CREATE TABLE ee_notify_baseline_keys (
    rule_id    INTEGER NOT NULL REFERENCES ee_notify_rules(id) ON DELETE CASCADE,
    repository TEXT    NOT NULL,
    image_key  TEXT    NOT NULL,
    PRIMARY KEY (rule_id, repository, image_key)
);
-- Environments already baselined since scanning was turned on, so a restart
-- (which looks like "turned on" to the scan engine) doesn't baseline again.
CREATE TABLE ee_notify_env_baselined (
    env_id INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE
);
