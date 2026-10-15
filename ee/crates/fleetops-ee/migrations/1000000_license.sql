-- Enterprise tables use versions from 1000000 up (editions spec §3.3).
CREATE TABLE ee_license (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    raw         TEXT    NOT NULL,
    uploaded_at INTEGER NOT NULL,
    uploaded_by INTEGER
);
