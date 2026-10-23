-- An admin confirms a swarm before its agents are trusted, and the
-- swarm's managers, as last confirmed by a known one, anchor membership
-- (swarm spec §2).
ALTER TABLE swarm_clusters ADD COLUMN confirmed_at INTEGER;
ALTER TABLE swarm_clusters ADD COLUMN pinned_by TEXT;
ALTER TABLE swarm_clusters ADD COLUMN pinned_node TEXT;
-- JSON array of node ids; null until the swarm is confirmed.
ALTER TABLE swarm_clusters ADD COLUMN known_managers TEXT;
-- Pins made before this release were trusted on first use: keep them.
UPDATE swarm_clusters SET confirmed_at = pinned_at;
