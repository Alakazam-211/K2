-- 0214: one node, many servers — the server's mirror of its share (0.46.3,
-- prd-compute-shared-queue-v1 §11.2).
--
-- The node is the truth (`/etc/k2-node/servers.toml` + its own pins); a
-- server only mirrors what the node's offer says about this server's share
-- (`Offer.share`, protocol 3), for display and placement:
--
-- - `role`: `owner` (this server enrolled the node first: admin verbs,
--   updates, health) or `tenant` (another server owns the node; this server
--   never runs node admin on it, V52). Every existing node is `owner`.
-- - `shared`: two or more servers are active on the node.
-- - `owner_label`: the owner server's label ("shared by …"), for tenants.
-- - `share_json` / `share_at`: the last `ShareView` and when it arrived.
--
-- Additive: a node that never sends `share` (solo, or older than 0.46.3)
-- leaves every row as it was.
ALTER TABLE compute_nodes ADD COLUMN role TEXT NOT NULL DEFAULT 'owner' CHECK (role IN ('owner','tenant'));
ALTER TABLE compute_nodes ADD COLUMN shared INTEGER NOT NULL DEFAULT 0;
ALTER TABLE compute_nodes ADD COLUMN owner_label TEXT;
ALTER TABLE compute_nodes ADD COLUMN share_json TEXT;
ALTER TABLE compute_nodes ADD COLUMN share_at INTEGER;
