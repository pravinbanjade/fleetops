-- Sub-project 3: secrets an environment is built from (kubeconfigs),
-- encrypted with the server key. `summary` is safe to show.
CREATE TABLE env_credentials (
  env_id     INTEGER PRIMARY KEY REFERENCES environments(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  ciphertext TEXT NOT NULL,
  summary    TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
