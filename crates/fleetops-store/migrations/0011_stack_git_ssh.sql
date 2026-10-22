-- SSH git sources (stack git SSH spec §2–§3). `ssh_key` is the stack's
-- OpenSSH private key, sealed (AAD `stack-git-key:<stack_id>`);
-- `ssh_public_key` is meant to be shared. `ssh_host_key` is the host key
-- pinned at the first completed handshake (`<algorithm> SHA256:…`).
ALTER TABLE stack_git ADD COLUMN ssh_key TEXT;
ALTER TABLE stack_git ADD COLUMN ssh_public_key TEXT;
ALTER TABLE stack_git ADD COLUMN ssh_host_key TEXT;
