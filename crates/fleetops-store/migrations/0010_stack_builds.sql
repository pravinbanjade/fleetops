-- Stack builds (stack build spec §3). `build_trees` is the git tree id of
-- each build service's context at the draft's commit, written by syncs;
-- `builds` is what a revision built: service → {tree, image}.
ALTER TABLE stack_drafts ADD COLUMN build_trees TEXT NOT NULL DEFAULT '{}';
ALTER TABLE stack_revisions ADD COLUMN builds TEXT NOT NULL DEFAULT '{}';
