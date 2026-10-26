-- An admin can disable an account (a compromised one, say) without
-- deleting it: it can't sign in, and keeps its grants, teams and history.
ALTER TABLE users ADD COLUMN disabled_at INTEGER;
