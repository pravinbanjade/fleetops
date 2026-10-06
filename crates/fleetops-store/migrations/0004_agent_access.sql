-- Sub-project 3b: the access level a kube-agent manifest grants
-- (read-only, standard, cluster-admin); null for Docker agents.
ALTER TABLE agent_credentials ADD COLUMN access TEXT;
UPDATE agent_credentials SET access = 'standard'
  WHERE environment_id IN (SELECT id FROM environments WHERE kind = 'kube-agent');
