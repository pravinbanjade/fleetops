-- Slice 3g: the kube-agent manifest also grants read on every kind.
ALTER TABLE agent_credentials ADD COLUMN custom_resources INTEGER NOT NULL DEFAULT 0;
