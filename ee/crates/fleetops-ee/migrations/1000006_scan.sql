-- Image vulnerability scanning (vuln-scanning spec): settings, scan runs and
-- their findings, and where each image is used.
CREATE TABLE ee_scan_global (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    enabled       INTEGER NOT NULL DEFAULT 0,
    -- JSON Mirror without the password; NULL uses Trivy's public locations.
    mirror        TEXT,
    -- The mirror password, sealed with the server key (aad "scan-global:<id>").
    mirror_secret TEXT,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE ee_scan_env (
    env_id        INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
    -- NULL inherits the global switch.
    enabled       INTEGER,
    mirror        TEXT,
    -- Sealed (aad "scan-env:<env_id>").
    mirror_secret TEXT,
    -- The engine socket on the host; NULL detects it.
    socket_path   TEXT,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE ee_scans (
    id               INTEGER PRIMARY KEY,
    image_key        TEXT    NOT NULL,
    env_id           INTEGER NOT NULL,
    node             TEXT    NOT NULL DEFAULT '',
    image_ref        TEXT    NOT NULL,
    namespace        TEXT    NOT NULL DEFAULT '',
    pull_secrets     TEXT    NOT NULL DEFAULT '[]',
    trigger          TEXT    NOT NULL CHECK (trigger IN ('arrival', 'manual', 'periodic', 'enable')),
    status           TEXT    NOT NULL CHECK (status IN ('queued', 'running', 'done', 'failed')),
    queued_at        INTEGER NOT NULL,
    started_at       INTEGER,
    finished_at      INTEGER,
    error            TEXT,
    trivy_image      TEXT,
    critical         INTEGER,
    high             INTEGER,
    medium           INTEGER,
    low              INTEGER,
    unknown          INTEGER,
    critical_fixable INTEGER,
    high_fixable     INTEGER
);
CREATE INDEX ee_scans_key ON ee_scans(image_key, id);
CREATE INDEX ee_scans_status ON ee_scans(status);
-- Only each key's latest finished scan keeps its findings.
CREATE TABLE ee_scan_findings (
    scan_id           INTEGER NOT NULL REFERENCES ee_scans(id) ON DELETE CASCADE,
    vuln_id           TEXT    NOT NULL,
    pkg_name          TEXT    NOT NULL,
    installed_version TEXT    NOT NULL,
    fixed_version     TEXT,
    severity          TEXT    NOT NULL,
    title             TEXT,
    url               TEXT,
    PRIMARY KEY (scan_id, vuln_id, pkg_name, installed_version)
);
CREATE TABLE ee_scan_first_seen (
    image_key     TEXT    NOT NULL,
    vuln_id       TEXT    NOT NULL,
    pkg_name      TEXT    NOT NULL,
    first_seen_at INTEGER NOT NULL,
    PRIMARY KEY (image_key, vuln_id, pkg_name)
);
-- Replaced per environment on every inventory.
CREATE TABLE ee_scan_uses (
    env_id       INTEGER NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    node         TEXT    NOT NULL DEFAULT '',
    image_key    TEXT    NOT NULL,
    image_ref    TEXT    NOT NULL,
    image_id     TEXT,
    namespace    TEXT    NOT NULL DEFAULT '',
    workload     TEXT    NOT NULL DEFAULT '',
    container    TEXT    NOT NULL DEFAULT '',
    running      INTEGER NOT NULL,
    pull_secrets TEXT    NOT NULL DEFAULT '[]'
);
CREATE INDEX ee_scan_uses_env ON ee_scan_uses(env_id);
CREATE INDEX ee_scan_uses_key ON ee_scan_uses(image_key);
