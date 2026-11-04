-- The os/arch of the node a Kubernetes image runs on, so its scan pod
-- pulls the same platform variant.
ALTER TABLE ee_scan_uses ADD COLUMN platform TEXT;
ALTER TABLE ee_scans ADD COLUMN platform TEXT;
